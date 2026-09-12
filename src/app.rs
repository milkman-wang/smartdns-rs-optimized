use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{RwLock, Semaphore};

use crate::{
    config::ServerOpts,
    dns::{DnsRequest, DnsResponse, SerialMessage},
    dns_client::DnsClient,
    dns_conf::RuntimeConfig,
    dns_mw::{DnsMiddlewareBuilder, DnsMiddlewareHandler},
    dns_mw_cache::DnsCache,
    log,
    server::{DnsHandle, ServerHandle},
    third_ext::FutureJoinAllExt as _,
};

#[derive(Clone)]
pub struct App(Arc<AppState>);

impl App {
    pub(crate) fn new(cfg: Arc<RuntimeConfig>) -> Self {
        let handler = DnsMiddlewareBuilder::new().build(cfg.clone());
        let dispatcher = Arc::new(QueryDispatcher::new(Arc::new(handler)));
        let dns_handle = DnsHandle::new(Arc::downgrade(&dispatcher));
        Self(
            AppState {
                dns_handle,
                cfg: RwLock::new(cfg),
                dispatcher,
                listeners: Default::default(),
                #[cfg(feature = "webui")]
                webui_server: RwLock::new(None),
                cache: RwLock::const_new(None),
                uptime: Instant::now(),
                loaded_at: RwLock::const_new(Instant::now()),
                guard: AppGuard,
            }
            .into(),
        )
    }

    pub async fn cache(&self) -> Option<Arc<DnsCache>> {
        self.cache.read().await.clone()
    }

    pub fn dns_handle(&self) -> DnsHandle {
        self.dns_handle.clone()
    }

    pub async fn query(
        &self,
        request: &DnsRequest,
        options: &ServerOpts,
    ) -> Result<DnsResponse, crate::dns::DnsError> {
        let handler = self.dispatcher.handler();
        handler.search(request, options).await
    }

    pub async fn cfg(&self) -> Arc<RuntimeConfig> {
        self.cfg.read().await.clone()
    }

    pub async fn reload(&self) -> anyhow::Result<()> {
        log::info!("reloading configuration...");
        let cfg = self.cfg().await;
        let replacement = cfg.reload_new()?;
        anyhow::ensure!(
            cfg.webui_enabled() == replacement.webui_enabled()
                && cfg.webui_address() == replacement.webui_address(),
            "changing the WebUI listener requires a service restart"
        );
        let cfg = replacement;
        crate::infra::packet_debug::configure(
            cfg.debug_save_fail_packet,
            cfg.debug_save_fail_packet_dir.as_deref(),
        );
        *self.cfg.write().await = cfg;
        self.update_middleware_handler().await;
        self.update_listeners().await;
        *self.loaded_at.write().await = Instant::now();
        log::info!("configuration reloaded");
        Ok(())
    }

    pub async fn loaded_at(&self) -> Duration {
        let now = Instant::now();
        now.duration_since(*self.loaded_at.read().await)
    }

    pub fn uptime(&self) -> Duration {
        let now = Instant::now();
        now.duration_since(self.uptime)
    }

    pub fn active_queries(&self) -> usize {
        self.dispatcher.active_queries.load(Ordering::Relaxed)
    }

    async fn init(&self) -> anyhow::Result<()> {
        let cfg = self.cfg().await;
        anyhow::ensure!(
            cfg!(feature = "webui") || !cfg.webui_enabled(),
            "WebUI is not included in this headless build; install the WebUI version"
        );
        self.update_middleware_handler().await;
        crate::plugins::initialize(self.cfg().await, self.clone()).await?;
        #[cfg(feature = "webui")]
        if cfg.webui_enabled() {
            *self.webui_server.write().await =
                Some(crate::webui::serve(self.clone(), cfg.webui_address()).await?);
        }
        self.update_listeners().await;
        crate::banner();
        log::info!("awaiting connections...");
        log::info!("server starting up");
        Ok(())
    }

