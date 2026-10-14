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
    dns_conf::RuntimeConfig,
    dns_error::LookupError,
    log::{debug, error},
    middleware::*,
};

use crate::libdns::proto::rr::domain::usage::LOCAL;
use crate::libdns::proto::{AuthorityData, op::ResponseCode, rr::rdata::opt::EdnsCode};
use futures::{
    FutureExt,
    future::{BoxFuture, Either, select},
};
use rr::rdata::opt::EdnsOption;
use tokio::sync::{Semaphore, oneshot};
use tokio::time::sleep;

/// Lookups that may go on probing after their first answer at once. Beyond
/// this, as under a flood of unique names, lookups answer as plain
/// `fastest-response` and their entries stay unprobed until prefetch or expiry
/// refreshes them.
const MAX_DEFERRED_SPEED_CHECKS: usize = 128;

pub struct NameServerMiddleware {
    client: DnsClient,
    deferred_speed_checks: Arc<Semaphore>,
}

impl NameServerMiddleware {
    pub fn new(client: DnsClient) -> Self {
        Self {
            client,
            deferred_speed_checks: Arc::new(Semaphore::new(MAX_DEFERRED_SPEED_CHECKS)),
        }
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
            && let Some(lookup) = client.lookup_nameserver(name, rtype).await
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
            if let Some(speed_check_mode) = deferred_speed_check_mode(ctx, req)
                && let Ok(permit) = self.deferred_speed_checks.clone().try_acquire_owned()
            {
                let (first, rest) = lookup_ip_answering_first(
                    name_server,
                    name.clone(),
                    ctx.cfg().clone(),
                    speed_check_mode.clone(),
                    lookup_options,
                )
                .await;
                if let Some(rest) = rest {
                    let group_name = group_name.clone();
                    ctx.pending_speed_check.set(
                        async move {
                            let _permit = permit;
                            rest.await.map(|res| res.with_name_server_group(group_name))
                        }
                        .boxed(),
                    );
                }
                return first.map(|res| res.with_name_server_group(group_name));
            }

            let cfg = ctx.cfg();

            let mut opts = LookupIpOptions {
                response_strategy: ctx
                    .domain_rule
                    .get(|rule| rule.response_mode)
                    .unwrap_or_else(|| cfg.response_mode()),
                speed_check_mode: ctx
                    .domain_rule
                    .as_ref()
                    .and_then(|rule| rule.speed_check_mode.as_ref())
                    .or_else(|| cfg.speed_check_mode()),
                no_speed_check: ctx.server_opts.no_speed_check(),
                ignore_ip: cfg.ignore_ip(),
                blacklist_ip: cfg.blacklist_ip(),
                whitelist_ip: cfg.whitelist_ip(),
                ip_alias: cfg.ip_alias(),
                lookup_options,
            };

            if ctx.server_opts.is_background {
                opts.response_strategy = ResponseMode::FastestIp;
            }

            lookup_ip(name_server.deref(), name.clone(), &opts).await
        } else {
            name_server.lookup(name.clone(), lookup_options).await
        }
        .map(|res| res.with_name_server_group(group_name))
    }
}

/// The probe modes of a `fastest-response` lookup that should keep its speed
/// check running after the first answer, as C SmartDNS does: the client gets
/// the first upstream answer, and the cache keeps the fastest address once the
/// probes finish (see `PendingSpeedCheck`). Only lookups whose answer the cache
/// stores qualify. `speed-check-mode none` keeps plain `fastest-response`.
fn deferred_speed_check_mode<'a>(
    ctx: &'a DnsContext,
    req: &DnsRequest,
) -> Option<&'a SpeedCheckModeList> {
    let cfg = ctx.cfg();
    let response_mode = ctx
        .domain_rule
        .get(|rule| rule.response_mode)
        .unwrap_or_else(|| cfg.response_mode());
    if response_mode != ResponseMode::FastestResponse
        || ctx.server_opts.no_speed_check()
        || ctx.server_opts.is_background
        // Dual-stack selection only compares the other family's answer.
        || ctx.is_dualstack
        || cfg.cache_size() == 0
        || ctx.no_cache
        || ctx.server_opts.no_cache()
        || req.is_dnssec()
    {
        return None;
    }

    ctx.domain_rule
        .as_ref()
        .and_then(|rule| rule.speed_check_mode.as_ref())
        .or_else(|| cfg.speed_check_mode())
        .filter(|modes| !modes.is_empty() && !modes.iter().any(|m| m.is_none()))
}

