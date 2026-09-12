use chrono::DateTime;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Read;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::ops::DerefMut;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;

use crate::config::ServerOpts;
use crate::dns_conf::RuntimeConfig;
use crate::libdns::proto::ProtoError;
use crate::log;
use crate::server::DnsHandle;
use crate::{
    dns::*,
    libdns::proto::{
        op::{Message, Query},
        rr::DNSClass,
    },
    log::{debug, error, info},
    middleware::*,
};
use lru::LruCache;
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tokio::time::sleep;

pub struct DnsCacheMiddleware {
    cfg: Arc<RuntimeConfig>,
    cache: Arc<DnsCache>,
    prefetch_notify: Arc<DomainPrefetchingNotify>,
    client: DnsHandle,
}

impl DnsCacheMiddleware {
    pub fn new(cfg: &Arc<RuntimeConfig>, dns_handle: DnsHandle) -> Self {
        let mut cache = DnsCache::new(
            cfg.cache_size(),
            cfg.serve_expired(),
            cfg.serve_expired_ttl(),
            cfg.serve_expired_reply_ttl(),
        );
        cache.memory_limit = cfg
            .cache
            .memory_size
            .map(|size| usize::try_from(size.as_u64()).unwrap_or(usize::MAX))
            .unwrap_or(0);
        cache.prefetch_time = match cfg.cache.expired_prefetch_time.unwrap_or(0) {
            0 => match cfg.serve_expired_ttl() / 2 {
                0 => 8 * 3600,
                interval => interval.min(8 * 3600),
            },
            interval => interval,
        };

        if cfg.cache_persist() {
            let cache_file = cfg.cache_file();
            let memory_usage = cache.memory_usage.clone();
            let memory_limit = cache.memory_limit;
            let cache = cache.cache();
            let cache_checkpoint_time = cfg.cache_checkpoint_time();
            tokio::spawn(async move {
                if cache_file.exists() {
                    let mut entries = cache.lock().unwrap();
                    entries.load(cache_file.as_path());
                    let mut usage = entries
                        .iter()
                        .map(|(_, entry)| entry.memory_usage())
                        .sum::<usize>();
                    while memory_limit > 0 && usage > memory_limit {
                        if let Some((_, entry)) = entries.pop_lru() {
                            usage = usage.saturating_sub(entry.memory_usage());
                        } else {
                            break;
                        }
                    }
                    memory_usage.store(usage, Ordering::Relaxed);
                }
                let interval = Duration::from_secs(cache_checkpoint_time);
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(interval) => {
                            let entries: Vec<DnsCacheEntry> = {
                                let cache = cache.lock().unwrap();
                                cache.iter().map(|(_, e)| e.clone()).collect()
                            };
                            let cache_file = cache_file.clone();
                            tokio::task::spawn_blocking(move || {
                                let cache_to_file = || {
                                    let mut file = File::options()
                                        .create(true)
                                        .truncate(true)
                                        .write(true)
                                        .open(&cache_file)?;
                                    DnsCacheEntry::serialize_many(entries.iter(), &mut file)
                                };

                                match cache_to_file() {
                                    Ok(_) => log::info!("save DNS cache to file {:?} successfully.", cache_file),
                                    Err(err) => log::error!("failed to save DNS cache to file {}: {}", cache_file.display(), err),
                                }
                            });
                        }
                        _ = crate::signal::terminate() => {
                            let cache = cache.lock().unwrap();
                            cache.persist(cache_file.as_path());
                            log::debug!("save DNS cache to file {}", cache_file.display());
                            break;
                        }
                    };
                }
            });
        }

        let mw = Self {
            cfg: cfg.clone(),
            cache: Arc::new(cache),
            prefetch_notify: Arc::new(DomainPrefetchingNotify::new()),
            client: dns_handle.with_new_opt(ServerOpts {
                is_background: true,
                ..Default::default()
            }),
        };

        if cfg.prefetch_domain() {
            mw.start_prefetching();
        };

        mw
    }

    pub fn cache(&self) -> &Arc<DnsCache> {
        &self.cache
    }

    fn start_prefetching(&self) {
        let prefetch_notify = self.prefetch_notify.clone();

        let client = self.client.clone();
        let cache = self.cache.clone();
        tokio::spawn(async move {
            let min_interval = Duration::from_secs(
                std::env::var("PREFETCH_MIN_INTERVAL")
                    .as_deref()
                    .unwrap_or("1")
                    .parse()
                    .unwrap_or(1),
            );
            let mut last_check = Instant::now();

            loop {
                prefetch_notify.notified().await;

                let now = Instant::now();
                let most_recent;
                if now - last_check > min_interval {
                    last_check = now;

                    let expired = {
                        let (expired, most_recent0) = cache.get_expired(now, Some(5));

                        debug!(
                            "Domain prefetch check(total: {}), elapsed {:?}",
                            cache.cache().lock().unwrap().len(),
                            now.elapsed()
                        );

                        most_recent = most_recent0;

                        expired
                    };

                    prefetch_domains(&client, &cache, expired).await;
                } else {
                    most_recent = Duration::ZERO;
                }

                // sleep and wait for next check.
                let dura = most_recent.max(min_interval);
                prefetch_notify.notify_after(dura).await;
            }
        });
    }
}

