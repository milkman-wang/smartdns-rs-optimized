use std::net::IpAddr;
use std::time::Duration;

use futures::FutureExt;
use futures::future::{Either, select};
use tokio::time::sleep;

use crate::config::SpeedCheckMode;
use crate::dns::*;
use crate::log::debug;
use crate::middleware::*;
use crate::third_ext::FutureTimeoutExt;

pub struct DnsDualStackIpSelectionMiddleware;

impl DnsDualStackIpSelectionMiddleware {
    pub fn is_configured(cfg: &crate::dns_conf::RuntimeConfig) -> bool {
        cfg.dualstack_ip_selection()
            || cfg.rule_groups().values().any(|group| {
                group
                    .domain_rules
                    .iter()
                    .any(|rule| rule.config.dualstack_ip_selection == Some(true))
            })
    }
}

#[async_trait::async_trait]
impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError>
    for DnsDualStackIpSelectionMiddleware
{
    async fn handle(
        &self,
        ctx: &mut DnsContext,
        req: &DnsRequest,
        next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
    ) -> Result<DnsResponse, DnsError> {
        use RecordType::{A, AAAA};

        // highest priority
        if ctx.server_opts.no_dualstack_selection() || ctx.server_opts.no_speed_check() {
            return next.run(ctx, req).await;
        }

        let query_type = req.query().query_type();

        // must be ip query.
        if !query_type.is_ip_addr() {
            return next.run(ctx, req).await;
        }

        let mut prefer_that = false; // As long as it succeeds, there is no need to check the selection threshold.

        if matches!(query_type, A) {
            if ctx.cfg().dualstack_ip_allow_force_aaaa() {
                prefer_that = true;
            } else {
                return next.run(ctx, req).await;
            }
        }

        // read config
        let dualstack_ip_selection = ctx
            .domain_rule
            .as_ref()
            .map(|rule| rule.dualstack_ip_selection)
            .unwrap_or_default()
            .unwrap_or(ctx.cfg().dualstack_ip_selection());

        if !dualstack_ip_selection {
            return next.run(ctx, req).await;
        }

        let selection_threshold =
            Duration::from_millis(ctx.cfg().dualstack_ip_selection_threshold());

        let speed_check_mode = ctx
            .domain_rule
            .get_ref(|r| r.speed_check_mode.as_ref())
            .or_else(|| ctx.cfg().speed_check_mode())
            .cloned();
        let Some(speed_check_mode) = speed_check_mode
            .filter(|modes| !modes.is_empty() && !modes.iter().any(SpeedCheckMode::is_none))
        else {
            return next.run(ctx, req).await;
        };

        let ttl = ctx.cfg().local_ttl() as u32;

        let that_type = match query_type {
            A => AAAA,
            AAAA => A,
            typ => typ,
        };

        let mut that_ctx = ctx.clone();
        that_ctx.is_dualstack = true;
        let that_req = {
            let mut req = req.clone();
            req.set_query_type(that_type);
            req
        };

        let that_next = next.clone();
        let that = async move {
            let start = std::time::Instant::now();
            let result = that_next.run(&mut that_ctx, &that_req).await;
            crate::stats::completed(&that_ctx, &that_req, &result, start.elapsed());
            crate::plugins::completed(&that_ctx, &that_req, &result, start, None);
            result
        };
        let that = std::pin::pin!(that);
        let this = next.run(ctx, req);

        let dual_task = futures::future::select(this, that).await;

        let this_no_records = || {
            debug!(
                "dual stack IP selection: {} , choose {}",
                req.query().name(),
                that_type
            );
            let query = req.query().original().clone();
            let mut response = DnsResponse::new_with_max_ttl(query.clone(), []);
            response.add_authority(Record::from_rdata(
                query.name().clone(),
                ttl,
                RData::default_soa(),
            ));
            Ok(response)
        };

        match dual_task {
            Either::Left((res, that)) => match res {
                Ok(this) => {
                    let that = that.timeout(selection_threshold).await;

                    if let Ok(Ok(that)) = that {
                        let that_faster = matches!(
                            which_faster(&this, &that, &speed_check_mode, selection_threshold)
                                .await,
                            Either::Right(_)
                        );

                        if that_faster && (prefer_that || matches!(query_type, AAAA)) {
                            return this_no_records();
                        }
                    }

                    Ok(this)
                }
                Err(err) => Err(err),
            },
            Either::Right((res, this)) => match res {
                Ok(that) => match this.await {
                    Ok(this) => {
                        let that_faster = matches!(
                            which_faster(&this, &that, &speed_check_mode, selection_threshold)
                                .await,
                            Either::Right(_)
                        );

                        if that_faster && (prefer_that || matches!(query_type, AAAA)) {
                            return this_no_records();
                        }
                        Ok(this)
                    }
                    Err(err) => Err(err),
                },
                Err(_) => this.await,
            },
        }
    }
}