    async fn update_listeners(&self) {
        use crate::server;

        let cfg = self.cfg().await;

        let (new_bind_addrs, shutdowns) = {
            let listeners = self.listeners.read().await;
            let new_bind_addrs = cfg
                .binds()
                .iter()
                .filter(|l| !listeners.contains_key(l))
                .collect::<Vec<_>>();

            let shutdowns = listeners
                .keys()
                .filter(|l| !cfg.binds().contains(l))
                .cloned()
                .collect::<Vec<_>>();

            (new_bind_addrs, shutdowns)
        };

        if !shutdowns.is_empty() {
            let mut listeners = self.listeners.write().await;
            let shutdowns = shutdowns
                .iter()
                .flat_map(|k| listeners.remove(k))
                .collect::<Vec<_>>();
            tokio::spawn(async move {
                for shutdown in shutdowns {
                    shutdown.shutdown().await;
                }
            });
        }

        if !new_bind_addrs.is_empty() {
            let dns_handle = &self.dns_handle;

            let idle_time = cfg.tcp_idle_time();

            for bind_addr in new_bind_addrs {
                let serve_handle = server::serve(self, &cfg, bind_addr, dns_handle, idle_time);

                match serve_handle {
                    Ok(server) => {
                        if let Some(prev_server) = self
                            .listeners
                            .write()
                            .await
                            .insert(bind_addr.clone(), server)
                        {
                            tokio::spawn(async move {
                                prev_server.shutdown().await;
                            });
                        }
                    }
                    Err(err) => {
                        log::error!("{}", err)
                    }
                }
            }
        }
    }

    async fn update_middleware_handler(&self) {
        let cfg = self.cfg.read().await.clone();
        let mut cache = self.cache.write().await;
        let middleware_handler = build_middleware(
            &cfg,
            &self.dns_handle,
            cfg.create_dns_client().await,
            &mut cache,
        );

        *self.dispatcher.handler.write().unwrap() = middleware_handler;
    }
}

impl std::ops::Deref for App {
    type Target = AppState;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

pub struct AppState {
    cfg: RwLock<Arc<RuntimeConfig>>,
    dispatcher: Arc<QueryDispatcher>,
    dns_handle: DnsHandle,
    listeners: RwLock<HashMap<crate::config::BindAddrConfig, ServerHandle>>,
    #[cfg(feature = "webui")]
    webui_server: RwLock<Option<crate::webui::Server>>,
    cache: RwLock<Option<Arc<DnsCache>>>,
    uptime: Instant,
    loaded_at: RwLock<Instant>,
    guard: AppGuard,
}

/// Shared execution state; transports run the middleware in their request task.
/// Background handles hold a weak reference so cached prefetch state cannot
/// keep an obsolete dispatcher alive after shutdown.
pub(crate) struct QueryDispatcher {
    handler: std::sync::RwLock<Arc<DnsMiddlewareHandler>>,
    active_queries: AtomicUsize,
    started: Instant,
    idle_deadline_ms: AtomicU64,
    background_concurrency: Semaphore,
}

impl QueryDispatcher {
    const MAX_IDLE_MS: u64 = 30 * 60 * 1000;

    fn new(handler: Arc<DnsMiddlewareHandler>) -> Self {
        Self {
            handler: std::sync::RwLock::new(handler),
            active_queries: AtomicUsize::new(0),
            started: Instant::now(),
            idle_deadline_ms: AtomicU64::new(Self::MAX_IDLE_MS),
            background_concurrency: Semaphore::new(16),
        }
    }

    fn handler(&self) -> Arc<DnsMiddlewareHandler> {
        self.handler.read().unwrap().clone()
    }

    pub(crate) async fn send(&self, message: SerialMessage, opts: &ServerOpts) -> SerialMessage {
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        let _permit = if opts.is_background {
            if elapsed_ms >= self.idle_deadline_ms.load(Ordering::Relaxed) {
                return crate::server::refused_response(message);
            }
            Some(self.background_concurrency.acquire().await.unwrap())
        } else {
            self.idle_deadline_ms
                .fetch_max(elapsed_ms + Self::MAX_IDLE_MS, Ordering::Relaxed);
            None
        };
        let _active = ActiveQueryGuard::new(&self.active_queries);
        process(self.handler(), message, opts).await
    }
}

struct ActiveQueryGuard<'a>(&'a AtomicUsize);
impl<'a> ActiveQueryGuard<'a> {
    fn new(active: &'a AtomicUsize) -> Self {
        active.fetch_add(1, Ordering::Relaxed);
        Self(active)
    }
}
impl Drop for ActiveQueryGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn serve(cfg: Arc<RuntimeConfig>) {
    crate::signal::reset();
    crate::infra::packet_debug::configure(
        cfg.debug_save_fail_packet,
        cfg.debug_save_fail_packet_dir.as_deref(),
    );
    let app = App::new(cfg.clone());