#[async_trait::async_trait]
impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for DnsCacheMiddleware {
    async fn handle(
        &self,
        ctx: &mut DnsContext,
        req: &DnsRequest,
        next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
    ) -> Result<DnsResponse, DnsError> {
        // skip cache
        if ctx.server_opts.no_cache() || ctx.no_cache || req.is_dnssec() {
            return next.run(ctx, req).await;
        }

        let query = req.query().original();

        if !ctx.server_opts.is_background {
            crate::stats::CACHE_CHECKS.fetch_add(1, Ordering::Relaxed);
            let no_serve_expired = ctx.server_opts.no_serve_expired()
                || ctx
                    .domain_rule
                    .get(|r| r.no_serve_expired)
                    .unwrap_or_default();

            let cached_res = self.cache.get(query, Instant::now());

            match cached_res {
                // check if it's the same nameserver group.
                Some((res, status)) if res.name_server_group() == Some(ctx.server_group_name()) => {
                    match status {
                        CacheStatus::Valid => {
                            debug!(
                                "name: {} {} using caching",
                                query.name(),
                                query.query_type()
                            );

                            ctx.source = LookupFrom::Cache;
                            crate::stats::CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                            return Ok(res);
                        }
                        CacheStatus::Expired if ctx.cfg().serve_expired() && !no_serve_expired => {
                            // start backgroud query
                            {
                                let mut opts = ctx.server_opts.clone();
                                opts.is_background = true;
                                let client = self.client.with_new_opt(opts);
                                let query = query.clone();
                                tokio::spawn(async move {
                                    client.send(query).await;
                                });
                            }

                            debug!(
                                "name: {} {} using caching",
                                query.name(),
                                query.query_type()
                            );
                            ctx.source = LookupFrom::Cache;
                            crate::stats::CACHE_HITS.fetch_add(1, Ordering::Relaxed);
                            return Ok(res);
                        }
                        _ => (),
                    }
                }
                _ => (),
            }
        }

        let res = next.run(ctx, req).await;
        let res = match res {
            Err(error) if error.is_negative_response() => {
                match error.as_no_records_response(query) {
                    Some(response) => Ok(response),
                    None => Err(error),
                }
            }
            other => other,
        };

        match res {
            Ok(lookup) => {
                if let Some(ttl) = negative_response_ttl(&lookup) {
                    if ttl > 0 && !ctx.no_cache {
                        self.cache.insert_response(
                            lookup.clone(),
                            Instant::now(),
                            ttl,
                            ctx.server_group_name(),
                        );
                    }
                    return Ok(lookup);
                }
                if lookup
                    .records()
                    .iter()
                    .all(|record| record.record_type() != query.query_type())
                {
                    // A negative reply without an SOA has no cacheable lifetime.
                    return Ok(lookup);
                }

                if !ctx.no_cache {
                    let server_group_name = ctx.server_group_name();

                    self.cache.insert_response(
                        lookup.clone(),
                        Instant::now(),
                        lookup.min_ttl().unwrap_or(0),
                        server_group_name,
                    );

                    if ctx.cfg().prefetch_domain()
                        && let Some(ttl) = lookup.min_ttl()
                    {
                        self.prefetch_notify
                            .notify_after(Duration::from_secs(ttl as u64))
                            .await;
                    }
                }
                Ok(lookup)
            }
            Err(err) => Err(err),
        }
    }
}

fn negative_response_ttl(response: &DnsResponse) -> Option<u32> {
    use crate::libdns::proto::op::ResponseCode;
    if !matches!(
        response.response_code(),
        ResponseCode::NoError | ResponseCode::NXDomain
    ) || (response.response_code() == ResponseCode::NoError
        && response
            .answers()
            .iter()
            .any(|r| r.record_type() == response.query().query_type()))
    {
        return None;
    }
    let soa_ttl = response
        .authorities()
        .iter()
        .filter_map(|record| match record.data() {
            RData::SOA(soa) => Some(record.ttl().min(soa.minimum())),
            _ => None,
        })
        .min()?;
    Some(
        response
            .answers()
            .iter()
            .filter(|r| r.record_type() == RecordType::CNAME)
            .fold(soa_ttl, |ttl, record| ttl.min(record.ttl())),
    )
}

// A persisted cache can expire thousands of entries together. Limit both
// concurrency and admission rate so fast refreshes cannot starve live queries
// or fill the router's connection tracking table by cycling UDP source ports.
const MAX_PREFETCH_CONCURRENCY: usize = 16;

async fn prefetch_domains(
    client: &DnsHandle,
    cache: &DnsCache,
    domains: Vec<(Query, Option<String>)>,
) {
    use futures_util::{StreamExt, stream};

    let mut batches = stream::iter(domains).chunks(MAX_PREFETCH_CONCURRENCY);
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    while let Some(batch) = batches.next().await {
        interval.tick().await;
        stream::iter(batch)
            .for_each_concurrent(MAX_PREFETCH_CONCURRENCY, |(query, group)| {
                let opts = ServerOpts {
                    is_background: true,
                    rule_group: group,
                    ..Default::default()
                };
                let client = client.with_new_opt(opts);
                async move {
                    let now = Instant::now();
                    client.send(query.clone()).await;
                    cache.finish_prefetch(&query, Instant::now());
                    debug!(
                        "Prefetch domain {} {}, elapsed {:?}",
                        query.name(),
                        query.query_type(),
                        now.elapsed()
                    );
                }
            })
            .await;
    }
}

struct DomainPrefetchingNotify {
    notity: Arc<Notify>,
    tick: RwLock<Instant>,
}

impl DomainPrefetchingNotify {
    pub fn new() -> Self {
        Self {
            notity: Default::default(),
            tick: RwLock::new(Instant::now()),
        }
    }

    async fn notify_after(&self, duration: Duration) {
        if duration.is_zero() {
            self.notity.notify_one()
        } else {
            let tick = *self.tick.read().await;
            let now = Instant::now();
            let next_tick = now + duration;
            if tick > now && next_tick > tick {
                debug!(
                    "Domain prefetch check will be performed in {:?}.",
                    tick - now
                );
                return;
            }

            *self.tick.write().await.deref_mut() = next_tick;
            debug!("Domain prefetch check will be performed in {:?}.", duration);
            let notify = self.notity.clone();
            tokio::spawn(async move {
                sleep(duration).await;
                notify.notify_one();
            });
        }
    }
}

impl Deref for DomainPrefetchingNotify {
    type Target = Notify;

    fn deref(&self) -> &Self::Target {
        self.notity.as_ref()
    }
}

/// Maximum TTL as defined in https://tools.ietf.org/html/rfc2181, 2147483647
/// Setting this to a value of 1 day, in seconds
const MAX_TTL: u32 = 86400_u32;

/// An LRU eviction cache specifically for storing DNS records
pub struct DnsCache {
    cache: Arc<Mutex<LruCache<Query, DnsCacheEntry>>>,
    serve_expired: bool,
    expired_ttl: u64,
    expired_reply_ttl: u64,
    memory_limit: usize,
    memory_usage: Arc<AtomicUsize>,
    prefetch_time: u64,
}

impl DnsCache {
    pub fn memory_bytes(&self) -> usize {
        self.memory_usage.load(Ordering::Relaxed)
    }

    pub fn len(&self) -> usize {
        self.cache.lock().unwrap().len()
    }

    pub fn summary_entries(&self) -> Vec<CacheEntrySummary> {
        let now = Instant::now();
        self.cache
            .lock()
            .unwrap()
            .iter()
            .take(1000)
            .map(|(query, entry)| CacheEntrySummary {
                name: query.name().to_ascii(),
                record_type: query.query_type().to_string(),
                ttl: entry.valid_until.saturating_duration_since(now).as_secs(),
                inserted_at: DateTime::<Local>::from(entry.stats.inserted_at).timestamp(),
                rcode: entry.data.response_code().to_string(),
                answers: entry
                    .data
                    .answers()
                    .iter()
                    .chain(entry.data.authorities())
                    .map(|record| record.data().to_string())
                    .collect(),
            })
            .collect()
    }
    fn new(
        cache_size: usize,
        serve_expired: bool,
        expired_ttl: u64,
        expired_reply_ttl: u64,
    ) -> Self {
        let cache = Arc::new(Mutex::new(LruCache::new(
            NonZeroUsize::new(cache_size).unwrap(),
        )));

        Self {
            cache,
            serve_expired,
            expired_ttl,
            expired_reply_ttl,
            memory_limit: 0,
            memory_usage: Arc::new(AtomicUsize::new(0)),
            prefetch_time: 0,
        }
    }