/// Returns the first valid upstream answer, as `fastest-response` does, and
/// the rest of the same lookup, which goes on as `fastest-ip`: it waits for the
/// other upstreams, probes every address and resolves to the answer for the
/// fastest one. No upstream is queried twice.
async fn lookup_ip_answering_first(
    server: Arc<NameServerGroup>,
    name: Name,
    cfg: Arc<RuntimeConfig>,
    speed_check_mode: SpeedCheckModeList,
    lookup_options: LookupOptions,
) -> (
    Result<DnsResponse, LookupError>,
    Option<BoxFuture<'static, Result<DnsResponse, LookupError>>>,
) {
    if server.servers.is_empty() {
        return (Err(ProtoErrorKind::NoConnections.into()), None);
    }

    let (first_tx, mut first_rx) = oneshot::channel();
    let mut lookup = async move {
        let options = LookupIpOptions {
            response_strategy: ResponseMode::FastestIp,
            speed_check_mode: Some(&speed_check_mode),
            no_speed_check: false,
            ignore_ip: cfg.ignore_ip(),
            whitelist_ip: cfg.whitelist_ip(),
            blacklist_ip: cfg.blacklist_ip(),
            ip_alias: cfg.ip_alias(),
            lookup_options,
        };
        let query_tasks = server
            .iter()
            .map(|ns| per_nameserver_lookup_ip(ns, name.clone(), &options).boxed())
            .collect();
        select_ip_response_reporting_first(
            name,
            ResponseMode::FastestIp,
            &speed_check_mode,
            query_tasks,
            Some(first_tx),
        )
        .await
    }
    .boxed();

    // The lookup pauses after reporting its first answer, so no probe starts
    // unless the rest is polled again.
    match select(&mut first_rx, &mut lookup).await {
        Either::Left((Ok(first), _)) => (first, Some(lookup)),
        // Not reached: the lookup reports its first answer before finishing.
        Either::Left((Err(_), _)) => (Err(ProtoErrorKind::NoConnections.into()), None),
        Either::Right((result, _)) => (result, None),
    }
}

struct LookupIpOptions<'a> {
    response_strategy: ResponseMode,
    speed_check_mode: Option<&'a SpeedCheckModeList>,
    no_speed_check: bool,
    ignore_ip: &'a IpSet,
    whitelist_ip: &'a IpSet,
    blacklist_ip: &'a IpSet,
    ip_alias: &'a IpMap<Arc<[IpAddr]>>,
    lookup_options: LookupOptions,
}

impl Deref for LookupIpOptions<'_> {
    type Target = LookupOptions;

    fn deref(&self) -> &Self::Target {
        &self.lookup_options
    }
}

impl From<LookupIpOptions<'_>> for LookupOptions {
    fn from(value: LookupIpOptions<'_>) -> Self {
        value.lookup_options
    }
}

impl From<&LookupIpOptions<'_>> for LookupOptions {
    fn from(value: &LookupIpOptions<'_>) -> Self {
        value.lookup_options.clone()
    }
}

