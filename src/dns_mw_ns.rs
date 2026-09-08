use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;
use std::{borrow::Borrow, net::IpAddr, time::Duration};

use crate::dns_client::{LookupOptions, NameServer};

use crate::infra::ipset::{IpMap, IpSet};
use crate::infra::ping::{PingError, PingOutput};
use crate::{
    config::{ResponseMode, SpeedCheckMode, SpeedCheckModeList},
    dns::*,
    dns_client::{DnsClient, GenericResolver, NameServerGroup},
    dns_error::LookupError,
    log::{debug, error},
    middleware::*,
};

use crate::libdns::proto::rr::domain::usage::LOCAL;
use crate::libdns::proto::{AuthorityData, op::ResponseCode, rr::rdata::opt::EdnsCode};
use futures::{FutureExt, future::BoxFuture};
use rr::rdata::opt::EdnsOption;
use tokio::time::sleep;

pub struct NameServerMiddleware {
    client: DnsClient,
}

impl NameServerMiddleware {
    pub fn new(client: DnsClient) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for NameServerMiddleware {
    #[inline]
    async fn handle(
        &self,
        ctx: &mut DnsContext,
        req: &DnsRequest,
        _next: crate::middleware::Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
    ) -> Result<DnsResponse, DnsError> {
        let name: &Name = req.query().name().borrow();
        let rtype = req.query().query_type();

        let client = &self.client;

        if rtype.is_ip_addr()
            && let Some(lookup) = client.lookup_nameserver(name.clone(), rtype).await
        {
            debug!(
                "lookup nameserver {} {} ip {:?}",
                name,
                rtype,
                lookup
                    .answers()
                    .iter()
                    .filter_map(|record| record.data().ip_addr())
                    .collect::<Vec<_>>()
            );
            ctx.no_cache = true;
            return Ok(lookup);
        }

        let lookup_options = LookupOptions {
            is_dnssec: req.is_dnssec(),
            record_type: rtype,
            client_subnet: req
                .extensions()
                .as_ref()
                .and_then(|edns| {
                    edns.option(EdnsCode::Subnet).and_then(|opt| match opt {
                        EdnsOption::Subnet(subnet) => Some(*subnet),
                        _ => None,
                    })
                })
                .or_else(|| ctx.domain_rule.get_ref(|r| r.subnet.as_ref()).cloned()),
        };

        // skip nameserver rule
        if ctx.server_opts.no_rule_nameserver() {
            return client.lookup(name.clone(), lookup_options).await;
        }

        let group_name = ctx.server_group_name().to_string();

        let name_server = {
            let name_server = if ctx.cfg().mdns_lookup() && LOCAL.zone_of(name) {
                client.get_server_group("mdns").await
            } else {
                None
            };

            let name_server = match name_server {
                Some(ns) => Some(ns),
                None => client.get_server_group(group_name.as_ref()).await,
            };

            match name_server {
                Some(ns) => ns,
                None => {
                    error!("no available nameserver found for {}", name);
                    return Err(ProtoErrorKind::NoConnections.into());
                }
            }
        };

        debug!(
            "query name: {} type: {}{} via [Group: {}]",
            name,
            rtype,
            match lookup_options.client_subnet.as_ref() {
                Some(subnet) => format!("\tsubnet: {}/{}", subnet.addr(), subnet.scope_prefix()),
                None => String::with_capacity(0),
            },
            group_name
        );

        ctx.source = LookupFrom::Server(group_name.to_string());

        if rtype.is_ip_addr() {
            let cfg = ctx.cfg();

            let mut opts = match ctx.domain_rule.as_ref() {
                Some(rule) => LookupIpOptions {
                    response_strategy: rule
                        .get(|n| n.response_mode)
                        .unwrap_or_else(|| cfg.response_mode()),
                    speed_check_mode: match rule.speed_check_mode.as_ref() {
                        Some(mode) => Some(mode.clone()),
                        None => cfg.speed_check_mode().cloned(),
                    },
                    no_speed_check: ctx.server_opts.no_speed_check(),
                    ignore_ip: cfg.ignore_ip().clone(),
                    blacklist_ip: cfg.blacklist_ip().clone(),
                    whitelist_ip: cfg.whitelist_ip().clone(),
                    ip_alias: cfg.ip_alias().clone(),
                    lookup_options,
                },
                None => LookupIpOptions {
                    response_strategy: cfg.response_mode(),
                    speed_check_mode: cfg.speed_check_mode().cloned(),
                    no_speed_check: ctx.server_opts.no_speed_check(),
                    ignore_ip: cfg.ignore_ip().clone(),
                    blacklist_ip: cfg.blacklist_ip().clone(),
                    whitelist_ip: cfg.whitelist_ip().clone(),
                    ip_alias: cfg.ip_alias().clone(),
                    lookup_options,
                },
            };

            if ctx.server_opts.is_background {
                opts.response_strategy = ResponseMode::FastestIp;
            }

            lookup_ip(name_server.deref(), name.clone(), &opts).await
        } else {
            name_server.lookup(name.clone(), lookup_options).await
        }
        .map(|res| res.with_name_server_group(group_name.to_string()))
    }
}

struct LookupIpOptions {
    response_strategy: ResponseMode,
    speed_check_mode: Option<SpeedCheckModeList>,
    no_speed_check: bool,
    ignore_ip: Arc<IpSet>,
    whitelist_ip: Arc<IpSet>,
    blacklist_ip: Arc<IpSet>,
    ip_alias: Arc<IpMap<Arc<[IpAddr]>>>,
    lookup_options: LookupOptions,
}

impl Deref for LookupIpOptions {
    type Target = LookupOptions;