    fn cache(&self) -> Arc<Mutex<LruCache<Query, DnsCacheEntry>>> {
        self.cache.clone()
    }

    pub fn clear(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.clear();
        self.memory_usage.store(0, Ordering::Relaxed);
    }

    pub fn cached_records(&self) -> Vec<CachedQueryRecord> {
        self.cache
            .lock()
            .unwrap()
            .iter()
            .map(|(query, entry)| CachedQueryRecord {
                name: query.name().clone(),
                query_type: query.query_type(),
                query_class: query.query_class(),
                records: entry.data.records().to_vec().into_boxed_slice(),
                hits: entry.stats.hits,
                last_access: entry.stats.last_access.into(),
            })
            .collect()
    }

    fn insert_response(
        &self,
        response: DnsResponse,
        now: Instant,
        ttl: u32,
        name_server_group: &str,
    ) {
        let valid_until = now + Duration::from_secs(ttl as u64);
        let mut lookup = response.with_valid_until(valid_until);
        if lookup.name_server_group() != Some(name_server_group) {
            lookup = lookup.with_name_server_group(name_server_group.to_owned());
        }
        let query = lookup.query().clone();
        let entry = DnsCacheEntry::new(lookup, valid_until);
        {
            // Publish the entry before returning, so the next query can use it.
            let mut cache = self.cache.lock().unwrap();
            let mut usage = self.memory_usage.load(Ordering::Relaxed);
            if let Some(previous) = cache.get_mut(&query) {
                usage = usage.saturating_sub(previous.memory_usage());
                let hits = previous.stats.hits;
                *previous = entry;
                previous.stats.hits = hits + 1;
                usage += previous.memory_usage();
            } else {
                usage += entry.memory_usage();
                if let Some((_, evicted)) = cache.push(query, entry) {
                    usage = usage.saturating_sub(evicted.memory_usage());
                }
            }
            while self.memory_limit > 0 && usage > self.memory_limit {
                if let Some((_, entry)) = cache.pop_lru() {
                    usage = usage.saturating_sub(entry.memory_usage());
                } else {
                    break;
                }
            }
            self.memory_usage.store(usage, Ordering::Relaxed);
        }
    }

    /// Based on the query, see if there are any records available
    fn get(&self, query: &Query, now: Instant) -> Option<(DnsResponse, CacheStatus)> {
        let mut cache = self.cache.lock().unwrap();

        let value = cache.get_mut(query)?;
        if !value.is_current(now)
            && (negative_response_ttl(&value.data).is_some()
                || !self.serve_expired
                || (self.expired_ttl > 0
                    && now.duration_since(value.valid_until)
                        > Duration::from_secs(self.expired_ttl)))
        {
            let removed = cache.pop(query).unwrap();
            self.memory_usage
                .fetch_sub(removed.memory_usage(), Ordering::Relaxed);
            return None;
        }

        value.stats.hit();
        let (ttl, status) = if value.is_current(now) {
            (value.ttl(now).as_secs() as u32, CacheStatus::Valid)
        } else {
            (self.expired_reply_ttl as u32, CacheStatus::Expired)
        };
        let repeated_ttl = value.reply_ttl.replace(ttl) == Some(ttl);
        if repeated_ttl && let Some(reply) = &value.ttl_reply {
            return Some((reply.response.clone(), status));
        }

        let mut res = {
            let mut res = value.data.clone();

            // For CNAME query, the cached response might only contain A/AAAA records
            // with the final name of the CNAME chain. If so, we should rewrite
            // the record names to match the original query name.
            // We detect this by checking if there are no CNAME records in the
            // response, all records are IP records, and there are records with a
            // name different from the query name.
            let has_cname = res
                .answers()
                .iter()
                .any(|r| r.record_type() == RecordType::CNAME);

            let all_ip_records = !res.answers().is_empty()
                && res.answers().iter().all(|r| r.record_type().is_ip_addr());

            if !has_cname
                && all_ip_records
                && res.answers().iter().any(|r| r.name() != query.name())
            {
                let query_name = query.name().clone();
                for record in res.answers_mut() {
                    record.set_name(query_name.clone());
                }
            }

            res.set_max_ttl(ttl);
            res
        };

        // Keep an adjusted response only after repeated hits at the same TTL.
        // Sparse traffic retains no extra message; the original remains intact
        // for persistence and stale replies.
        let previous = value
            .ttl_reply
            .as_ref()
            .map_or(0, |reply| reply.memory_usage);
        let mut total = self.memory_usage.load(Ordering::Relaxed) - previous;
        value.memory_usage -= previous;
        value.ttl_reply = None;
        if repeated_ttl {
            res.prepare_wire();
            let memory_usage =
                DnsCacheEntry::response_memory(&res) + std::mem::size_of::<TtlReply>();
            if self.memory_limit == 0 || total + memory_usage <= self.memory_limit {
                value.memory_usage += memory_usage;
                total += memory_usage;
                value.ttl_reply = Some(Box::new(TtlReply {
                    response: res.clone(),
                    memory_usage,
                }));
            }
        }
        self.memory_usage.store(total, Ordering::Relaxed);
        Some((res, status))
    }

    fn finish_prefetch(&self, query: &Query, now: Instant) {
        let mut cache = self.cache.lock().unwrap();
        if let Some(entry) = cache.peek_mut(query)
            && entry.is_in_prefetching
        {
            // Successful insertion already resets the flag. Failed refreshes
            // must become eligible again without continuously retrying.
            entry.is_in_prefetching = false;
            entry.retry_prefetch_at = Some(now + Duration::from_secs(self.prefetch_time.max(1)));
        }
    }

