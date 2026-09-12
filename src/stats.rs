use crate::{
    config::NameServerInfo,
    dns::{DnsContext, DnsError, DnsRequest, DnsResponse, LookupFrom, RecordType},
    libdns::proto::op::ResponseCode,
};
use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
pub struct QueryStatistics {
    pub total: AtomicU64,
    pub success: AtomicU64,
    pub received: AtomicU64,
    pub elapsed_micros: AtomicU64,
}

impl QueryStatistics {
    pub fn average_ms(&self) -> f32 {
        let count = self.received.load(Ordering::Relaxed);
        if count == 0 {
            0.0
        } else {
            self.elapsed_micros.load(Ordering::Relaxed) as f32 / count as f32 / 1000.0
        }
    }
}

pub static REQUESTS: QueryStatistics = QueryStatistics {
    total: AtomicU64::new(0),
    success: AtomicU64::new(0),
    received: AtomicU64::new(0),
    elapsed_micros: AtomicU64::new(0),
};
pub static FROM_CLIENT: AtomicU64 = AtomicU64::new(0);
pub static BLOCKED: AtomicU64 = AtomicU64::new(0);
pub static CACHE_CHECKS: AtomicU64 = AtomicU64::new(0);
pub static CACHE_HITS: AtomicU64 = AtomicU64::new(0);

pub fn blocked(ctx: &DnsContext, result: &Result<DnsResponse, DnsError>) -> bool {
    match result {
        Ok(response) => {
            matches!(ctx.source, LookupFrom::Static | LookupFrom::Zone(_))
                && response
                    .answers()
                    .iter()
                    .chain(response.authorities())
                    .any(|r| r.record_type() == RecordType::SOA)
        }
        Err(crate::dns_error::LookupError::ResponseCode(ResponseCode::NXDomain)) => true,
        _ => false,
    }
}

pub fn completed(
    ctx: &DnsContext,
    req: &DnsRequest,
    result: &Result<DnsResponse, DnsError>,
    duration: Duration,
) {
    REQUESTS.total.fetch_add(1, Ordering::Relaxed);
    REQUESTS.received.fetch_add(1, Ordering::Relaxed);
    REQUESTS
        .elapsed_micros
        .fetch_add(duration.as_micros() as u64, Ordering::Relaxed);
    if result.is_ok() || result.as_ref().is_err_and(|e| e.is_negative_response()) {
        REQUESTS.success.fetch_add(1, Ordering::Relaxed);
    }
    if !ctx.server_opts.is_background && !ctx.is_dualstack && req.src().port() != 0 {
        FROM_CLIENT.fetch_add(1, Ordering::Relaxed);
    }
    if blocked(ctx, result) {
        BLOCKED.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct Upstream {
    pub config: NameServerInfo,
    pub host: String,
    pub ip: String,
    pub stats: QueryStatistics,
    pub alive: AtomicBool,
    pub registered: AtomicBool,
}

static UPSTREAMS: Mutex<Vec<Weak<Upstream>>> = Mutex::new(vec![]);

impl Upstream {
    pub fn new(config: NameServerInfo) -> Arc<Self> {
        let server = Arc::new(Self {
            host: config.server.host().to_string(),
            ip: config
                .server
                .ip()
                .map(|ip| ip.to_string())
                .unwrap_or_default(),
            config,
            stats: Default::default(),
            alive: AtomicBool::new(true),
            registered: AtomicBool::new(true),
        });
        let mut servers = UPSTREAMS.lock().unwrap();
        servers.retain(|server| server.strong_count() > 0);
        servers.push(Arc::downgrade(&server));
        server
    }

    pub fn completed(
        &self,
        result: &Result<DnsResponse, crate::libdns::proto::ProtoError>,
        elapsed: Duration,
    ) {
        let reachable = result.is_ok()
            || result
                .as_ref()
                .is_err_and(|e| e.is_nx_domain() || e.is_no_records_found());
        self.alive.store(reachable, Ordering::Relaxed);
        if reachable {
            self.stats.received.fetch_add(1, Ordering::Relaxed);
            self.stats
                .elapsed_micros
                .fetch_add(elapsed.as_micros() as u64, Ordering::Relaxed);
            self.stats.success.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn upstreams() -> Vec<Arc<Upstream>> {
    UPSTREAMS
        .lock()
        .unwrap()
        .iter()
        .filter_map(Weak::upgrade)
        .filter(|server| server.registered.load(Ordering::Relaxed))
        .collect()
}
