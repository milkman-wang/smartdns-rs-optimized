use std::{borrow::Borrow, net::IpAddr, sync::Arc};

use crate::libdns::proto::{
    op::Query,
    rr::{
        IntoName, RecordType,
        rdata::opt::{EdnsCode, EdnsOption},
    },
};

use crate::{
    config::ServerOpts,
    dns::{DnsContext, DnsError, DnsRequest, DnsResponse},
    dns_conf::RuntimeConfig,
    middleware::{Middleware, MiddlewareBuilder, MiddlewareDefaultHandler, MiddlewareHost},
};

pub type DnsMiddlewareHost = MiddlewareHost<DnsContext, DnsRequest, DnsResponse, DnsError>;

pub struct DnsMiddlewareHandler {
    cfg: Arc<RuntimeConfig>,
    host: DnsMiddlewareHost,
    query_limit: Option<Arc<tokio::sync::Semaphore>>,
}

impl DnsMiddlewareHandler {
    pub async fn search(
        &self,
        req: &DnsRequest,
        server_opts: &ServerOpts,
    ) -> Result<DnsResponse, DnsError> {
        let cfg = self.cfg.clone();
        let started = std::time::Instant::now();

        let _permit = if !server_opts.is_background {
            match &self.query_limit {
                Some(limit) => Some(limit.try_acquire().map_err(|_| {
                    DnsError::from(crate::libdns::proto::op::ResponseCode::Refused)
                })?),
                None => None,
            }
        } else {
            None
        };

        let mut server_opts = server_opts.clone();

        let client_subnet = req
            .extensions()
            .as_ref()
            .and_then(|s| s.option(EdnsCode::Subnet))
            .and_then(|s| match s {
                EdnsOption::Subnet(s) => Some(s),
                _ => None,
            })
            .map(|s| match s.addr() {
                std::net::IpAddr::V4(addr) => {
                    IpNet::V4(Ipv4Net::new(addr, s.source_prefix()).unwrap())
                }
                std::net::IpAddr::V6(addr) => match addr.to_ipv4_mapped() {
                    Some(addr) => IpNet::V4(Ipv4Net::new(addr, s.source_prefix()).unwrap()),
                    None => IpNet::V6(Ipv6Net::new(addr, s.source_prefix()).unwrap()),
                },
            });

        let client_rules = cfg.client_rules();
        let client_mac = if client_rules
            .iter()
            .any(|rule| matches!(rule.client, crate::config::Client::MacAddr(_)))
            || crate::plugins::wants_mac()
        {
            crate::infra::client_mac::lookup(req.src().ip()).await
        } else {
            None
        };
        let mac_rule = client_mac
            .as_deref()
            .and_then(|mac| client_rules.iter().find(|rule| rule.match_mac(mac)));
        let client_rule = mac_rule.or_else(|| match client_subnet {
            Some(subnet) => client_rules.iter().find(|s| s.match_net(&subnet)),
            None => {
                let mut client_ip = req.src().ip();

                if let IpAddr::V6(addr) = client_ip
                    && let Some(addr) = addr.to_ipv4_mapped()
                {
                    client_ip = addr.into();
                }

                client_rules.iter().find(|s| s.match_ip(&client_ip))
            }
        });

        if let Some(rule) = client_rule {
            let mut client_opts = rule.options.clone();
            client_opts.is_background = server_opts.is_background;
            client_opts.apply(server_opts);
            if !rule.group.is_empty() {
                client_opts.rule_group = Some(rule.group.clone());
            }
            server_opts = client_opts;
        }

        let mut ctx = DnsContext::new(req.query().name().borrow(), cfg, server_opts);
        let result = self.host.execute(&mut ctx, req).await;
        crate::stats::completed(&ctx, req, &result, started.elapsed());
        crate::plugins::completed(&ctx, req, &result, started, client_mac.as_deref());
        result
    }

    pub async fn lookup<N: IntoName>(
        &self,
        name: N,
        query_type: RecordType,
    ) -> Result<DnsResponse, DnsError> {
        let query = Query::query(name.into_name()?, query_type);
        self.search(&query.into(), &Default::default()).await
    }
}

pub struct DnsMiddlewareBuilder {
    builder: MiddlewareBuilder<DnsContext, DnsRequest, DnsResponse, DnsError>,
}

impl DnsMiddlewareBuilder {
    pub fn new() -> Self {
        Self {
            builder: MiddlewareBuilder::new(DnsDefaultHandler),
        }
    }

    pub fn with<M: Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> + 'static>(
        mut self,
        middleware: M,
    ) -> Self {
        self.builder = self.builder.with(middleware);
        self
    }

    pub fn build(self, cfg: Arc<RuntimeConfig>) -> DnsMiddlewareHandler {
        let query_limit = cfg
            .max_query_limit
            .filter(|limit| *limit > 0)
            .map(|limit| Arc::new(tokio::sync::Semaphore::new(limit)));
        DnsMiddlewareHandler {
            host: self.builder.build(),
            cfg,
            query_limit,
        }
    }
}