    fn get_expired(
        &self,
        now: Instant,
        seconds_ahead: Option<u64>,
    ) -> (Vec<(Query, Option<String>)>, Duration) {
        let mut cache = self.cache.lock().unwrap();
        let mut most_recent = Duration::from_secs(MAX_TTL as u64);

        if !cache.is_empty() {
            let mut expired = vec![];
            let delay = if self.serve_expired {
                self.prefetch_time
            } else {
                0
            };
            let check_time = if delay > 0 {
                now.checked_sub(Duration::from_secs(delay)).unwrap_or(now)
            } else {
                now
            } + Duration::from_secs(seconds_ahead.unwrap_or(5)); // 5 seconds ahead

            for (query, entry) in cache.iter_mut() {
                if negative_response_ttl(&entry.data).is_some() {
                    continue;
                }
                if entry.is_in_prefetching {
                    continue;
                }
                if let Some(retry_at) = entry.retry_prefetch_at
                    && retry_at > now
                {
                    most_recent = most_recent.min(retry_at.duration_since(now));
                    continue;
                }
                // only prefetch query type ip addr
                if !query.query_type().is_ip_addr() {
                    continue;
                }

                if entry.is_current(check_time) {
                    most_recent = most_recent.min(entry.ttl(check_time));
                    continue;
                }

                entry.is_in_prefetching = true;

                expired.push((
                    query.to_owned(),
                    entry.stats.hits,
                    entry.data.name_server_group().map(String::from),
                ));
            }
            drop(cache);

            expired.sort_by_key(|(_, hits, _)| std::cmp::Reverse(*hits));

            (
                expired.into_iter().map(|(q, _, g)| (q, g)).collect(),
                most_recent,
            )
        } else {
            (Vec::with_capacity(0), most_recent)
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CacheStatus {
    Valid,
    Expired,
}

#[derive(Deserialize, Serialize)]
pub struct CachedQueryRecord {
    name: Name,
    hits: usize,
    last_access: DateTime<Local>,
    query_type: RecordType,
    query_class: DNSClass,
    records: Box<[Record]>,
}

#[derive(Serialize)]
pub struct CacheEntrySummary {
    pub name: String,
    pub record_type: String,
    pub ttl: u64,
    pub inserted_at: i64,
    pub rcode: String,
    pub answers: Vec<String>,
}

#[derive(Clone)]
struct DnsCacheEntry {
    data: DnsResponse,
    memory_usage: usize,
    valid_until: Instant,
    is_in_prefetching: bool,
    retry_prefetch_at: Option<Instant>,
    stats: DnsCacheStats,
    reply_ttl: Option<u32>,
    ttl_reply: Option<Box<TtlReply>>,
}

#[derive(Clone)]
struct TtlReply {
    response: DnsResponse,
    memory_usage: usize,
}

impl DnsCacheEntry {
    fn memory_usage(&self) -> usize {
        self.memory_usage
    }

    fn measure_memory(data: &DnsResponse) -> usize {
        Self::response_memory(data)
            + std::mem::size_of::<Self>()
            + std::mem::size_of::<Query>()
            + data.query().name().len()
    }

    fn response_memory(data: &DnsResponse) -> usize {
        // Charge the DNS payload plus decoded records, questions, and message storage.
        let records = data.answers().len() + data.authorities().len() + data.additionals().len();
        data.wire_len()
            .unwrap_or_else(|| data.to_vec().map(|wire| wire.len()).unwrap_or(0))
            + records * std::mem::size_of::<Record>()
            + std::mem::size_of::<Message>()
            + 2 * std::mem::size_of::<usize>()
            + std::mem::size_of_val(data.queries())
            + data.name_server_group().map(str::len).unwrap_or(0)
            + data.prepared_wire_memory()
    }
}

impl DnsCacheEntry {
    fn new(data: DnsResponse, valid_until: Instant) -> Self {
        Self {
            memory_usage: Self::measure_memory(&data),
            data,
            valid_until,
            is_in_prefetching: false,
            retry_prefetch_at: None,
            stats: DnsCacheStats::new(),
            reply_ttl: None,
            ttl_reply: None,
        }
    }

    fn set_valid_until(&mut self, valid_until: Instant) {
        self.valid_until = valid_until;
    }

    /// Returns true if this set of ips is still valid
    fn is_current(&self, now: Instant) -> bool {
        now <= self.valid_until
    }

    /// Returns the ttl as a Duration of time remaining.
    fn ttl(&self, now: Instant) -> Duration {
        self.valid_until.saturating_duration_since(now)
    }
}

#[derive(Clone)]
struct DnsCacheStats {
    inserted_at: std::time::SystemTime,
    /// The number of lookups that have been performed
    hits: usize,
    last_access: std::time::SystemTime,
}

impl DnsCacheStats {
    fn new() -> Self {
        let now = std::time::SystemTime::now();
        Self {
            inserted_at: now,
            hits: 0,
            last_access: now,
        }
    }

    fn hit(&mut self) {
        self.hits += 1;
        self.last_access = std::time::SystemTime::now();
    }
}

use crate::libdns::proto::serialize::binary::{
    BinDecodable, BinDecoder, BinEncodable, BinEncoder, DecodeError,
};

impl BinEncodable for DnsCacheEntry {
    fn emit(&self, encoder: &mut BinEncoder<'_>) -> Result<(), ProtoError> {
        let res = &self.data;

        // message
        encoder.emit_u8(1)?;
        res.deref().emit(encoder)?;

        // valid_until
        encoder.emit_u8(5)?;
        let now = Instant::now();
        let wall_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expires = if self.valid_until > now {
            wall_now.saturating_add((self.valid_until - now).as_secs())
        } else {
            wall_now.saturating_sub((now - self.valid_until).as_secs())
        };
        encoder.emit_u32((expires >> 32) as u32)?;
        encoder.emit_u32(expires as u32)?;

        // group_name
        encoder.emit_u8(3)?;
        if let Some(group_name) = res.name_server_group().map(|n| n.as_bytes()) {
            encoder.emit_u16(group_name.len() as u16)?;
            encoder.emit_vec(group_name)?;
        } else {
            encoder.emit_u16(0)?;
        }

        // hits
        encoder.emit_u8(6)?;
        let (kind, micros) = match res.probe_result() {
            ProbeResult::Unchecked => (0, 0),
            ProbeResult::Failed => (1, 0),
            ProbeResult::Measured(duration) => {
                (2, duration.as_micros().min(u32::MAX as u128) as u32)
            }
        };
        encoder.emit_u8(kind)?;
        encoder.emit_u32(micros)?;
        encoder.emit_u8(4)?;
        encoder.emit_u32(self.stats.hits as u32)?;
        Ok(())
    }
}

impl<'r> BinDecodable<'r> for DnsCacheEntry {
    fn read(decoder: &mut BinDecoder<'r>) -> Result<Self, ProtoError> {
        // message
        if !decoder.read_u8()?.verify(|v| *v == 1).is_valid() {
            return Err(DecodeError::InsufficientBytes.into());
        }
        let message = Message::read(decoder)?;

        // valid_until
        let valid_until = match decoder.read_u8()?.unverified() {
            // Existing cache files store a relative remaining TTL.
            2 => Instant::now() + Duration::from_secs(decoder.read_u32()?.unverified() as u64),
            5 => {
                let expires = ((decoder.read_u32()?.unverified() as u64) << 32)
                    | decoder.read_u32()?.unverified() as u64;
                let wall_now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let now = Instant::now();
                if expires > wall_now {
                    now + Duration::from_secs(expires - wall_now)
                } else {
                    now.checked_sub(Duration::from_secs(wall_now - expires))
                        .unwrap_or(now)
                }
            }
            _ => return Err(DecodeError::InsufficientBytes.into()),
        };

        // group_name
        if !decoder.read_u8()?.verify(|v| *v == 3).is_valid() {
            return Err(DecodeError::InsufficientBytes.into());
        }
        let group_name = {
            let name_len = decoder.read_u16()?.unverified();
            if name_len > 0 {
                let name_bytes = decoder.read_slice(name_len as usize)?.unverified();
                String::from_utf8(name_bytes.to_vec()).ok()
            } else {
                None
            }
        };

        // hits
        let mut field = decoder.read_u8()?.unverified();
        let mut probe = ProbeResult::Unchecked;
        if field == 6 {
            let kind = decoder.read_u8()?.unverified();
            let micros = decoder.read_u32()?.unverified();
            probe = match kind {
                0 => ProbeResult::Unchecked,
                1 => ProbeResult::Failed,
                2 => ProbeResult::Measured(Duration::from_micros(micros as u64)),
                _ => return Err(DecodeError::InsufficientBytes.into()),
            };
            field = decoder.read_u8()?.unverified();
        }
        if field != 4 {
            return Err(DecodeError::InsufficientBytes.into());
        }
        let hits = decoder.read_u32()?.unverified();

        // construct the response
        let mut res: DnsResponse = message.into();
        res = res.with_valid_until(valid_until).with_probe_result(probe);
        if let Some(g) = group_name {
            res = res.with_name_server_group(g);
        }
        let mut entry = DnsCacheEntry::new(res, valid_until);
        entry.stats.hits = hits as usize;

        Ok(entry)
    }
}

impl DnsCacheEntry {
    fn serialize_many<'a>(
        entries: impl Iterator<Item = &'a DnsCacheEntry>,
        writer: &mut impl std::io::Write,
    ) -> Result<(), ProtoError> {
        let mut buf = vec![];

        for entry in entries {
            buf.truncate(0);
            let mut encoder = BinEncoder::new(&mut buf);
            if (*entry).emit(&mut encoder).is_ok() {
                let _ = writer.write_all(&buf);
            }
        }
        Ok(())
    }

    fn deserialize_many(data: &[u8]) -> Result<Vec<DnsCacheEntry>, ProtoError> {
        let mut entries = vec![];
        let mut offset = 0;

        while offset < data.len() {
            let mut decoder = BinDecoder::new(&data[offset..]);
            entries.push(DnsCacheEntry::read(&mut decoder)?);
            offset += decoder.index();
        }

        Ok(entries)
    }
}

trait PersistCache {
    fn persist<P: AsRef<Path>>(&self, path: P);

    fn load<P: AsRef<Path>>(&mut self, path: P);
}

impl PersistCache for LruCache<Query, DnsCacheEntry> {
    fn persist<P: AsRef<Path>>(&self, path: P) {
        let path = path.as_ref();
        let cache_to_file = || {
            let mut file = File::options()
                .create(true)
                .truncate(true)
                .write(true)
                .open(path)?;
            let entries = self.iter().map(|(_, entry)| entry);
            DnsCacheEntry::serialize_many(entries, &mut file)
        };

        match cache_to_file() {
            Ok(_) => info!("save DNS cache to file {:?} successfully.", path),
            Err(err) => error!("failed to save DNS cache to file {}", err),
        }
    }

    fn load<P: AsRef<Path>>(&mut self, path: P) {
        let path = path.as_ref();
        info!("reading DNS cache from file: {:?}", path);
        let now = Instant::now();

        let read_from_cache_file = || {
            let mut file = File::options().read(true).open(path)?;
            let mut data = vec![];
            file.read_to_end(&mut data)?;

            DnsCacheEntry::deserialize_many(&data)
        };

        match read_from_cache_file() {
            Ok(entries) => {
                let count = entries.len();
                let cache = self;
                for entry in entries {
                    let query = entry.data.query().clone();
                    cache.put(query, entry);
                }
                info!(
                    "DNS cache {} records loaded, elapsed {:?}",
                    count,
                    now.elapsed()
                );
            }
            Err(err) => error!("failed to read DNS cache file {:?} {}", path, err),
        }
    }
}

#[cfg(test)]
mod tests {

    use rr::rdata::{A, CNAME};
    use std::net::IpAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn test_c_compat_negative_cache_keeps_soa_and_expires() {
        use crate::dns_mw::DnsMockMiddleware;
        use crate::libdns::proto::{op::ResponseCode, rr::rdata::SOA};
        for code in [ResponseCode::NoError, ResponseCode::NXDomain] {
            let cfg = Arc::new(RuntimeConfig::default());
            let (_, handle) = DnsHandle::mock();
            let middleware = DnsCacheMiddleware::new(&cfg, handle);
            let cache = middleware.cache().clone();
            let query = Query::query("negative.example.".parse().unwrap(), RecordType::AAAA);
            let mut negative = DnsResponse::new_with_max_ttl(query.clone(), []);
            negative.set_response_code(code);
            negative.add_authority(Record::from_rdata(
                "example.".parse().unwrap(),
                60,
                RData::SOA(SOA::new(
                    "ns.example.".parse().unwrap(),
                    "hostmaster.example.".parse().unwrap(),
                    1,
                    60,
                    60,
                    3600,
                    30,
                )),
            ));
            let handler = DnsMockMiddleware::mock(middleware)
                .with_result(query.clone(), Ok(negative))
                .build(cfg);
            handler
                .lookup(query.name().clone(), RecordType::AAAA)
                .await
                .unwrap();
            let response = handler
                .lookup(query.name().clone(), RecordType::AAAA)
                .await
                .unwrap();
            assert_eq!(response.response_code(), code);
            assert_eq!(response.name_server_group(), Some("default"));
            assert!(response.answers().is_empty());
            assert_eq!(response.authorities()[0].record_type(), RecordType::SOA);
            assert!(response.authorities()[0].ttl() <= 30);
            let deadline = cache.cache.lock().unwrap().get(&query).unwrap().valid_until;
            let cached = cache
                .get(&query, deadline - Duration::from_secs(1))
                .unwrap()
                .0;
            assert_eq!(cached.authorities()[0].ttl(), 1);
            assert!(
                cache
                    .get(&query, deadline + Duration::from_secs(1))
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn test_c_compat_memory_budget_evicts_oldest_entries() {
        let mut cache = DnsCache::new(100, true, 0, 5);
        let now = Instant::now();
        let q1 = Query::query("one.example.".parse().unwrap(), RecordType::A);
        let q2 = Query::query("two.example.".parse().unwrap(), RecordType::A);
        cache.insert_response(
            DnsResponse::new_with_max_ttl(
                q1.clone(),
                [Record::from_rdata(
                    q1.name().clone(),
                    60,
                    RData::A("192.0.2.1".parse().unwrap()),
                )],
            ),
            now,
            60,
            "default",
        );
        cache.memory_limit = cache.memory_usage.load(Ordering::Relaxed);
        cache.insert_response(
            DnsResponse::new_with_max_ttl(
                q2.clone(),
                [Record::from_rdata(
                    q2.name().clone(),
                    60,
                    RData::A("192.0.2.2".parse().unwrap()),
                )],
            ),
            now,
            60,
            "default",
        );
        assert!(cache.get(&q1, now).is_none());
        assert_eq!(
            cache.get(&q2, now).unwrap().0.ip_addrs(),
            vec!["192.0.2.2".parse::<IpAddr>().unwrap()]
        );
        assert!(cache.memory_usage.load(Ordering::Relaxed) <= cache.memory_limit);
        cache.clear();
        assert_eq!(cache.memory_usage.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn ttl_replies_preserve_records_expiry_and_the_memory_budget() {
        let query = Query::query("ttl.example.".parse().unwrap(), RecordType::A);
        let response = DnsResponse::new_with_max_ttl(
            query.clone(),
            [Record::from_rdata(
                query.name().clone(),
                30,
                RData::A("192.0.2.1".parse().unwrap()),
            )],
        );
        let now = Instant::now();
        let mut cache = DnsCache::new(4, true, 30, 5);
        cache.insert_response(response.clone(), now, 30, "default");
        let original_memory = cache.memory_bytes();
        let (mut first, _) = cache.get(&query, now + Duration::from_millis(100)).unwrap();
        assert_eq!(first.answers()[0].ttl(), 29);
        first.set_new_ttl(1);
        assert_eq!(cache.memory_bytes(), original_memory);
        cache.get(&query, now + Duration::from_millis(200)).unwrap();
        let memory = cache.memory_bytes();
        let (second, _) = cache.get(&query, now + Duration::from_millis(900)).unwrap();
        assert_eq!(second.answers()[0].ttl(), 29);
        assert_eq!(
            second.ip_addrs(),
            vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(cache.memory_bytes(), memory);
        cache
            .get(&query, now + Duration::from_millis(1100))
            .unwrap();
        assert_eq!(cache.memory_bytes(), original_memory);
        assert_eq!(
            cache
                .get(&query, now + Duration::from_secs(30))
                .unwrap()
                .0
                .answers()[0]
                .ttl(),
            0
        );
        let (stale, status) = cache.get(&query, now + Duration::from_secs(31)).unwrap();
        assert!(matches!(status, CacheStatus::Expired));
        assert_eq!(stale.answers()[0].ttl(), 5);
        assert_eq!(cache.cached_records()[0].records[0].ttl(), 30);

        cache.clear();
        cache.insert_response(response, now, 30, "default");
        cache.memory_limit = cache.memory_bytes();
        cache.get(&query, now + Duration::from_millis(100)).unwrap();
        let (reply, _) = cache.get(&query, now + Duration::from_millis(900)).unwrap();
        assert_eq!(reply.answers()[0].ttl(), 29);
        assert_eq!(cache.memory_bytes(), cache.memory_limit);
        assert_eq!(cache.len(), 1);
    }

    #[tokio::test]
    async fn test_memory_accounting_updates_on_replacement_and_expiry() {
        use crate::libdns::proto::rr::rdata::TXT;
        let cache = DnsCache::new(10, false, 0, 0);
        let now = Instant::now();
        let query = Query::query("memory.example.".parse().unwrap(), RecordType::TXT);
        for text in ["short".to_owned(), "longer".repeat(32)] {
            let response = DnsResponse::from_rdata(query.clone(), RData::TXT(TXT::new(vec![text])));
            let expected = response.clone().with_name_server_group("default".into());
            cache.insert_response(response, now, 1, "default");
            assert_eq!(cache.len(), 1);
            assert_eq!(
                cache.memory_bytes(),
                DnsCacheEntry::measure_memory(&expected)
            );
            assert_eq!(
                cache.get(&query, now).unwrap().0.answers(),
                expected.answers()
            );
        }
        assert!(cache.get(&query, now + Duration::from_secs(2)).is_none());
        assert_eq!(cache.memory_bytes(), 0);
    }

    #[tokio::test]
    async fn test_query_stale_cache_policy_is_kept_on_upstream_failure() {
        use crate::dns_mw::DnsMockMiddleware;
        for policy in [
            "serve-expired no",
            "domain-rule /expired.example/ -no-serve-expired",
            "serve-expired-ttl 2",
        ] {
            let cfg = Arc::new(
                RuntimeConfig::builder()
                    .with("serve-expired yes")
                    .with("prefetch-domain no")
                    .with("cache-persist no")
                    .with(policy)
                    .build()
                    .unwrap(),
            );
            let (mut background_rx, background_handle) = DnsHandle::mock();
            let cache_middleware = DnsCacheMiddleware::new(&cfg, background_handle);
            let query = Query::query("expired.example.".parse().unwrap(), RecordType::A);
            let record = Record::from_rdata(
                query.name().clone(),
                30,
                RData::A("192.0.2.1".parse().unwrap()),
            );
            cache_middleware.cache().insert_response(
                DnsResponse::new_with_max_ttl(query.clone(), [record]),
                Instant::now() - Duration::from_secs(40),
                30,
                "default",
            );
            let error = DnsError::no_records_found(query.clone(), 1);
            let handler = DnsMockMiddleware::mock(cache_middleware)
                .with_result(query.clone(), Err(error.clone()))
                .build(cfg);
            assert_eq!(
                handler
                    .lookup(query.name().clone(), RecordType::A)
                    .await
                    .unwrap_err(),
                error,
                "{policy}"
            );
            assert!(background_rx.try_recv().is_err(), "{policy}");
        }
    }

    #[tokio::test]
    async fn test_query_stale_cache_age_limit_and_unlimited_default() {
        let query = Query::query("expired.example.".parse().unwrap(), RecordType::A);
        let record = Record::from_rdata(
            query.name().clone(),
            30,
            RData::A("192.0.2.1".parse().unwrap()),
        );
        let now = Instant::now();
        for (limit, age, allowed) in [(2, 32, true), (2, 33, false), (0, 86400, true)] {
            let cache = DnsCache::new(10, true, limit, 5);
            cache.insert_response(
                DnsResponse::new_with_max_ttl(query.clone(), [record.clone()]),
                now,
                30,
                "default",
            );
            let response = cache.get(&query, now + Duration::from_secs(age));
            if allowed {
                let (response, status) = response.unwrap();
                assert!(matches!(status, CacheStatus::Expired));
                assert_eq!(
                    response.records(),
                    &[Record::from_rdata(
                        query.name().clone(),
                        5,
                        record.data().clone()
                    )]
                );
            } else {
                assert!(response.is_none());
            }
        }
    }

    struct CountingUpstream {
        queries: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for CountingUpstream {
        async fn handle(
            &self,
            _ctx: &mut DnsContext,
            req: &DnsRequest,
            _next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
        ) -> Result<DnsResponse, DnsError> {
            self.queries.fetch_add(1, Ordering::SeqCst);

            let query = req.query().original().clone();
            let record = Record::from_rdata(
                query.name().clone(),
                600,
                RData::A("192.0.2.1".parse().unwrap()),
            );
            Ok(DnsResponse::new_with_max_ttl(query, vec![record]))
        }
    }

    fn create_lookup(name: &str, data: RData, ttl: u64) -> DnsCacheEntry {
        let name: Name = name.parse().unwrap();
        let ttl = Duration::from_secs(ttl);
        let query = Query::query(name.clone(), data.record_type());
        let records = vec![Record::from_rdata(name, ttl.as_secs() as u32, data)];
        let valid_until = Instant::now() + ttl;
        DnsCacheEntry::new(
            DnsResponse::new_with_deadline(query, records, valid_until),
            valid_until,
        )
    }

    #[test]
    fn test_lookup_serde() {
        let lookups = [
            create_lookup(
                "abc.exmample.com.",
                RData::A("127.0.0.1".parse().unwrap()),
                30,
            ),
            create_lookup("xyz.exmample.com.", RData::AAAA("::1".parse().unwrap()), 38),
        ];

        let mut data = vec![];
        DnsCacheEntry::serialize_many(lookups.iter(), &mut data).unwrap();
        let lookup2 = DnsCacheEntry::deserialize_many(&data).unwrap();

        assert_eq!(lookup2.len(), lookups.len());

        assert_eq!(&lookups[0].data, &lookup2[0].data);
        assert_eq!(&lookups[1].data, &lookup2[1].data);
    }

    #[test]
    fn test_persist_retains_stale_age_and_probe_result() {
        let mut entry = create_lookup("stale.example.", RData::A("192.0.2.1".parse().unwrap()), 60);
        entry.valid_until = Instant::now() - Duration::from_secs(300);
        entry.data = entry
            .data
            .with_probe_result(ProbeResult::Measured(Duration::from_micros(12345)));
        let mut data = vec![];
        DnsCacheEntry::serialize_many(std::iter::once(&entry), &mut data).unwrap();
        let restored = DnsCacheEntry::deserialize_many(&data).unwrap();
        assert!(Instant::now().duration_since(restored[0].valid_until) >= Duration::from_secs(299));
        assert_eq!(restored[0].data.probe_result(), entry.data.probe_result());
    }

    #[tokio::test]
    async fn test_cache_persist() {
        let lookup1 = create_lookup(
            "abc.exmample.com.",
            RData::A("127.0.0.1".parse().unwrap()),
            3000,
        );
        let lookup2 = create_lookup(
            "xyz.exmample.com.",
            RData::AAAA("::1".parse().unwrap()),
            3000,
        );

        let cache = DnsCache::new(10, true, 30, 5);

        let now = Instant::now();

        cache.insert_response(
            lookup1.data.clone(),
            now,
            lookup1.data.min_ttl().unwrap(),
            "default",
        );

        cache.insert_response(
            lookup2.data.clone(),
            now,
            lookup2.data.min_ttl().unwrap(),
            "default",
        );

        assert!(cache.get(lookup1.data.query(), now).is_some());

        {
            let lru_cache = cache.cache();
            let mut lru_cache = lru_cache.lock().unwrap();
            assert_eq!(lru_cache.len(), 2);

            lru_cache.persist("./logs/smartdns-test.cache");

            assert!(lru_cache.get(lookup1.data.query()).is_some());

            lru_cache.clear();

            assert_eq!(lru_cache.len(), 0);

            lru_cache.load("./logs/smartdns-test.cache");

            assert_eq!(lru_cache.len(), 2);

            assert!(
                lru_cache
                    .iter()
                    .map(|(q, _)| q)
                    .any(|q| q == lookup1.data.query())
            );
            assert!(
                lru_cache
                    .iter()
                    .map(|(q, _)| q)
                    .any(|q| q == lookup2.data.query())
            );

            assert!(lru_cache.contains(lookup1.data.query()));
            assert!(lru_cache.contains(lookup2.data.query()));
        };

        let res = cache.get(lookup1.data.query(), now);

        assert!(res.is_some());

        let (lookup, _) = res.unwrap();

        assert_eq!(lookup.query(), lookup1.data.query());
        assert_eq!(lookup.records(), lookup1.data.records());
    }

    #[tokio::test]
    async fn test_cache_record_ordering() {
        let query = Query::query("www.vscode-unpkg.net.".parse().unwrap(), RecordType::A);
        let records = [
            Record::from_rdata(
                "www.vscode-unpkg.net.".parse().unwrap(),
                2028,
                RData::CNAME(CNAME(
                    "vscode-unpkg-gvgaavacadd3anb4.z01.azurefd.net."
                        .parse()
                        .unwrap(),
                )),
            ),
            Record::from_rdata(
                "vscode-unpkg-gvgaavacadd3anb4.z01.azurefd.net."
                    .parse()
                    .unwrap(),
                2,
                RData::CNAME(CNAME(
                    "star-azurefd-prod.trafficmanager.net.".parse().unwrap(),
                )),
            ),
            Record::from_rdata(
                "star-azurefd-prod.trafficmanager.net.".parse().unwrap(),
                32,
                RData::CNAME(CNAME(
                    "shed.dual-low.s-part-0031.t-0009.t-msedge.net."
                        .parse()
                        .unwrap(),
                )),
            ),
            Record::from_rdata(
                "shed.dual-low.s-part-0031.t-0009.t-msedge.net."
                    .parse()
                    .unwrap(),
                32,
                RData::CNAME(CNAME("s-part-0031.t-0009.t-msedge.net.".parse().unwrap())),
            ),
            Record::from_rdata(
                "s-part-0031.t-0009.t-msedge.net.".parse().unwrap(),
                32,
                RData::A(A("13.107.246.59".parse().unwrap())),
            ),
        ];

        let cache = DnsCache::new(10, true, 30, 5);

        let now = Instant::now();

        cache.insert_response(
            DnsResponse::new_with_max_ttl(query.clone(), records.clone()),
            now,
            2,
            "default",
        );

        assert!(cache.get(&query, now).unwrap().0.records() == records);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_valid_cache_hits_do_not_start_background_queries() {
        use crate::dns_mw::DnsMiddlewareBuilder;
        use futures_util::future::join_all;
        use std::sync::Arc;

        let cfg = Arc::new(RuntimeConfig::default());
        assert!(!cfg.prefetch_domain());

        let (background_rx, background_handle) = DnsHandle::mock();
        let cache_middleware = DnsCacheMiddleware::new(&cfg, background_handle);
        let cache = cache_middleware.cache().clone();
        let upstream_queries = Arc::new(AtomicUsize::new(0));
        let handler = DnsMiddlewareBuilder::new()
            .with(cache_middleware)
            .with(CountingUpstream {
                queries: upstream_queries.clone(),
            })
            .build(cfg);

        let name: Name = "cache-hit.example.".parse().unwrap();
        let query = Query::query(name.clone(), RecordType::A);

        handler.lookup(name.clone(), RecordType::A).await.unwrap();
        assert_eq!(upstream_queries.load(Ordering::SeqCst), 1);

        assert!(cache.get(&query, Instant::now()).is_some());

        let receiver = tokio::spawn(count_background_queries(background_rx));
        let requests = (0..128).map(|_| handler.lookup(name.clone(), RecordType::A));
        let responses = join_all(requests).await;

        for response in responses {
            let response = response.unwrap();
            assert_eq!(response.query(), &query);
            assert_eq!(response.records().len(), 1);
            assert_eq!(
                response.records()[0].data(),
                &RData::A("192.0.2.1".parse().unwrap())
            );
        }
        assert_eq!(upstream_queries.load(Ordering::SeqCst), 1);
        assert_eq!(receiver.await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_expired_cache_hit_still_starts_background_refresh() {
        use crate::dns_mw::DnsMiddlewareBuilder;

        let cfg = Arc::new(RuntimeConfig::default());
        let (mut background_rx, background_handle) = DnsHandle::mock();
        let cache_middleware = DnsCacheMiddleware::new(&cfg, background_handle);
        let query = Query::query("expired.example.".parse().unwrap(), RecordType::A);
        let record = Record::from_rdata(
            query.name().clone(),
            30,
            RData::A("192.0.2.1".parse().unwrap()),
        );
        cache_middleware.cache().insert_response(
            DnsResponse::new_with_max_ttl(query.clone(), [record]),
            Instant::now() - Duration::from_secs(31),
            30,
            "default",
        );
        let upstream_queries = Arc::new(AtomicUsize::new(0));
        let handler = DnsMiddlewareBuilder::new()
            .with(cache_middleware)
            .with(CountingUpstream {
                queries: upstream_queries.clone(),
            })
            .build(cfg);

        let response = handler
            .lookup(query.name().clone(), RecordType::A)
            .await
            .unwrap();
        assert_eq!(response.records().len(), 1);
        assert_eq!(response.records()[0].ttl(), 5);
        assert_eq!(
            response.records()[0].data(),
            &RData::A("192.0.2.1".parse().unwrap())
        );
        assert_eq!(upstream_queries.load(Ordering::SeqCst), 0);
        let (message, opts, _) = tokio::time::timeout(Duration::from_secs(1), background_rx.recv())
            .await
            .expect("an expired answer must schedule a refresh")
            .unwrap();
        assert!(opts.is_background);
        assert_eq!(
            DnsRequest::try_from(message).unwrap().query().original(),
            &query
        );
    }

    #[tokio::test]
    async fn test_refreshed_entry_can_be_prefetched_again() {
        let cache = DnsCache::new(10, true, 0, 5);
        let query = Query::query("prefetch.example.".parse().unwrap(), RecordType::A);
        let record = Record::from_rdata(
            query.name().clone(),
            30,
            RData::A("192.0.2.1".parse().unwrap()),
        );
        let now = Instant::now();
        cache.insert_response(
            DnsResponse::new_with_max_ttl(query.clone(), [record.clone()]),
            now,
            30,
            "default",
        );

        let first_refresh = now + Duration::from_secs(31);
        let (expired, _) = cache.get_expired(first_refresh, Some(0));
        assert_eq!(expired, vec![(query.clone(), Some("default".into()))]);
        assert!(cache.get_expired(first_refresh, Some(0)).0.is_empty());

        cache.insert_response(
            DnsResponse::new_with_max_ttl(query.clone(), [record]),
            first_refresh,
            30,
            "default",
        );
        let (response, status) = cache.get(&query, first_refresh).unwrap();
        assert!(matches!(status, CacheStatus::Valid));
        assert_eq!(response.records()[0].ttl(), 30);
        assert_eq!(
            response.records()[0].data(),
            &RData::A("192.0.2.1".parse().unwrap())
        );
        let (expired, _) = cache.get_expired(first_refresh + Duration::from_secs(31), Some(0));
        assert_eq!(expired, vec![(query, Some("default".into()))]);
    }

    #[tokio::test]
    async fn test_failed_prefetch_retries_after_configured_interval() {
        let mut cache = DnsCache::new(10, true, 3600, 5);
        cache.prefetch_time = 30;
        let query = Query::query("retry.example.".parse().unwrap(), RecordType::A);
        let record = Record::from_rdata(
            query.name().clone(),
            10,
            RData::A("192.0.2.1".parse().unwrap()),
        );
        let now = Instant::now();
        cache.insert_response(
            DnsResponse::new_with_max_ttl(query.clone(), [record]),
            now,
            10,
            "default",
        );
        let first = now + Duration::from_secs(41);
        assert_eq!(cache.get_expired(first, Some(0)).0.len(), 1);
        cache.finish_prefetch(&query, first);
        assert!(
            cache
                .get_expired(first + Duration::from_secs(29), Some(0))
                .0
                .is_empty()
        );
        assert_eq!(
            cache
                .get_expired(first + Duration::from_secs(30), Some(0))
                .0,
            vec![(query, Some("default".into()))]
        );
    }

    #[tokio::test]
    async fn test_prefetch_limits_in_flight_queries_and_drains_the_queue() {
        let (mut receiver, client) = DnsHandle::mock();
        let domains = (0..40)
            .map(|i| {
                (
                    Query::query(
                        format!("prefetch{i}.example.").parse().unwrap(),
                        RecordType::A,
                    ),
                    Some("prefetch-group".to_owned()),
                )
            })
            .collect();
        let started = Instant::now();
        let worker = tokio::spawn(async move {
            let cache = DnsCache::new(40, true, 0, 5);
            prefetch_domains(&client, &cache, domains).await
        });

        let mut pending = Vec::new();
        for _ in 0..16 {
            let request = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(request.1.is_background);
            assert_eq!(request.1.rule_group.as_deref(), Some("prefetch-group"));
            pending.push(request);
        }
        tokio::task::yield_now().await;
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));

        for (message, _, sender) in pending {
            let request = DnsRequest::try_from(message).unwrap();
            assert!(sender.send(request.to_response().into()).is_ok());
        }
        for _ in 16..40 {
            let (message, _, sender) =
                tokio::time::timeout(Duration::from_secs(2), receiver.recv())
                    .await
                    .unwrap()
                    .unwrap();
            let request = DnsRequest::try_from(message).unwrap();
            assert!(sender.send(request.to_response().into()).is_ok());
        }
        assert!(started.elapsed() >= Duration::from_millis(1900));
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .expect("all queued domains should finish without another incoming query")
            .unwrap();
    }

    async fn count_background_queries(
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<crate::server::IncomingDnsMessage>,
    ) -> usize {
        let deadline = Instant::now() + Duration::from_millis(100);
        let mut count = 0;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return count;
            }

            match tokio::time::timeout(remaining, receiver.recv()).await {
                Ok(Some(_)) => count += 1,
                Ok(None) | Err(_) => return count,
            }
        }
    }
}