async fn lookup_ip(
    server: &NameServerGroup,
    name: Name,
    options: &LookupIpOptions<'_>,
) -> Result<DnsResponse, LookupError> {
    use ResponseMode::*;

    assert!(options.record_type.is_ip_addr());

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

    if response_strategy == FastestResponse
        && let [server] = server.servers.as_slice()
    {
        return per_nameserver_lookup_ip(server, name, options).await;
    }
    let query_tasks = server
        .iter()
        .map(|ns| per_nameserver_lookup_ip(ns, name.clone(), options).boxed())
        .collect::<Vec<_>>();
    if query_tasks.is_empty() {
        return Err(ProtoErrorKind::NoConnections.into());
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
    if *received_response || !is_valid_response(response) {
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

/// An answer or a NODATA/NXDOMAIN reply, rather than an upstream failure.
fn is_valid_response(response: &Result<DnsResponse, LookupError>) -> bool {
    match response {
        Ok(response) => matches!(
            response.response_code(),
            ResponseCode::NoError | ResponseCode::NXDomain
        ),
        Err(error) => error.is_negative_response(),
    }
}

async fn select_ip_response(
    name: Name,
    response_strategy: ResponseMode,
    speed_check_mode: &[SpeedCheckMode],
    query_tasks: Vec<BoxFuture<'_, Result<DnsResponse, LookupError>>>,
) -> Result<DnsResponse, LookupError> {
    select_ip_response_reporting_first(name, response_strategy, speed_check_mode, query_tasks, None)
        .await
}

/// With `FastestIp`, `first_response` receives the answer `fastest-response`
/// would return, the first valid answer or the last one when none is valid,
/// while the selection goes on and waits for every upstream.
async fn select_ip_response_reporting_first(
    name: Name,
    response_strategy: ResponseMode,
    speed_check_mode: &[SpeedCheckMode],
    mut query_tasks: Vec<BoxFuture<'_, Result<DnsResponse, LookupError>>>,
    mut first_response: Option<oneshot::Sender<Result<DnsResponse, LookupError>>>,
) -> Result<DnsResponse, LookupError> {
    use ResponseMode::*;
    use futures_util::future::select_all;

    let reports_first = first_response.is_some();
    debug_assert!(!reports_first || response_strategy == FastestIp);
    let mut received_response = false;
    let mut ok_tasks = vec![];
    let mut err_tasks = vec![];
    let mut probe_result = None;

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
                Some(out) => {
                    probe_result = Some(out.elapsed());
                    Some(out.dest().ip_addr())
                }
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
                    if reports_first {
                        // The caller gets the first valid answer at once, so
                        // a slower upstream only delays the speed check.
                        received_response |= is_valid_response(&res);
                    } else {
                        limit_remaining_queries(&mut query_tasks, &mut received_response, &res);
                    }
                    if let Some(first) =
                        first_response.take_if(|_| received_response || query_tasks.is_empty())
                    {
                        let _ = first.send(res.clone());
                        // Let the caller take the answer before any probe
                        // starts: it may drop the rest of the selection.
                        tokio::task::yield_now().await;
                    }
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
                Some(fastest_ip) => {
                    probe_result = Some(fastest_ip.elapsed());
                    Some(fastest_ip.dest().ip_addr())
                }
                None => ip_addr_stats
                    .into_iter()
                    .max_by_key(|(_, n)| *n)
                    .map(|(ip, _)| ip),
            }
        }
        FastestResponse => loop {
            let (res, _idx, rest) = select_all(query_tasks).await;
            if is_valid_response(&res) || rest.is_empty() {
                return res;
            }
            query_tasks = rest;
        },
    };

    if let Some(selected_ip) = selected_ip {
        for mut res in ok_tasks {
            let record = res
                .take_answers()
                .into_iter()
                .find(|r| matches!(r.data().ip_addr(), Some(ip) if ip == selected_ip));
            if let Some(mut record) = record {
                // Selection removes the CNAME chain, so the IP must answer the
                // original name on cold lookups as it already does from cache.
                record.set_name(name.clone());
                res.add_answer(record);
                return Ok(res.with_probe_result(
                    probe_result.map_or(ProbeResult::Failed, ProbeResult::Measured),
                ));
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
) -> Option<PingOutput> {
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
                        fastest_ip = Some(ping_out);
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
    options: &LookupIpOptions<'_>,
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
                for mut record in answers {
                    let Some(ip) = record.data().ip_addr().filter(ip_filter) else {
                        continue;
                    };
                    // IP filtering also removes any CNAME records.
                    record.set_name(query.name().clone());
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
    async fn test_speed_response_cname_answers_use_query_name() {
        let name: Name = "alias.example.".parse().unwrap();
        let canonical: Name = "cdn.example.".parse().unwrap();
        for (record_type, ips) in [
            (RecordType::A, ["192.0.2.1", "192.0.2.2"]),
            (RecordType::AAAA, ["2001:db8::1", "2001:db8::2"]),
        ] {
            let ips = ips.map(|ip| ip.parse::<IpAddr>().unwrap());
            for mode in [ResponseMode::FirstPing, ResponseMode::FastestIp] {
                let response = DnsResponse::new_with_max_ttl(
                    Query::query(name.clone(), record_type),
                    vec![
                        Record::from_rdata(
                            name.clone(),
                            60,
                            RData::CNAME(crate::libdns::proto::rr::rdata::CNAME(canonical.clone())),
                        ),
                        Record::from_rdata(canonical.clone(), 60, RData::from(ips[0])),
                        Record::from_rdata(canonical.clone(), 60, RData::from(ips[1])),
                    ],
                );
                let response = select_ip_response(
                    name.clone(),
                    mode,
                    &[],
                    vec![async { Ok(response) }.boxed()],
                )
                .await
                .unwrap();

                assert_eq!(response.query().name(), &name);
                assert_eq!(response.answers().len(), 1);
                assert_eq!(response.answers()[0].name(), &name);
                assert_eq!(response.answers()[0].record_type(), record_type);
                assert!(ips.contains(&response.ip_addrs()[0]));
            }
        }
    }

    #[tokio::test]
    async fn test_ip_filter_cname_answers_use_query_name() {
        use crate::config::NameServerInfo;
        use crate::dns_url::DnsUrl;
        use crate::libdns::proto::op::Message;
        use crate::libdns::proto::rr::rdata::CNAME;

        for (record_type, ip) in [
            (RecordType::A, "192.0.2.1".parse::<IpAddr>().unwrap()),
            (RecordType::AAAA, "2001:db8::1".parse().unwrap()),
        ] {
            let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let url: DnsUrl = format!("udp://{}", socket.local_addr().unwrap())
                .parse()
                .unwrap();
            let mut config = NameServerInfo::from(url);
            config.blacklist_ip = true;
            let server = NameServer::new(config, None, None, None, None).unwrap();
            let upstream = tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let (len, peer) = socket.recv_from(&mut buffer).await.unwrap();
                let request = Message::from_vec(&buffer[..len]).unwrap();
                let name = request.queries()[0].name().clone();
                let canonical: Name = "cdn.example.".parse().unwrap();
                let mut response = request.to_response();
                response.set_recursion_available(true);
                response.add_answer(Record::from_rdata(
                    name,
                    60,
                    RData::CNAME(CNAME(canonical.clone())),
                ));
                response.add_answer(Record::from_rdata(canonical, 60, RData::from(ip)));
                socket
                    .send_to(&response.to_vec().unwrap(), peer)
                    .await
                    .unwrap();
            });
            let name: Name = "alias.example.".parse().unwrap();
            let empty_ips = IpSet::default();
            let empty_aliases = IpMap::default();
            let options = LookupIpOptions {
                response_strategy: ResponseMode::FastestResponse,
                speed_check_mode: None,
                no_speed_check: true,
                ignore_ip: &empty_ips,
                whitelist_ip: &empty_ips,
                blacklist_ip: &empty_ips,
                ip_alias: &empty_aliases,
                lookup_options: LookupOptions {
                    record_type,
                    ..Default::default()
                },
            };
            let response = tokio::time::timeout(
                Duration::from_secs(2),
                per_nameserver_lookup_ip(&server, name.clone(), &options),
            )
            .await
            .unwrap()
            .unwrap();
            upstream.await.unwrap();

            assert_eq!(response.answers().len(), 1);
            assert_eq!(response.answers()[0].name(), &name);
            assert_eq!(response.answers()[0].record_type(), record_type);
            assert_eq!(response.ip_addrs(), vec![ip]);
        }
    }

    #[tokio::test]
    async fn test_query_upstream_failures_do_not_cancel_late_success() {
        for mode in [
            ResponseMode::FastestResponse,
            ResponseMode::FirstPing,
            ResponseMode::FastestIp,
        ] {
            for code in [ResponseCode::ServFail, ResponseCode::Refused] {
                let answer = speed_test_response(&["192.0.2.1"], RecordType::A);
                let query = answer.query().clone();
                let authority =
                    AuthorityData::new(Box::new(query.clone()), None, true, false, None);
                let mut no_records: crate::libdns::proto::NoRecords = authority.into();
                no_records.response_code = code;
                let error: LookupError = ProtoErrorKind::NoRecordsFound(no_records).into();
                let other_error = error.clone();
                let tasks = vec![
                    async { Err(error) }.boxed(),
                    async {
                        sleep(Duration::from_millis(20)).await;
                        Err(other_error)
                    }
                    .boxed(),
                    async {
                        sleep(Duration::from_millis(350)).await;
                        Ok(answer)
                    }
                    .boxed(),
                ];
                let response = tokio::time::timeout(
                    Duration::from_secs(2),
                    select_ip_response(query.name().clone(), mode, &[], tasks),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(response.response_code(), ResponseCode::NoError);
                assert_eq!(response.query(), &query);
                assert_eq!(
                    response.ip_addrs(),
                    vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
                );
            }
        }
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
    fn test_speed_check_is_deferred_only_for_cached_fastest_response() {
        use crate::config::ServerOpts;

        let name: Name = "speed.example.".parse().unwrap();
        let req: DnsRequest = Query::query(name.clone(), RecordType::A).into();
        let deferred = |directives: &[&str], server_opts: ServerOpts, is_dualstack: bool| {
            let cfg = directives
                .iter()
                .fold(RuntimeConfig::builder(), |builder, directive| {
                    builder.with(directive)
                })
                .build()
                .unwrap();
            let mut ctx = DnsContext::new(&name, Arc::new(cfg), server_opts);
            ctx.is_dualstack = is_dualstack;
            deferred_speed_check_mode(&ctx, &req).cloned()
        };
        let fastest_response = [
            "response-mode fastest-response",
            "speed-check-mode ping,tcp:443",
        ];

        assert_eq!(
            deferred(&fastest_response, Default::default(), false)
                .unwrap()
                .as_slice(),
            &[SpeedCheckMode::Ping, SpeedCheckMode::Tcp(443)]
        );
        for directives in [
            &["response-mode fastest-ip", "speed-check-mode ping"][..],
            &["response-mode first-ping", "speed-check-mode ping"],
            &["response-mode fastest-response", "speed-check-mode none"],
            &[
                "response-mode fastest-response",
                "speed-check-mode ping",
                "cache-size 0",
            ],
            &[
                "response-mode fastest-response",
                "speed-check-mode ping",
                "domain-rules /speed.example/ -no-cache",
            ],
        ] {
            assert!(
                deferred(directives, Default::default(), false).is_none(),
                "{directives:?}"
            );
        }
        for server_opts in [
            ServerOpts {
                is_background: true,
                ..Default::default()
            },
            ServerOpts {
                no_speed_check: Some(true),
                ..Default::default()
            },
            ServerOpts {
                no_cache: Some(true),
                ..Default::default()
            },
        ] {
            assert!(deferred(&fastest_response, server_opts, false).is_none());
        }
        assert!(
            deferred(&fastest_response, Default::default(), true).is_none(),
            "dual-stack selection does not cache the other family"
        );
    }

    #[tokio::test]
    async fn test_fastest_ip_reports_the_first_answer_while_probing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let modes = [SpeedCheckMode::Tcp(listener.local_addr().unwrap().port())];
        let first = speed_test_response(&["127.0.0.2"], RecordType::A);
        let name = first.query().name().clone();
        let (release, released) = oneshot::channel::<()>();
        let tasks = vec![
            async { Ok(first) }.boxed(),
            async {
                let _ = released.await;
                Ok(speed_test_response(&["127.0.0.1"], RecordType::A))
            }
            .boxed(),
        ];
        let (first_tx, first_rx) = oneshot::channel();
        let (reported, fastest) = tokio::join!(
            async {
                let reported = tokio::time::timeout(Duration::from_secs(1), first_rx)
                    .await
                    .expect("the first answer must not wait for the other upstreams")
                    .unwrap();
                // Later than the window a waiting client allows other upstreams.
                sleep(Duration::from_millis(300)).await;
                release.send(()).unwrap();
                reported
            },
            select_ip_response_reporting_first(
                name,
                ResponseMode::FastestIp,
                &modes,
                tasks,
                Some(first_tx)
            )
        );
        assert_eq!(
            reported.unwrap().ip_addrs(),
            vec!["127.0.0.2".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            fastest.unwrap().ip_addrs(),
            vec!["127.0.0.1".parse::<IpAddr>().unwrap()],
            "the selection must wait for the slower upstream"
        );
    }

    #[tokio::test]
    async fn test_dropping_the_rest_after_the_first_answer_sends_no_probe() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let modes = [SpeedCheckMode::Tcp(listener.local_addr().unwrap().port())];
        let answer = speed_test_response(&["127.0.0.1"], RecordType::A);
        let name = answer.query().name().clone();
        let (first_tx, mut first_rx) = oneshot::channel();
        let mut selection = select_ip_response_reporting_first(
            name,
            ResponseMode::FastestIp,
            &modes,
            vec![async { Ok(answer) }.boxed()],
            Some(first_tx),
        )
        .boxed();

        let reported = match select(&mut first_rx, &mut selection).await {
            Either::Left((reported, _)) => reported.unwrap().unwrap(),
            Either::Right(_) => panic!("the selection must pause after the first answer"),
        };
        assert_eq!(
            reported.ip_addrs(),
            vec!["127.0.0.1".parse::<IpAddr>().unwrap()]
        );
        drop(selection);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "a dropped selection must not have probed"
        );
    }

    #[tokio::test]
    async fn test_fastest_ip_reports_the_last_answer_when_none_is_valid() {
        let query = Query::query("failed.example.".parse().unwrap(), RecordType::A);
        let error = |code| {
            let authority = AuthorityData::new(Box::new(query.clone()), None, true, false, None);
            let mut no_records: crate::libdns::proto::NoRecords = authority.into();
            no_records.response_code = code;
            LookupError::from(ProtoErrorKind::NoRecordsFound(no_records))
        };
        let (servfail, refused) = (error(ResponseCode::ServFail), error(ResponseCode::Refused));
        let tasks = vec![
            async { Err(servfail) }.boxed(),
            async {
                sleep(Duration::from_millis(20)).await;
                Err(refused)
            }
            .boxed(),
        ];
        let (first_tx, first_rx) = oneshot::channel();
        let result = select_ip_response_reporting_first(
            query.name().clone(),
            ResponseMode::FastestIp,
            &[SpeedCheckMode::Ping],
            tasks,
            Some(first_tx),
        )
        .await;
        assert!(result.is_err());
        let reported = first_rx.await.unwrap().unwrap_err();
        assert_eq!(
            reported
                .as_no_records_response(&query)
                .unwrap()
                .response_code(),
            ResponseCode::Refused
        );
    }

    /// Cache and name server middlewares over two UDP upstreams: the first
    /// answers at once with an address that fails the probe, the second 30 ms
    /// later with one that passes it.
    struct UpstreamHarness {
        handler: crate::dns_mw::DnsMiddlewareHandler,
        deferred_speed_checks: Arc<Semaphore>,
        queries: Vec<Arc<std::sync::atomic::AtomicUsize>>,
        background: tokio::sync::mpsc::UnboundedReceiver<crate::server::IncomingDnsMessage>,
        probe_target: tokio::net::TcpListener,
    }

    async fn upstream_harness() -> UpstreamHarness {
        use crate::dns_mw::DnsMiddlewareBuilder;
        use crate::dns_mw_cache::DnsCacheMiddleware;
        use crate::libdns::proto::op::Message;
        use crate::server::DnsHandle;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let probe_target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut directives = vec![
            "response-mode fastest-response".to_string(),
            format!(
                "speed-check-mode tcp:{}",
                probe_target.local_addr().unwrap().port()
            ),
            "prefetch-domain no".to_string(),
            "cache-persist no".to_string(),
        ];
        let mut queries = vec![];
        for (ip, delay_ms) in [("127.0.0.2", 0), ("127.0.0.1", 30)] {
            let socket = Arc::new(tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap());
            directives.push(format!("server {}", socket.local_addr().unwrap()));
            let count = Arc::new(AtomicUsize::new(0));
            queries.push(count.clone());
            let ip: IpAddr = ip.parse().unwrap();
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                loop {
                    // Windows reports a reply to an already closed client port
                    // as an error on the next receive.
                    let Ok((len, peer)) = socket.recv_from(&mut buffer).await else {
                        continue;
                    };
                    let Ok(request) = Message::from_vec(&buffer[..len]) else {
                        continue;
                    };
                    let mut response = request.to_response();
                    response.set_recursion_available(true);
                    if let Some(query) = request.queries().first()
                        && query.query_type() == RecordType::A
                    {
                        count.fetch_add(1, Ordering::SeqCst);
                        response.add_answer(Record::from_rdata(
                            query.name().clone(),
                            600,
                            RData::from(ip),
                        ));
                    }
                    let socket = socket.clone();
                    tokio::spawn(async move {
                        sleep(Duration::from_millis(delay_ms)).await;
                        let _ = socket.send_to(&response.to_vec().unwrap(), peer).await;
                    });
                }
            });
        }
        let cfg = directives
            .iter()
            .fold(RuntimeConfig::builder(), |builder, directive| {
                builder.with(directive)
            })
            .build()
            .unwrap();
        let cfg = Arc::new(cfg);
        let (background, background_handle) = DnsHandle::mock();
        let name_server = NameServerMiddleware::new(cfg.create_dns_client().await);
        let deferred_speed_checks = name_server.deferred_speed_checks.clone();
        let handler = DnsMiddlewareBuilder::new()
            .with(DnsCacheMiddleware::new(&cfg, background_handle))
            .with(name_server)
            .build(cfg);
        UpstreamHarness {
            handler,
            deferred_speed_checks,
            queries,
            background,
            probe_target,
        }
    }

    async fn wait_for_cached_ip(
        handler: &crate::dns_mw::DnsMiddlewareHandler,
        name: &Name,
        ip: &str,
    ) -> bool {
        let ip: IpAddr = ip.parse().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while handler
                .lookup(name.clone(), RecordType::A)
                .await
                .unwrap()
                .ip_addrs()
                != [ip]
            {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn test_fastest_response_speed_check_reuses_the_upstream_queries() {
        use std::sync::atomic::Ordering;

        let UpstreamHarness {
            handler,
            queries,
            mut background,
            probe_target: _probe_target,
            ..
        } = upstream_harness().await;
        let name: Name = "speed.example.".parse().unwrap();

        let response = handler.lookup(name.clone(), RecordType::A).await.unwrap();
        assert_eq!(
            response.ip_addrs(),
            vec!["127.0.0.2".parse::<IpAddr>().unwrap()],
            "the client must get the first upstream answer"
        );
        assert!(
            wait_for_cached_ip(&handler, &name, "127.0.0.1").await,
            "the cache must keep the probed fastest address"
        );
        for count in &queries {
            assert_eq!(
                count.load(Ordering::SeqCst),
                1,
                "each upstream must be queried once"
            );
        }
        assert!(background.try_recv().is_err(), "no second lookup may run");
    }

    #[tokio::test]
    async fn test_speed_checks_beyond_the_limit_keep_the_first_answer() {
        let UpstreamHarness {
            handler,
            deferred_speed_checks,
            probe_target,
            ..
        } = upstream_harness().await;
        let all = deferred_speed_checks
            .try_acquire_many_owned(MAX_DEFERRED_SPEED_CHECKS as u32)
            .unwrap();

        let name: Name = "full.speed.example.".parse().unwrap();
        let first: IpAddr = "127.0.0.2".parse().unwrap();
        let response = handler.lookup(name.clone(), RecordType::A).await.unwrap();
        assert_eq!(response.ip_addrs(), [first]);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), probe_target.accept())
                .await
                .is_err(),
            "a lookup beyond the limit must not probe"
        );
        let response = handler.lookup(name, RecordType::A).await.unwrap();
        assert_eq!(response.ip_addrs(), [first]);

        drop(all);
        let name: Name = "free.speed.example.".parse().unwrap();
        handler.lookup(name.clone(), RecordType::A).await.unwrap();
        assert!(
            wait_for_cached_ip(&handler, &name, "127.0.0.1").await,
            "a released slot must allow the next speed check"
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