    let log_dispatch = log::make_dispatch(
        cfg.log_file(),
        cfg.log_enabled(),
        cfg.log_level(),
        cfg.log_filter(),
        cfg.log_size(),
        cfg.log_num(),
        cfg.log_file_mode().into(),
        cfg.log_config().console(),
        cfg.log_config().syslog.unwrap_or(false),
    );

    let _main_log_guard = log::set_default(&log_dispatch);
    let mut runtime = if cfg.num_workers() == 1 {
        tokio::runtime::Builder::new_current_thread()
    } else {
        let mut runtime = tokio::runtime::Builder::new_multi_thread();
        runtime.worker_threads(cfg.num_workers());
        runtime
    };
    let runtime = runtime
        .enable_all()
        .thread_name("smartdns-runtime")
        .on_thread_start(move || {
            log::LOG_GUARD.replace(Some(log::set_default(&log_dispatch)));
        })
        .on_thread_stop(move || {
            log::LOG_GUARD.take();
        })
        .build()
        .expect("failed to initialize Tokio Runtime");

    let _guard = runtime.enter();

    runtime
        .block_on(app.init())
        .expect("failed to initialize SmartDNS");

    let shutdown_timeout = Duration::from_secs(5);

    runtime.block_on(async move {
        use crate::signal;
        let _ = signal::terminate().await;
        // close all servers.
        #[cfg(feature = "webui")]
        if let Some(server) = app.webui_server.write().await.take() {
            server.shutdown().await;
        }
        let mut shutdown_listeners = Default::default();
        std::mem::swap(
            app.listeners.write().await.deref_mut(),
            &mut shutdown_listeners,
        );
        shutdown_listeners
            .into_values()
            .map(|server| server.shutdown())
            .join_all()
            .await;
    });

    runtime.shutdown_timeout(shutdown_timeout);
    crate::plugins::shutdown();
}

struct AppGuard;

async fn process(
    handler: Arc<DnsMiddlewareHandler>,
    message: SerialMessage,
    server_opts: &ServerOpts,
) -> SerialMessage {
    use crate::libdns::proto::ProtoError;
    use crate::libdns::proto::op::{Header, Message, MessageType, OpCode, ResponseCode};

    let addr = message.addr();
    let protocol = message.protocol();

    match DnsRequest::try_from(message) {
        Ok(request) => {
            match request.message_type() {
                MessageType::Query => {
                    match request.op_code() {
                        OpCode::Query => {
                            // start process
                            let request_header = request.header();
                            let mut response_header = Header::response_from_request(request_header);

                            response_header.set_recursion_available(true);
                            response_header.set_authoritative(false);

                            let response = {
                                let start = Instant::now();
                                let res = handler.search(&request, server_opts).await;

                                log::debug!(
                                    "{}Request: {:?}",
                                    if server_opts.is_background {
                                        "Background"
                                    } else {
                                        ""
                                    },
                                    request
                                );
                                match res {
                                    Ok(lookup) => {
                                        log::debug!(
                                            "Response: {}, Duration: {:?}",
                                            lookup.deref(),
                                            start.elapsed()
                                        );
                                        lookup
                                    }
                                    Err(e) => {
                                        if e.is_nx_domain() {
                                            log::debug!(
                                                "{}Response: error resolving: NXDomain, Duration: {:?}",
                                                if server_opts.is_background {
                                                    "Background"
                                                } else {
                                                    ""
                                                },
                                                start.elapsed()
                                            );
                                            response_header
                                                .set_response_code(ResponseCode::NXDomain);
                                        }
                                        let original = request.query().original();
                                        match e.as_no_records_response(original) {
                                            Some(response) => response,
                                            None => {
                                                log::debug!(
                                                    "{}Response: error resolving: {}, Duration: {:?}",
                                                    if server_opts.is_background {
                                                        "Background"
                                                    } else {
                                                        ""
                                                    },
                                                    e,
                                                    start.elapsed()
                                                );
                                                response_header.set_response_code(
                                                    if e.is_nx_domain() {
                                                        ResponseCode::NXDomain
                                                    } else {
                                                        ResponseCode::ServFail
                                                    },
                                                );
                                                let mut res = DnsResponse::empty();
                                                res.add_query(original.to_owned());
                                                res
                                            }
                                        }
                                    }
                                }
                            };

                            SerialMessage::reply(response, response_header, addr, protocol)
                        }
                        OpCode::Status | OpCode::Notify | OpCode::Update | OpCode::Unknown(_) => {
                            let mut response = request.to_response();
                            response.set_response_code(ResponseCode::NotImp);
                            SerialMessage::raw(response, addr, protocol)
                        }
                    }
                }
                MessageType::Response => {
                    let mut response = request.to_response();
                    response.set_response_code(ResponseCode::FormErr);
                    SerialMessage::raw(response, addr, protocol)
                }
            }
        }
        Err(ProtoError { kind, .. }) if kind.as_form_error().is_some() => {
            // We failed to parse the request due to some issue in the message, but the header is available, so we can respond
            let (request_header, error) = kind
                .into_form_error()
                .expect("as form_error already confirmed this is a FormError");

            // debug for more info on why the message parsing failed
            log::debug!(
                "request:{id} src:{proto}://{addr}#{port} type:{message_type} {op}:FormError:{error}",
                id = request_header.id(),
                proto = protocol,
                addr = addr.ip(),
                port = addr.port(),
                message_type = request_header.message_type(),
                op = request_header.op_code(),
                error = error,
            );

            let mut response_header = Header::response_from_request(&request_header);
            response_header.set_response_code(ResponseCode::FormErr);
            let mut response_message = Message::query().to_response();
            response_message.set_header(response_header);
            SerialMessage::raw(response_message, addr, protocol)
        }
        _ => SerialMessage::raw(Message::query(), addr, protocol),
    }
}