#[derive(Default)]
struct DnsDefaultHandler;

#[async_trait::async_trait]
impl MiddlewareDefaultHandler<DnsContext, DnsRequest, DnsResponse, DnsError> for DnsDefaultHandler {
    async fn handle(
        &self,
        ctx: &mut DnsContext,
        req: &DnsRequest,
    ) -> Result<DnsResponse, DnsError> {
        Err(DnsError::no_records_found(
            req.query().original().to_owned(),
            ctx.cfg().rr_ttl().unwrap_or_default() as u32,
        ))
    }
}

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
#[cfg(test)]
pub use tests::*;

#[cfg(test)]
mod tests {

    use crate::libdns::proto::rr::{RData, Record};
    use std::{
        collections::HashMap,
        fmt::Debug,
        net::{Ipv4Addr, Ipv6Addr},
    };

    use super::*;
    use crate::infra::middleware::*;

    pub struct DnsMockMiddleware {
        map: HashMap<Query, Result<DnsResponse, DnsError>>,
    }

    impl DnsMockMiddleware {
        #[inline]
        pub fn builder() -> DnsMockMiddlewareBuilder {
            DnsMockMiddlewareBuilder::new()
        }

        pub fn mock<M: Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> + 'static>(
            middleware: M,
        ) -> DnsMockMiddlewareBuilder {
            Self::builder().with_extra_middleware(middleware)
        }
    }

    #[async_trait::async_trait]
    impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for DnsMockMiddleware {
        async fn handle(
            &self,
            ctx: &mut DnsContext,
            req: &DnsRequest,
            next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
        ) -> Result<DnsResponse, DnsError> {
            match self.map.get(req.query().original()) {
                Some(res) => res.clone(),
                None => next.run(ctx, req).await,
            }
        }
    }

    pub struct DnsMockMiddlewareBuilder {
        map: HashMap<Query, Result<DnsResponse, DnsError>>,
        builder: DnsMiddlewareBuilder,
    }

    impl DnsMockMiddlewareBuilder {
        fn new() -> Self {
            Self {
                map: Default::default(),
                builder: DnsMiddlewareBuilder::new(),
            }
        }

        pub fn with_extra_middleware<
            M: Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> + 'static,
        >(
            mut self,
            middleware: M,
        ) -> Self {
            self.builder = self.builder.with(middleware);
            self
        }

        pub fn build<T: Into<Arc<RuntimeConfig>>>(self, cfg: T) -> DnsMiddlewareHandler {
            let Self { map, builder } = self;

            builder.with(DnsMockMiddleware { map }).build(cfg.into())
        }

        pub fn with_a_record<N: IntoName>(self, name: N, ip: Ipv4Addr) -> Self {
            self.with_rdata(name, RData::A(ip.into()), 10 * 60)
        }

        pub fn with_a_record_and_ttl<N: IntoName>(self, name: N, ip: Ipv4Addr, ttl: u32) -> Self {
            self.with_rdata(name, RData::A(ip.into()), ttl)
        }

        pub fn with_aaaa_record<N: IntoName>(self, name: N, ip: Ipv6Addr) -> Self {
            self.with_rdata(name, RData::AAAA(ip.into()), 10 * 60)
        }

        pub fn with_aaaa_record_and_ttl<N: IntoName>(
            self,
            name: N,
            ip: Ipv6Addr,
            ttl: u32,
        ) -> Self {
            self.with_rdata(name, RData::AAAA(ip.into()), ttl)
        }

        pub fn with_rdata<N: IntoName>(self, name: N, rdata: RData, ttl: u32) -> Self {
            let name = match name.into_name() {
                Ok(name) => name,
                Err(err) => panic!("invalid Name {err}"),
            };

            self.with_record(Record::from_rdata(name, ttl, rdata))
        }

        pub fn with_record(self, record: Record) -> Self {
            self.with_multi_records(record.name().clone(), record.record_type(), vec![record])
        }

        pub fn with_result(mut self, query: Query, result: Result<DnsResponse, DnsError>) -> Self {
            self.map.insert(query, result);
            self
        }

        pub fn with_multi_records<Name: IntoName + Debug>(
            mut self,
            name: Name,
            record_type: RecordType,
            records: Vec<Record>,
        ) -> Self {
            let name = match name.into_name() {
                Ok(name) => name,
                Err(err) => panic!("invalid Name {err}"),
            };

            let query = Query::query(name, record_type);

            self.map.insert(
                query.clone(),
                Ok(DnsResponse::new_with_max_ttl(query, records)),
            );

            self
        }
    }

    impl DnsMiddlewareHandler {
        pub async fn lookup_rdata<N: IntoName>(
            &self,
            name: N,
            query_type: RecordType,
        ) -> Result<Vec<RData>, DnsError> {
            self.lookup(name, query_type)
                .await
                .map(|lookup| lookup.record_iter().map(|s| s.data()).cloned().collect())
        }
    }

    #[tokio::test]
    async fn test_c_compat_foreground_query_limit_releases_capacity() {
        use crate::libdns::proto::op::ResponseCode;
        struct Gated {
            entered: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Semaphore>,
        }
        #[async_trait::async_trait]
        impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for Gated {
            async fn handle(
                &self,
                _ctx: &mut DnsContext,
                req: &DnsRequest,
                _next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
            ) -> Result<DnsResponse, DnsError> {
                self.entered.notify_one();
                let _permit = self.release.acquire().await.unwrap();
                Ok(DnsResponse::from_rdata(
                    req.query().original().clone(),
                    RData::A("192.0.2.1".parse().unwrap()),
                ))
            }
        }
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let cfg = RuntimeConfig::builder()
            .with("max-query-limit 1")
            .build()
            .unwrap();
        let handler = Arc::new(
            DnsMiddlewareBuilder::new()
                .with(Gated {
                    entered: entered.clone(),
                    release: release.clone(),
                })
                .build(Arc::new(cfg)),
        );
        let first = {
            let handler = handler.clone();
            tokio::spawn(async move { handler.lookup("one.example.", RecordType::A).await })
        };
        entered.notified().await;
        assert_eq!(
            handler
                .lookup("two.example.", RecordType::A)
                .await
                .unwrap_err(),
            DnsError::from(ResponseCode::Refused)
        );
        release.add_permits(1);
        assert_eq!(
            first.await.unwrap().unwrap().ip_addrs(),
            vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            handler
                .lookup("three.example.", RecordType::A)
                .await
                .unwrap()
                .ip_addrs(),
            vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn client_options_override_listener_defaults_and_preserve_request_metadata() {
        use crate::config::{ConfigForIP, KernelIpSet};
        struct CheckOptions {
            group: &'static str,
            background: bool,
        }
        #[async_trait::async_trait]
        impl Middleware<DnsContext, DnsRequest, DnsResponse, DnsError> for CheckOptions {
            async fn handle(
                &self,
                ctx: &mut DnsContext,
                req: &DnsRequest,
                _next: Next<'_, DnsContext, DnsRequest, DnsResponse, DnsError>,
            ) -> Result<DnsResponse, DnsError> {
                assert!(ctx.server_opts.no_cache());
                assert!(
                    matches!(&ctx.server_opts.ipset.as_ref().unwrap()[0], ConfigForIP::All(set) if set.0 == "client4")
                );
                assert_eq!(ctx.server_opts.rule_group.as_deref(), Some(self.group));
                assert_eq!(
                    ctx.server_opts.local_addr,
                    Some("127.0.0.1:5302".parse().unwrap())
                );
                assert_eq!(ctx.server_opts.is_background, self.background);
                assert_eq!(ctx.server_opts.no_dualstack_selection, Some(true));
                Ok(DnsResponse::from_rdata(
                    req.query().original().clone(),
                    RData::A("192.0.2.1".parse().unwrap()),
                ))
            }
        }
        for (group_option, expected_group) in [("", "default"), (" -group devices", "devices")] {
            for background in [false, true] {
                let cfg = RuntimeConfig::builder()
                    .with(&format!(
                        "client-rules 127.0.0.2 -ipset client4 -no-cache{group_option}"
                    ))
                    .build()
                    .unwrap();
                let handler = DnsMockMiddleware::mock(CheckOptions {
                    group: expected_group,
                    background,
                })
                .build(cfg);
                let mut message = crate::libdns::proto::op::Message::query();
                message.add_query(Query::query(
                    "client.example.".parse().unwrap(),
                    RecordType::A,
                ));
                let request = DnsRequest::new(
                    message,
                    "127.0.0.2:12345".parse().unwrap(),
                    crate::libdns::Protocol::Udp,
                );
                let options = ServerOpts {
                    no_cache: Some(false),
                    no_dualstack_selection: Some(true),
                    ipset: Some(vec![ConfigForIP::All(KernelIpSet("listener4".into()))]),
                    rule_group: Some("bound".into()),
                    local_addr: Some("127.0.0.1:5302".parse().unwrap()),
                    is_background: background,
                    ..Default::default()
                };
                let response = handler.search(&request, &options).await.unwrap();
                assert_eq!(
                    response.ip_addrs(),
                    vec!["192.0.2.1".parse::<IpAddr>().unwrap()]
                );
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_mock_middleware_ip() {
        let mw = DnsMockMiddleware::builder()
            .with_a_record("qq.com", "1.5.6.7".parse().unwrap())
            .build(RuntimeConfig::default());

        let res = mw.lookup_rdata("qq.com", RecordType::A).await.unwrap();

        assert_eq!(res, vec![RData::A("1.5.6.7".parse().unwrap())]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_mock_middleware_soa() {
        let mw = DnsMockMiddleware::builder()
            .with_a_record("qq.com", "1.5.6.7".parse().unwrap())
            .build(RuntimeConfig::default());

        let res = mw.lookup_rdata("baidu.com", RecordType::A).await;

        assert!(res.is_err());

        let err = res.unwrap_err();

        assert!(err.is_soa());
    }
}