    fn deref(&self) -> &Self::Target {
        &self.lookup_options
    }
}

impl From<LookupIpOptions> for LookupOptions {
    fn from(value: LookupIpOptions) -> Self {
        value.lookup_options
    }
}

impl From<&LookupIpOptions> for LookupOptions {
    fn from(value: &LookupIpOptions) -> Self {
        value.lookup_options.clone()
    }
}

async fn lookup_ip(
    server: &NameServerGroup,
    name: Name,
    options: &LookupIpOptions,
) -> Result<DnsResponse, LookupError> {
    use ResponseMode::*;

    assert!(options.record_type.is_ip_addr());

    let query_tasks = server
        .iter()
        .map(|ns| per_nameserver_lookup_ip(ns, name.clone(), options).boxed())
        .collect::<Vec<_>>();

    if query_tasks.is_empty() {
        return Err(ProtoErrorKind::NoConnections.into());
    }

    // ignore speed check
    let mut response_strategy = if options.no_speed_check || options.speed_check_mode.is_none() {
        FastestResponse
    } else {
        options.response_strategy
    };

    let mut speed_check_mode = options
        .speed_check_mode
        .as_ref()
        .map(|m| m.as_slice())
        .unwrap_or_default();

    if speed_check_mode.iter().any(|m| m.is_none()) {
        response_strategy = FastestResponse; // ignore speed check
        speed_check_mode = &[];
    }

    select_ip_response(name, response_strategy, speed_check_mode, query_tasks).await
}

// Once an upstream has answered, a slow peer must not hold the response until its
// full network timeout. Keep a short window for competing answers, including a
// positive answer following NODATA. Probe timeouts remain independent.
fn limit_remaining_queries<'a>(
    tasks: &mut Vec<BoxFuture<'a, Result<DnsResponse, LookupError>>>,
    received_response: &mut bool,
    response: &Result<DnsResponse, LookupError>,
) {
    let valid_response = match response {
        Ok(response) => matches!(
            response.response_code(),
            ResponseCode::NoError | ResponseCode::NXDomain
        ),
        Err(error) => error.is_no_records_found() || error.is_nx_domain(),
    };
    if *received_response || !valid_response {
        return;
    }
    *received_response = true;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
    *tasks = std::mem::take(tasks)
        .into_iter()
        .map(|task| {
            async move {
                tokio::time::timeout_at(deadline, task)
                    .await
                    .unwrap_or_else(|_| Err(ProtoErrorKind::Timeout.into()))
            }
            .boxed()
        })
        .collect();
}