fn build_middleware(
    cfg: &Arc<RuntimeConfig>,
    dns_handle: &DnsHandle,
    dns_client: DnsClient,
    dns_cache: &mut Option<Arc<DnsCache>>,
) -> Arc<DnsMiddlewareHandler> {
    use crate::dns_mw_addr::AddressMiddleware;
    use crate::dns_mw_audit::DnsAuditMiddleware;
    use crate::dns_mw_bogus::DnsBogusMiddleware;
    use crate::dns_mw_cache::DnsCacheMiddleware;
    use crate::dns_mw_cname::DnsCNameMiddleware;
    use crate::dns_mw_dns64::Dns64Middleware;
    use crate::dns_mw_dnsmasq::DnsmasqMiddleware;
    use crate::dns_mw_dualstack::DnsDualStackIpSelectionMiddleware;
    use crate::dns_mw_hosts::DnsHostsMiddleware;
    use crate::dns_mw_ns::NameServerMiddleware;
    use crate::dns_mw_zone::DnsZoneMiddleware;

    let middleware_handler = {
        let mut builder = DnsMiddlewareBuilder::new();

        // Observe final TTLs and local/cache responses as well as upstream answers.
        if crate::dns_mw_nftset::DnsNftsetMiddleware::is_configured(cfg) {
            builder = builder.with(crate::dns_mw_nftset::DnsNftsetMiddleware);
        }

        // check if audit enabled.
        if cfg.audit_enable() {
            builder = builder.with(DnsAuditMiddleware::new(cfg));
        }

        if cfg.rule_groups().values().any(|x| !x.cnames.is_empty()) {
            builder = builder.with(DnsCNameMiddleware);
        }

        if let Some(dns64_prefix) = cfg.dns64_prefix {
            builder = builder.with(Dns64Middleware::new(dns64_prefix));
        }

        builder = builder.with(AddressMiddleware);

        if cfg
            .dnsmasq_lease_file()
            .map(|x| x.is_file())
            .unwrap_or_default()
        {
            builder = builder.with(DnsmasqMiddleware::new(
                cfg.dnsmasq_lease_file().unwrap(),
                cfg.domain().cloned(),
            ));
        }

        builder = builder.with(DnsZoneMiddleware::new());

        if let Some(path) = cfg.odhcpd_lease_file.as_ref() {
            builder = builder.with(crate::dns_mw_odhcpd::OdhcpdMiddleware::new(
                path.clone(),
                cfg.domain().cloned(),
            ));
        }

        if cfg.resolv_hostanme() {
            builder = builder.with(DnsHostsMiddleware::new());
        }

        // check if cache enabled.
        if cfg.cache_size() > 0 {
            let cache_middleware = DnsCacheMiddleware::new(cfg, dns_handle.clone());
            dns_cache.replace(cache_middleware.cache().clone());
            builder = builder.with(cache_middleware);
        }

        if DnsDualStackIpSelectionMiddleware::is_configured(cfg) {
            builder = builder.with(DnsDualStackIpSelectionMiddleware);
        }

        if !cfg.bogus_nxdomain().is_empty() {
            builder = builder.with(DnsBogusMiddleware);
        }

        builder = builder.with(NameServerMiddleware::new(dns_client));

        builder.build(cfg.clone())
    };

    Arc::new(middleware_handler)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::{RData, Record, RecordType};
    use crate::dns_error::LookupError;
    use crate::dns_mw::DnsMockMiddleware;
    use crate::libdns::proto::{
        AuthorityData, NoRecords, ProtoErrorKind,
        op::{Message, MessageType, OpCode, Query, ResponseCode},
        rr::rdata::{CNAME, SOA},
    };

    #[tokio::test]
    async fn direct_dispatch_keeps_reload_and_shutdown_visible_to_existing_handles() {
        let app = App::new(Arc::new(RuntimeConfig::default()));
        let handle = app.dns_handle();
        let query = Query::query("reload.example.".parse().unwrap(), RecordType::A);
        let mut request = Message::new(2468, MessageType::Query, OpCode::Query);
        request.add_query(query.clone());
        for ip in ["192.0.2.1", "192.0.2.2"] {
            *app.dispatcher.handler.write().unwrap() = Arc::new(
                DnsMockMiddleware::builder()
                    .with_a_record(query.name().clone(), ip.parse().unwrap())
                    .build(RuntimeConfig::default()),
            );
            let response = Message::try_from(handle.send(request.clone()).await).unwrap();
            assert_eq!(response.id(), 2468);
            assert_eq!(
                response.answers()[0].data().ip_addr(),
                Some(ip.parse().unwrap())
            );
            assert_eq!(app.active_queries(), 0);
        }
        drop(app);
        let response = Message::try_from(handle.send(request).await).unwrap();
        assert_eq!(response.id(), 2468);
        assert_eq!(response.queries(), &[query]);
        assert_eq!(response.response_code(), ResponseCode::Refused);
    }

    struct WaitingMiddleware(Arc<tokio::sync::Notify>);

    #[async_trait::async_trait]
    impl
        crate::middleware::Middleware<
            crate::dns::DnsContext,
            DnsRequest,
            DnsResponse,
            crate::dns::DnsError,
        > for WaitingMiddleware
    {
        async fn handle(
            &self,
            _: &mut crate::dns::DnsContext,
            _: &DnsRequest,
            _: crate::middleware::Next<
                '_,
                crate::dns::DnsContext,
                DnsRequest,
                DnsResponse,
                crate::dns::DnsError,
            >,
        ) -> Result<DnsResponse, crate::dns::DnsError> {
            self.0.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn cancelled_direct_query_releases_activity_and_background_permit() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let dispatcher = Arc::new(QueryDispatcher::new(Arc::new(
            DnsMiddlewareBuilder::new()
                .with(WaitingMiddleware(entered.clone()))
                .build(Arc::new(RuntimeConfig::default())),
        )));
        let handle = DnsHandle::new(Arc::downgrade(&dispatcher)).with_new_opt(ServerOpts {
            is_background: true,
            ..Default::default()
        });
        let task = tokio::spawn(async move {
            let mut request = Message::query();
            request.add_query(Query::query(
                "wait.example.".parse().unwrap(),
                RecordType::A,
            ));
            handle.send(request).await
        });
        entered.notified().await;
        assert_eq!(dispatcher.active_queries.load(Ordering::Relaxed), 1);
        assert_eq!(dispatcher.background_concurrency.available_permits(), 15);
        task.abort();
        let _ = task.await;
        assert_eq!(dispatcher.active_queries.load(Ordering::Relaxed), 0);
        assert_eq!(dispatcher.background_concurrency.available_permits(), 16);
    }

    #[tokio::test]
    async fn foreground_activity_resumes_prefetch_after_idle_timeout() {
        let query = Query::query("idle.example.".parse().unwrap(), RecordType::A);
        let dispatcher = QueryDispatcher::new(Arc::new(
            DnsMockMiddleware::builder()
                .with_a_record(query.name().clone(), "192.0.2.3".parse().unwrap())
                .build(RuntimeConfig::default()),
        ));
        dispatcher.idle_deadline_ms.store(0, Ordering::Relaxed);
        let dispatcher = Arc::new(dispatcher);
        let foreground = DnsHandle::new(Arc::downgrade(&dispatcher));
        let background = foreground.with_new_opt(ServerOpts {
            is_background: true,
            ..Default::default()
        });
        let mut request = Message::query();
        request.add_query(query);
        let response = Message::try_from(background.send(request.clone()).await).unwrap();
        assert_eq!(response.response_code(), ResponseCode::Refused);
        for handle in [&foreground, &background] {
            let response = Message::try_from(handle.send(request.clone()).await).unwrap();
            assert_eq!(
                response.answers()[0].data().ip_addr(),
                Some("192.0.2.3".parse().unwrap())
            );
        }
    }

    #[tokio::test]
    async fn test_query_final_packet_preserves_response_codes() {
        let query = Query::query("alias.example.".parse().unwrap(), RecordType::A);
        let mut cases: Vec<(ResponseCode, Result<DnsResponse, LookupError>)> = Vec::new();
        for code in [
            ResponseCode::NoError,
            ResponseCode::NXDomain,
            ResponseCode::ServFail,
            ResponseCode::Refused,
        ] {
            let authority = AuthorityData::new(Box::new(query.clone()), None, true, false, None);
            let mut no_records: NoRecords = authority.into();
            no_records.response_code = code;
            cases.push((code, Err(ProtoErrorKind::NoRecordsFound(no_records).into())));
        }
        cases.push((ResponseCode::ServFail, Err(ProtoErrorKind::Timeout.into())));
        cases.push((ResponseCode::NXDomain, Err(ResponseCode::NXDomain.into())));
        cases.push((ResponseCode::Refused, Err(ResponseCode::Refused.into())));

        let mut response = DnsResponse::new_with_max_ttl(
            query.clone(),
            [Record::from_rdata(
                query.name().clone(),
                30,
                RData::CNAME(CNAME("missing.example.".parse().unwrap())),
            )],
        );
        response.set_response_code(ResponseCode::NXDomain);
        response.add_authority(Record::from_rdata(
            "example.".parse().unwrap(),
            30,
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
        cases.push((ResponseCode::NXDomain, Ok(response)));

        let mut truncated = DnsResponse::new_with_max_ttl(
            query.clone(),
            [Record::from_rdata(
                query.name().clone(),
                30,
                "192.0.2.1".parse::<std::net::IpAddr>().unwrap().into(),
            )],
        );
        truncated.set_truncated(true);
        cases.push((ResponseCode::NoError, Ok(truncated)));

        for (code, result) in cases {
            let truncated = result.as_ref().is_ok_and(|r| r.truncated());
            let answers = result
                .as_ref()
                .map(|r| r.answers().to_vec())
                .unwrap_or_default();
            let authorities = result
                .as_ref()
                .map(|r| r.authorities().to_vec())
                .unwrap_or_default();
            let handler = Arc::new(
                DnsMockMiddleware::builder()
                    .with_result(query.clone(), result)
                    .build(RuntimeConfig::default()),
            );
            let mut request = Message::new(1234, MessageType::Query, OpCode::Query);
            request.set_recursion_desired(true);
            request.add_query(query.clone());
            let packet = process(handler, request.into(), &ServerOpts::default()).await;
            let bytes: Vec<u8> = packet.try_into().unwrap();
            let response = Message::from_vec(&bytes).unwrap();
            assert_eq!(response.id(), 1234);
            assert_eq!(response.response_code(), code);
            assert_eq!(response.truncated(), truncated);
            assert_eq!(response.queries(), std::slice::from_ref(&query));
            assert_eq!(response.answers(), answers);
            assert_eq!(response.authorities(), authorities);
        }
    }
}