async fn which_faster(
    this: &DnsResponse,
    that: &DnsResponse,
    modes: &[SpeedCheckMode],
    selection_threshold: Duration,
) -> Either<(), ()> {
    match (this.probe_result(), that.probe_result()) {
        (ProbeResult::Measured(this), ProbeResult::Measured(that)) => {
            return if this.saturating_sub(that) > selection_threshold {
                Either::Right(())
            } else {
                Either::Left(())
            };
        }
        (ProbeResult::Failed, ProbeResult::Measured(_)) => return Either::Right(()),
        (ProbeResult::Measured(_), ProbeResult::Failed) => return Either::Left(()),
        _ => (),
    }
    let this_ip_addrs = this.ip_addrs();
    let that_ip_addrs = that.ip_addrs();

    let this_ping = multi_mode_ping_fastest(this_ip_addrs, modes.to_vec()).boxed();
    let that_ping = multi_mode_ping_fastest(that_ip_addrs, modes.to_vec()).boxed();

    let which_faster = select(this_ping, that_ping).await;

    let that_faster = match which_faster {
        Either::Right((Some((_, that_dura)), this_ping)) => match this_ping.await {
            Some((_, this_dura)) => {
                this_dura > that_dura && (this_dura - that_dura) > selection_threshold
            }
            None => true,
        },
        _ => false,
    };

    if that_faster {
        Either::Right(())
    } else {
        Either::Left(())
    }
}

async fn multi_mode_ping_fastest(
    ip_addrs: Vec<IpAddr>,
    modes: Vec<SpeedCheckMode>,
) -> Option<(IpAddr, Duration)> {
    use crate::infra::ping::{PingOptions, ping_fastest};
    let duration = Duration::from_millis(200);
    let ping_ops = PingOptions::default().with_timeout_secs(2);

    let mut fastest_ip = None;

    for mode in &modes {
        let dests = mode.to_ping_addrs(&ip_addrs);

        let ping_task = ping_fastest(dests, ping_ops).boxed();
        let timeout_task = sleep(duration).boxed();
        match futures_util::future::select(ping_task, timeout_task).await {
            futures::future::Either::Left((ping_res, _)) => {
                match ping_res {
                    Ok(ping_out) => {
                        // ping success
                        let ip = ping_out.dest().ip_addr();
                        let duration = ping_out.elapsed();
                        fastest_ip = Some((ip, duration));
                        break;
                    }
                    Err(_) => continue,
                }
            }
            futures::future::Either::Right((_, _)) => {
                // timeout
                continue;
            }
        }
    }

    fastest_ip
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dns_conf::RuntimeConfig,
        dns_mw::DnsMockMiddleware,
        libdns::proto::op::{Query, ResponseCode},
    };
    #[tokio::test]
    async fn test_dualstack_uses_configured_probe_and_returns_nodata() {
        for (speed, filtered) in [("tcp-syn:443", true), ("none", false)] {
            let cfg = RuntimeConfig::builder()
                .with("dualstack-ip-selection yes")
                .with(&format!("speed-check-mode {speed}"))
                .build()
                .unwrap();
            let a = Query::query("dualstack.example.".parse().unwrap(), RecordType::A);
            let aaaa = Query::query(a.name().clone(), RecordType::AAAA);
            let ipv4 = DnsResponse::from_rdata(a.clone(), RData::A("192.0.2.4".parse().unwrap()))
                .with_probe_result(ProbeResult::Measured(Duration::from_millis(1)));
            let ipv6 =
                DnsResponse::from_rdata(aaaa.clone(), RData::AAAA("2001:db8::6".parse().unwrap()))
                    .with_probe_result(ProbeResult::Measured(Duration::from_millis(1000)));
            let handler = DnsMockMiddleware::mock(DnsDualStackIpSelectionMiddleware)
                .with_result(a, Ok(ipv4))
                .with_result(aaaa.clone(), Ok(ipv6))
                .build(cfg);
            let response = handler
                .lookup(aaaa.name().clone(), RecordType::AAAA)
                .await
                .unwrap();
            assert_eq!(response.response_code(), ResponseCode::NoError);
            assert_eq!(response.answers().is_empty(), filtered);
            if filtered {
                assert_eq!(response.authorities()[0].record_type(), RecordType::SOA);
            }
        }
    }
    #[test]
    fn configured_detection_keeps_domain_and_group_overrides() {
        for (lines, expected) in [
            (vec!["dualstack-ip-selection no"], false),
            (vec!["dualstack-ip-selection yes"], true),
            (
                vec![
                    "dualstack-ip-selection no",
                    "domain-rule /example/ -dualstack-ip-selection yes",
                ],
                true,
            ),
            (
                vec![
                    "dualstack-ip-selection no",
                    "domain-rule /example/ -dualstack-ip-selection no",
                ],
                false,
            ),
            (
                vec![
                    "dualstack-ip-selection no",
                    "group-begin work",
                    "domain-rule /example/ -dualstack-ip-selection yes",
                    "group-end",
                ],
                true,
            ),
        ] {
            let mut builder = RuntimeConfig::builder();
            for line in &lines {
                builder = builder.with(line);
            }
            assert_eq!(
                DnsDualStackIpSelectionMiddleware::is_configured(&builder.build().unwrap()),
                expected,
                "{lines:?}"
            );
        }
    }
}