async fn select_ip_response(
    name: Name,
    response_strategy: ResponseMode,
    speed_check_mode: &[SpeedCheckMode],
    mut query_tasks: Vec<BoxFuture<'_, Result<DnsResponse, LookupError>>>,
) -> Result<DnsResponse, LookupError> {
    use ResponseMode::*;
    use futures_util::future::{Either, select, select_all};

    let mut received_response = false;
    let mut ok_tasks = vec![];
    let mut err_tasks = vec![];

    let selected_ip = match response_strategy {
        FirstPing => {
            let mut ping_tasks = vec![];
            let mut fastest_ip = None;
            loop {
                let (ping_res, query_res) = match (query_tasks.len(), ping_tasks.len()) {
                    (0, 0) => break,
                    (0, _) => {
                        let (fastest_ip, _, rest) = select_all(ping_tasks).await;
                        ping_tasks = rest;
                        (fastest_ip, None)
                    }
                    (_, 0) => {
                        let (res, _idx, rest) = select_all(query_tasks).await;
                        query_tasks = rest;
                        (None, Some(res))
                    }
                    _ => {
                        let a = select_all(ping_tasks);
                        let b = select_all(query_tasks);
                        let c = select(a, b).await;
                        match c {
                            Either::Left(((fastest_ip, _, rest), other)) => {
                                ping_tasks = rest;
                                query_tasks = other.into_inner();
                                (fastest_ip, None)
                            }
                            Either::Right(((res, _, rest), other)) => {
                                query_tasks = rest;
                                ping_tasks = other.into_inner();
                                (None, Some(res))
                            }
                        }
                    }
                };

                if let Some(ip) = ping_res {
                    fastest_ip = Some(ip);
                    break;
                }

                if let Some(res) = &query_res {
                    limit_remaining_queries(&mut query_tasks, &mut received_response, res);
                }
                match query_res {
                    Some(v) => match v {
                        Ok(lookup) => {
                            let ip_addrs = lookup.ip_addrs();
                            if ip_addrs.len() == 1 {
                                return Ok(lookup);
                            }
                            ok_tasks.push(lookup);
                            if !ip_addrs.is_empty() {
                                ping_tasks.push(
                                    multi_mode_ping_fastest(
                                        name.clone(),
                                        ip_addrs,
                                        speed_check_mode.to_vec(),
                                    )
                                    .boxed(),
                                );
                            }
                        }
                        Err(err) => {
                            err_tasks.push(err);
                        }
                    },
                    None => continue,
                }
            }

            match fastest_ip {
                Some(ip) => Some(ip),
                None => {
                    let ip_addr_stats = ok_tasks.iter().flat_map(|r| r.ip_addrs()).fold(
                        HashMap::<IpAddr, usize>::new(),
                        |mut map, ip| {
                            map.entry(ip).and_modify(|n| *n += 1).or_insert(1);
                            map
                        },
                    );
                    ip_addr_stats
                        .into_iter()
                        .max_by_key(|(_, n)| *n)
                        .map(|(ip, _)| ip)
                }
            }
        }
        FastestIp => {
            let mut ping_tasks = vec![];

            let mut ip_addr_stats = HashMap::new();

            let mut fastest_ip: Option<PingOutput> = None;

            loop {
                #[allow(clippy::type_complexity)]
                let (ping_res, query_res): (
                    Option<Result<PingOutput, PingError>>,
                    Option<Result<DnsResponse, DnsError>>,
                ) = match (query_tasks.len(), ping_tasks.len()) {
                    (0, 0) => break,
                    (0, _) => {
                        let (res, _idx, rest) = select_all(ping_tasks).await;
                        ping_tasks = rest;
                        (Some(res), None)
                    }
                    (_, 0) => {
                        let (res, _idx, rest) = select_all(query_tasks).await;
                        query_tasks = rest;
                        (None, Some(res))
                    }
                    _ => {
                        let a = select_all(ping_tasks);
                        let b = select_all(query_tasks);
                        let c = select(a, b).await;
                        match c {
                            Either::Left(((res, _, rest), other)) => {
                                ping_tasks = rest;
                                query_tasks = other.into_inner();
                                (Some(res), None)
                            }
                            Either::Right(((res, _, rest), other)) => {
                                query_tasks = rest;
                                ping_tasks = other.into_inner();
                                (None, Some(res))
                            }
                        }
                    }
                };

                if let Some(Ok(out)) = ping_res
                    && match fastest_ip.as_ref() {
                        Some(t) => out.elapsed() < t.elapsed(),
                        None => true,
                    }
                {
                    fastest_ip = Some(out);
                }

                if let Some(res) = query_res {
                    limit_remaining_queries(&mut query_tasks, &mut received_response, &res);
                    match res {
                        Ok(lookup) => {
                            let ip_addrs = lookup.ip_addrs();

                            for ip_addr in &ip_addrs {
                                *ip_addr_stats.entry(*ip_addr).or_insert_with(|| {
                                    ping_tasks.push(
                                        multi_mode_ping(
                                            name.clone(),
                                            *ip_addr,
                                            speed_check_mode.to_vec(),
                                        )
                                        .boxed(),
                                    );
                                    0u8
                                }) += 1;
                            }
                            ok_tasks.push(lookup);
                        }
                        Err(err) => {
                            err_tasks.push(err);
                        }
                    }
                }
            }

            match fastest_ip {
                Some(fastest_ip) => Some(fastest_ip.dest().ip_addr()),
                None => ip_addr_stats
                    .into_iter()
                    .max_by_key(|(_, n)| *n)
                    .map(|(ip, _)| ip),
            }
        }
        FastestResponse => {
            let mut last_error = None;
            loop {
                let (res, _idx, rest) = select_all(query_tasks).await;
                if let Ok(response) = &res
                    && response.response_code() == ResponseCode::NoError
                {
                    return res;
                }

                if rest.is_empty() {
                    return res;
                }

                if let Err(err) = res {
                    if matches!(last_error, Some(e) if e == err) {
                        return Err(err);
                    } else {
                        last_error = Some(err);
                    }
                }
                query_tasks = rest;
            }
        }
    };

    if let Some(selected_ip) = selected_ip {
        for mut res in ok_tasks {
            let record = res
                .take_answers()
                .into_iter()
                .find(|r| matches!(r.data().ip_addr(), Some(ip) if ip == selected_ip));
            if let Some(record) = record {
                res.add_answer(record);
                return Ok(res);
            }
        }
        unreachable!()
    }

    match ok_tasks.into_iter().next() {
        Some(lookup) => Ok(lookup),
        None => match err_tasks.into_iter().next() {
            Some(err) => Err(err),
            None => unreachable!(),
        },
    }
}

