use crate::config::{ConfigForIP, KernelIpSet, NFTsetConfig};
use crate::dns::*;
use crate::middleware::*;
use std::net::IpAddr;

pub struct DnsNftsetMiddleware;

impl DnsNftsetMiddleware {
    pub fn is_configured(cfg: &crate::dns_conf::RuntimeConfig) -> bool {
        use crate::config::IBindConfig;
        let has_sets =
            |opts: &crate::config::ServerOpts| opts.ipset.is_some() || opts.nftset.is_some();
        cfg.binds().iter().any(|bind| has_sets(bind.server_opts()))
            || cfg
                .client_rules()
                .iter()
                .any(|rule| has_sets(&rule.options))
            || cfg.rule_groups().values().any(|group| {
                !group.nftsets.is_empty()
                    || group.network_sets.ipset_no_speed.is_some()
                    || group.network_sets.nftset_no_speed.is_some()
                    || group
                        .domain_rules
                        .iter()
                        .any(|rule| rule.config.ipset.is_some() || rule.config.nftset.is_some())
            })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SetUpdate {
    IpSet(KernelIpSet, IpAddr, u32),
    NftSet(NFTsetConfig, IpAddr, u32),
}

fn applies<T: crate::config::parser::NomParser>(rule: &ConfigForIP<T>, ip: IpAddr) -> bool {
    match rule {
        ConfigForIP::All(_) | ConfigForIP::None => true,
        ConfigForIP::V4(_) | ConfigForIP::NoneV4 => ip.is_ipv4(),
        ConfigForIP::V6(_) | ConfigForIP::NoneV6 => ip.is_ipv6(),
    }
}

fn domain_sets<T: crate::config::parser::NomParser>(
    mut node: Option<&crate::dns_rule::DomainRuleTreeNode>,
    ip: IpAddr,
    get: impl Fn(&crate::dns_rule::DomainRuleTreeNode) -> Option<&[ConfigForIP<T>]>,
) -> Option<&[ConfigForIP<T>]> {
    while let Some(rule) = node {
        if let Some(sets) = get(rule)
            && sets.iter().any(|rule| applies(rule, ip))
        {
            return Some(sets);
        }
        node = rule.zone().map(AsRef::as_ref);
    }
    None
}

// A family-specific ignore is a rule too: it must suppress the fallback.
fn select_sets<T: crate::config::parser::NomParser>(
    ip: IpAddr,
    layers: [Option<&[ConfigForIP<T>]>; 3],
) -> Vec<&T> {
    for layer in layers.into_iter().flatten() {
        let mut sets = vec![];
        for rule in layer {
            match rule {
                ConfigForIP::None => return vec![],
                ConfigForIP::NoneV4 if ip.is_ipv4() => return vec![],
                ConfigForIP::NoneV6 if ip.is_ipv6() => return vec![],
                ConfigForIP::All(set) => sets.push(set),
                ConfigForIP::V4(set) if ip.is_ipv4() => sets.push(set),
                ConfigForIP::V6(set) if ip.is_ipv6() => sets.push(set),
                _ => (),
            }
        }
        if !sets.is_empty() {
            return sets;
        }
    }
    vec![]
}

fn updates(ctx: &DnsContext, response: &DnsResponse) -> Vec<SetUpdate> {
    if ctx.server_opts.is_background || ctx.server_opts.no_rule_ipset() {
        return vec![];
    }
    let groups = ctx.cfg().rule_groups();
    let default = groups.get("default").map(|g| &g.network_sets);
    let group = ctx
        .server_opts
        .rule_group
        .as_deref()
        .and_then(|name| groups.get(name))
        .map(|g| &g.network_sets);
    let ipset_timeout = group
        .and_then(|g| g.ipset_timeout)
        .or_else(|| default.and_then(|g| g.ipset_timeout))
        .unwrap_or(false);
    let nftset_timeout = group
        .and_then(|g| g.nftset_timeout)
        .or_else(|| default.and_then(|g| g.nftset_timeout))
        .unwrap_or(false);
    let failed = response.probe_result() == ProbeResult::Failed;
    let fallback_ip = failed
        .then(|| {
            group
                .and_then(|g| g.ipset_no_speed.as_deref())
                .or_else(|| default.and_then(|g| g.ipset_no_speed.as_deref()))
        })
        .flatten();
    let fallback_nft = failed
        .then(|| {
            group
                .and_then(|g| g.nftset_no_speed.as_deref())
                .or_else(|| default.and_then(|g| g.nftset_no_speed.as_deref()))
        })
        .flatten();
    let defaults = ctx
        .cfg()
        .find_domain_rule(response.query().name(), "default");
    let mut updates = vec![];
    for record in response.records() {
        let Some(ip) = record.data().ip_addr() else {
            continue;
        };
        let domain_ip = domain_sets(ctx.domain_rule.as_deref(), ip, |r| r.ipset.as_deref())
            .or_else(|| domain_sets(defaults.as_deref(), ip, |r| r.ipset.as_deref()));
        let domain_nft = domain_sets(ctx.domain_rule.as_deref(), ip, |r| r.nftset.as_deref())
            .or_else(|| domain_sets(defaults.as_deref(), ip, |r| r.nftset.as_deref()));
        let timeout = record.ttl().saturating_mul(3).max(1);
        for set in select_sets(
            ip,
            [domain_ip, ctx.server_opts.ipset.as_deref(), fallback_ip],
        ) {
            updates.push(SetUpdate::IpSet(
                set.clone(),
                ip,
                if ipset_timeout { timeout } else { 0 },
            ));
        }
        for set in select_sets(
            ip,
            [domain_nft, ctx.server_opts.nftset.as_deref(), fallback_nft],
        ) {
            updates.push(SetUpdate::NftSet(
                set.clone(),
                ip,
                if nftset_timeout { timeout } else { 0 },
            ));
        }
    }
    updates
}

#[async_trait::async_trait]
impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for DnsNftsetMiddleware {
    async fn handle(
        &self,
        ctx: &mut DnsContext,
        req: &DnsRequest,
        next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
    ) -> Result<DnsResponse, DnsError> {
        let response = next.run(ctx, req).await?;
        let updates = updates(ctx, &response);
        if !updates.is_empty() {
            let debug = ctx.cfg().nftset_debug;
            tokio::spawn(async move {
                for update in updates {
                    if debug {
                        crate::log::info!("network set update: {:?}", update);
                    }
                    let result: anyhow::Result<()> = match &update {
                        SetUpdate::IpSet(set, ip, timeout) => {
                            crate::infra::kernel_ipset::add(&set.0, *ip, *timeout)
                                .await
                                .map_err(Into::into)
                        }
                        SetUpdate::NftSet(set, ip, timeout) => add_nftset(set, *ip, *timeout).await,
                    };
                    if let Err(error) = result {
                        crate::log::warn!("network set {:?}: {}", update, error);
                    }
                }
            });
        }
        Ok(response)
    }
}

async fn add_nftset(set: &NFTsetConfig, ip: IpAddr, timeout: u32) -> anyhow::Result<()> {
    crate::infra::kernel_nftset::add(&set.family, &set.table, &set.name, ip, timeout).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerOpts;
    use crate::dns_conf::RuntimeConfig;
    use crate::libdns::proto::op::Query;
    use std::sync::Arc;

    #[test]
    fn test_all_network_set_configuration_sources_enable_middleware() {
        let plain = RuntimeConfig::builder().build().unwrap();
        assert!(!DnsNftsetMiddleware::is_configured(&plain));
        for line in [
            "ipset /example/route4",
            "nftset /example/#4:inet#fw4#route4",
            "domain-rules /example/ -ipset route4",
            "domain-rules /example/ -nftset #4:inet#fw4#route4",
            "bind 127.0.0.1:5300 -ipset route4",
            "client-rules 192.0.2.0/24 -nftset #4:inet#fw4#route4",
            "ipset-no-speed route4",
            "nftset-no-speed #4:inet#fw4#route4",
        ] {
            let cfg = RuntimeConfig::builder().with(line).build().unwrap();
            assert!(DnsNftsetMiddleware::is_configured(&cfg), "{line}");
        }
    }

    #[cfg(all(feature = "nft", target_os = "linux"))]
    #[tokio::test]
    #[ignore = "requires root, nft and ipset"]
    async fn test_kernel_sets_accept_addresses_timeouts_and_report_errors() {
        use std::process::Command;
        fn command(program: &str, args: &[&str]) -> String {
            let result = Command::new(program).args(args).output().unwrap();
            assert!(
                result.status.success(),
                "{} {:?}: {}",
                program,
                args,
                String::from_utf8_lossy(&result.stderr)
            );
            String::from_utf8(result.stdout).unwrap()
        }
        let name = format!("smartdns_test_{}", std::process::id());
        command(
            "ipset",
            &[
                "create", &name, "hash:ip", "family", "inet", "timeout", "120",
            ],
        );
        command("nft", &["add", "table", "inet", &name]);
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = Command::new("ipset").args(["destroy", &self.0]).output();
                let _ = Command::new("nft")
                    .args(["delete", "table", "inet", &self.0])
                    .output();
            }
        }
        let _cleanup = Cleanup(name.clone());
        command(
            "nft",
            &[
                "add",
                "set",
                "inet",
                &name,
                "v4",
                "{ type ipv4_addr; flags timeout; }",
            ],
        );
        command(
            "nft",
            &[
                "add",
                "set",
                "inet",
                &name,
                "v6",
                "{ type ipv6_addr; flags timeout,interval; }",
            ],
        );
        crate::infra::kernel_ipset::add(&name, "192.0.2.4".parse().unwrap(), 90)
            .await
            .unwrap();
        let ipset = command("ipset", &["list", &name]);
        assert!(ipset.contains("192.0.2.4 timeout"), "{ipset}");
        for (set_name, ip) in [("v4", "192.0.2.4"), ("v6", "2001:db8::ff")] {
            let set = NFTsetConfig {
                family: "inet".into(),
                table: name.clone(),
                name: set_name.into(),
            };
            add_nftset(&set, ip.parse().unwrap(), 90).await.unwrap();
            let data = command("nft", &["list", "set", "inet", &name, set_name]);
            assert!(data.contains(ip) && data.contains("expires"), "{data}");
        }
        assert!(
            crate::infra::kernel_ipset::add("smartdns_absent", "192.0.2.4".parse().unwrap(), 90)
                .await
                .is_err()
        );
        let absent = NFTsetConfig {
            family: "inet".into(),
            table: name,
            name: "absent".into(),
        };
        assert!(
            add_nftset(&absent, "192.0.2.4".parse().unwrap(), 90)
                .await
                .is_err()
        );
    }