async fn multi_mode_ping_fastest(
    name: Name,
    ip_addrs: Vec<IpAddr>,
    modes: Vec<SpeedCheckMode>,
) -> Option<IpAddr> {
    use crate::infra::ping::{PingOptions, ping_fastest};
    let duration = Duration::from_millis(200);
    let ping_ops = PingOptions::default().with_timeout_secs(2);

    let mut fastest_ip = None;

    for mode in &modes {
        debug!("Speed test {} {:?} ping {:?}", name, mode, ip_addrs);
        let dests = mode.to_ping_addrs(&ip_addrs);

        let ping_task = ping_fastest(dests, ping_ops).boxed();
        let timeout_task = sleep(duration).boxed();
        match futures_util::future::select(ping_task, timeout_task).await {
            futures::future::Either::Left((ping_res, _)) => {
                match ping_res {
                    Ok(ping_out) => {
                        // ping success
                        let ip = ping_out.dest().ip_addr();
                        debug!(
                            "The fastest ip of {} is {}, delay: {:?}",
                            name,
                            ip,
                            ping_out.elapsed()
                        );
                        fastest_ip = Some(ip);
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

async fn multi_mode_ping(
    name: Name,
    ip_addr: IpAddr,
    modes: Vec<SpeedCheckMode>,
) -> Result<PingOutput, PingError> {
    use crate::infra::ping::{PingOptions, ping};
    let duration = Duration::from_millis(200);
    let ping_ops = PingOptions::default().with_timeout_secs(2);

    for mode in &modes {
        let dest = match mode.to_ping_addr(ip_addr) {
            Some(addr) => addr,
            None => return Err(PingError::NoAddress),
        };

        let ping_task = ping(dest, ping_ops).boxed();
        let timeout_task = sleep(duration).boxed();
        match futures_util::future::select(ping_task, timeout_task).await {
            futures::future::Either::Left((ping_res, _)) => match ping_res {
                Ok(ping_out) => {
                    debug!(
                        "Speed test {} {:?} ping {:?} elapsed {:?}",
                        name,
                        mode,
                        ip_addr,
                        ping_out.elapsed()
                    );
                    return Ok(ping_out);
                }
                Err(_) => continue,
            },
            futures::future::Either::Right((_, _)) => {
                // timeout
                continue;
            }
        }
    }

    Err(PingError::Timeout)
}

async fn per_nameserver_lookup_ip(
    server: &NameServer,
    name: Name,
    options: &LookupIpOptions,
) -> Result<DnsResponse, LookupError> {
    assert!(options.lookup_options.record_type.is_ip_addr());

    let res = server.lookup(name.clone(), options).await;

    let ns_opts = server.options();
    let whitelist_on = ns_opts.whitelist_ip;
    let blacklist_on = ns_opts.blacklist_ip;

    let LookupIpOptions {
        whitelist_ip,
        blacklist_ip,
        ip_alias,
        ignore_ip,
        ..
    } = options;

    if !whitelist_on && !blacklist_on && ignore_ip.is_empty() && ip_alias.is_empty() {
        return res;
    }

    let ip_filter = |ip: &IpAddr| {
        // whitelist
        if whitelist_on && whitelist_ip.contains(ip) {
            return true;
        }

        if blacklist_on && blacklist_ip.contains(ip) {
            return false;
        }

        !ignore_ip.contains(ip)
    };

    match res {
        Ok(mut lookup) => {
            let query = lookup.query().clone();

            let answers = lookup.take_answers();
            let answers = {
                let mut new_ans = Vec::new();
                let mut alias_set = Vec::new(); // dedup
                for record in answers {
                    let Some(ip) = record.data().ip_addr().filter(ip_filter) else {
                        continue;
                    };
                    match ip_alias.get(&ip) {
                        None => new_ans.push(record),
                        Some(ip) if !alias_set.contains(&ip.as_ptr()) => {
                            alias_set.push(ip.as_ptr());
                            new_ans.extend(ip.iter().filter_map(|&ip| {
                                let mut record = record.clone();
                                record.set_data(ip.into());
                                match (options.record_type, ip) {
                                    (RecordType::A, IpAddr::V4(_))
                                    | (RecordType::AAAA, IpAddr::V6(_)) => Some(record),
                                    _ => {
                                        lookup.add_additional(record);
                                        None
                                    }
                                }
                            }));
                        }
                        Some(_) => continue,
                    }
                }
                new_ans
            };

            if answers.is_empty() {
                let no_records = AuthorityData::new(Box::new(query), None, true, true, None);
                return Err(ProtoErrorKind::NoRecordsFound(no_records.into()).into());
            }

            *lookup.answers_mut() = answers;
            lookup.set_valid_until_max();

            Ok(lookup)
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use crate::libdns::proto::op::Query;
    use crate::libdns::proto::rr::rdata::opt::ClientSubnet;

    use super::*;
    use crate::{dns_conf::RuntimeConfig, third_ext::FutureJoinAllExt};

    fn speed_test_response(ips: &[&str], record_type: RecordType) -> DnsResponse {
        let name: Name = "speed-test.example.".parse().unwrap();
        let records: Vec<_> = ips
            .iter()
            .map(|ip| {
                Record::from_rdata(name.clone(), 60, RData::from(ip.parse::<IpAddr>().unwrap()))
            })
            .collect();
        DnsResponse::new_with_max_ttl(Query::query(name, record_type), records)
    }

    #[tokio::test]
    async fn test_speed_response_nodata_does_not_wait_for_silent_upstream() {
        for mode in [ResponseMode::FirstPing, ResponseMode::FastestIp] {
            let response = speed_test_response(&[], RecordType::AAAA);
            let name = response.query().name().clone();
            let tasks = vec![
                async { Ok(response) }.boxed(),
                futures::future::pending().boxed(),
            ];
            let response = tokio::time::timeout(
                Duration::from_secs(1),
                select_ip_response(name, mode, &[SpeedCheckMode::Ping], tasks),
            )
            .await
            .expect("NODATA must not wait for a silent upstream")
            .unwrap();
            assert_eq!(response.response_code(), ResponseCode::NoError);
            assert!(response.records().is_empty());
            assert_eq!(response.query().query_type(), RecordType::AAAA);
        }
    }

    #[tokio::test]
    async fn test_speed_response_negative_error_is_preserved() {
        for mode in [ResponseMode::FirstPing, ResponseMode::FastestIp] {
            let query = Query::query("missing.example.".parse().unwrap(), RecordType::AAAA);
            let name = query.name().clone();
            let authority = AuthorityData::new(Box::new(query), None, true, true, None);
            let tasks = vec![
                async { Err(ProtoErrorKind::NoRecordsFound(authority.into()).into()) }.boxed(),
                futures::future::pending().boxed(),
            ];
            let error = tokio::time::timeout(
                Duration::from_secs(1),
                select_ip_response(name, mode, &[SpeedCheckMode::Ping], tasks),
            )
            .await
            .expect("a negative DNS answer must bound the remaining wait")
            .unwrap_err();
            assert!(error.is_nx_domain());
        }
    }

    #[tokio::test]
    async fn test_speed_response_accepts_positive_after_nodata() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let modes = [SpeedCheckMode::Tcp(listener.local_addr().unwrap().port())];
        for mode in [ResponseMode::FirstPing, ResponseMode::FastestIp] {
            let empty = speed_test_response(&[], RecordType::A);
            let name = empty.query().name().clone();
            let tasks = vec![
                async { Ok(empty) }.boxed(),
                async {
                    sleep(Duration::from_millis(30)).await;
                    Ok(speed_test_response(&["127.0.0.1"], RecordType::A))
                }
                .boxed(),
                futures::future::pending().boxed(),
            ];
            let response = tokio::time::timeout(
                Duration::from_secs(1),
                select_ip_response(name, mode, &modes, tasks),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                response.ip_addrs(),
                vec!["127.0.0.1".parse::<IpAddr>().unwrap()]
            );
        }
    }

    #[tokio::test]
    async fn test_speed_response_failed_probes_keep_answer_without_waiting_for_upstream() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let modes = [SpeedCheckMode::Tcp(listener.local_addr().unwrap().port())];
        for mode in [ResponseMode::FirstPing, ResponseMode::FastestIp] {
            let answer = speed_test_response(&["127.0.0.2", "127.0.0.3"], RecordType::A);
            let name = answer.query().name().clone();
            let tasks = vec![
                async { Ok(answer) }.boxed(),
                futures::future::pending().boxed(),
            ];
            let response = tokio::time::timeout(
                Duration::from_secs(1),
                select_ip_response(name, mode, &modes, tasks),
            )
            .await
            .expect("failed probes must not expose the full upstream timeout")
            .unwrap();
            let ips = response.ip_addrs();
            assert_eq!(ips.len(), 1);
            assert!(
                [
                    "127.0.0.2".parse::<IpAddr>().unwrap(),
                    "127.0.0.3".parse().unwrap()
                ]
                .contains(&ips[0])
            );
        }
    }

    #[tokio::test]
    async fn test_speed_response_first_ping_continues_after_failed_probe_group() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let modes = [SpeedCheckMode::Tcp(listener.local_addr().unwrap().port())];
        let answer = speed_test_response(&["127.0.0.2", "127.0.0.3"], RecordType::A);
        let name = answer.query().name().clone();
        let tasks = vec![
            async { Ok(answer) }.boxed(),
            async {
                sleep(Duration::from_millis(30)).await;
                Ok(speed_test_response(&["127.0.0.1"], RecordType::A))
            }
            .boxed(),
        ];
        let response = select_ip_response(name, ResponseMode::FirstPing, &modes, tasks)
            .await
            .unwrap();
        assert_eq!(
            response.ip_addrs(),
            vec!["127.0.0.1".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn test_edns_client_subnet() {
        async fn inner_test(i: usize) -> bool {
            // https://lite.ip2location.com/ip-address-ranges-by-country

            let servers = [
                "server https://120.53.53.53/dns-query",
                "server https://223.5.5.5/dns-query",
            ];

            let server = servers[i % servers.len()];

            let cfg = RuntimeConfig::builder().with(server).build().unwrap();

            let domain = "www.bing.com";

            let client = cfg.create_dns_client().await;

            let subnets = ["113.65.29.0/24", "103.225.87.0/24", "113.65.29.0/24"];

            let results = subnets
                .into_iter()
                .map(|subnet| {
                    client.lookup(
                        domain,
                        LookupOptions {
                            is_dnssec: false,
                            record_type: RecordType::A,
                            client_subnet: Some(ClientSubnet::from_str(subnet).unwrap()),
                        },
                    )
                })
                .join_all()
                .await
                .into_iter()
                .flatten()
                .map(|lookup| {
                    let mut ips = lookup.ip_addrs();
                    ips.sort();
                    ips
                })
                .collect::<Vec<_>>();

            let t1 = results[0].clone();
            let t2 = results[1].clone();
            let t3 = results[2].clone();
            let success = t1 == t3 && t1 != t2;
            if !success {
                println!("{t1:?}");
                println!("{t2:?}");
                println!("{t3:?}");
            }
            success
        }

        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                use futures_util::future::select_all;
                let mut success = false;
                let mut tasks = (0..10).map(|i| inner_test(i).boxed()).collect::<Vec<_>>();

                loop {
                    let (res, _idx, rest) = select_all(tasks).await;

                    if res {
                        success = res;
                        break;
                    }

                    if rest.is_empty() {
                        break;
                    }

                    tasks = rest;
                }
                assert!(success);
            });
    }
}