    #[test]
    fn test_set_rule_precedence_timeout_and_probe_fallback() {
        let cfg = RuntimeConfig::builder()
            .with("nftset-timeout yes")
            .with("ipset-timeout yes")
            .with("nftset-no-speed #4:inet#fw4#failed4,#6:inet#fw4#failed6")
            .with("ipset /selected.example/#4:route4,#6:-")
            .with("nftset /selected.example/#4:inet#fw4#route4")
            .build()
            .unwrap();
        let query = Query::query("selected.example.".parse().unwrap(), RecordType::A);
        let mut ctx = DnsContext::new(query.name(), Arc::new(cfg), ServerOpts::default());
        let ip = "192.0.2.1".parse::<IpAddr>().unwrap();
        let mut response = DnsResponse::new_with_max_ttl(
            query,
            [Record::from_rdata(
                "selected.example.".parse().unwrap(),
                30,
                ip.into(),
            )],
        )
        .with_probe_result(ProbeResult::Failed);
        let result = updates(&ctx, &response);
        assert_eq!(
            result[0],
            SetUpdate::IpSet(KernelIpSet("route4".into()), ip, 90)
        );
        assert_eq!(
            result[1],
            SetUpdate::NftSet(
                NFTsetConfig {
                    family: "inet".into(),
                    table: "fw4".into(),
                    name: "route4".into()
                },
                ip,
                90
            )
        );
        ctx.domain_rule = None;
        response.queries_mut()[0].set_name("unmatched.example.".parse().unwrap());
        assert!(
            matches!(&updates(&ctx, &response)[0], SetUpdate::NftSet(set, _, 90) if set.name == "failed4")
        );
        assert!(
            updates(
                &ctx,
                &response.clone().with_probe_result(ProbeResult::Unchecked)
            )
            .is_empty()
        );
        assert!(
            updates(
                &ctx,
                &response
                    .clone()
                    .with_probe_result(ProbeResult::Measured(std::time::Duration::from_millis(1)))
            )
            .is_empty()
        );
        ctx.server_opts.no_rule_ipset = Some(true);
        assert!(updates(&ctx, &response).is_empty());
    }

    #[test]
    fn test_set_family_inheritance_and_rule_group_scope() {
        let cfg = RuntimeConfig::builder()
            .with("nftset /example/#6:inet#fw4#global6")
            .with("nftset /child.example/#4:inet#fw4#global4")
            .with("group-begin work")
            .with("nftset /example/#4:inet#fw4#work4")
            .with("group-end")
            .build()
            .unwrap();
        let cfg = Arc::new(cfg);
        let query = Query::query("child.example.".parse().unwrap(), RecordType::ANY);
        let response = DnsResponse::new_with_max_ttl(
            query.clone(),
            [
                Record::from_rdata(
                    query.name().clone(),
                    30,
                    RData::A("192.0.2.4".parse().unwrap()),
                ),
                Record::from_rdata(
                    query.name().clone(),
                    30,
                    RData::AAAA("2001:db8::4".parse().unwrap()),
                ),
            ],
        );
        for (group, expected4) in [(None, "global4"), (Some("work".into()), "work4")] {
            let ctx = DnsContext::new(
                query.name(),
                cfg.clone(),
                ServerOpts {
                    rule_group: group,
                    ..Default::default()
                },
            );
            let updates = updates(&ctx, &response);
            assert!(matches!(&updates[0], SetUpdate::NftSet(set, _, _) if set.name == expected4));
            assert!(matches!(&updates[1], SetUpdate::NftSet(set, _, _) if set.name == "global6"));
        }
    }
}
