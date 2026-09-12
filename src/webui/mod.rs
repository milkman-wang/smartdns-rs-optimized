//! Optional management UI, implemented in Rust and compiled with `webui`.
use crate::{api::ServeState, app::App, config::IBindConfig, dns::*, plugins::Plugin, stats};
use axum::{Json, Router, extract::State, response::Html, routing::get};
use serde::{Serialize, Serializer, ser::SerializeStruct};
use smallvec::SmallVec;
use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
    sync::{Arc, LazyLock, Mutex, atomic::Ordering},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const QUERY_HISTORY_LIMIT: usize = 1000;
const LOG_HISTORY_LIMIT: usize = 300;

#[derive(Clone)]
struct QueryEntry {
    time: i64,
    client: IpAddr,
    name: Name,
    record_type: RecordType,
    rcode: Option<crate::libdns::proto::op::ResponseCode>,
    answers: SmallVec<[QueryAnswer; 1]>,
    source: LookupFrom,
    elapsed_ms: f64,
}

// Keep common address answers inline; other record types retain their typed data.
#[derive(Clone)]
enum QueryAnswer {
    Ip(IpAddr),
    Other(Box<RData>),
}

impl From<&RData> for QueryAnswer {
    fn from(data: &RData) -> Self {
        match data.ip_addr() {
            Some(ip) => Self::Ip(ip),
            None => Self::Other(Box::new(data.clone())),
        }
    }
}

impl std::fmt::Display for QueryAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ip(ip) => std::fmt::Display::fmt(ip, f),
            Self::Other(data) => std::fmt::Display::fmt(data, f),
        }
    }
}

// Format display strings only when the UI requests a snapshot, rather than
// allocating them for every DNS query that enters the bounded history.
impl Serialize for QueryEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut entry = serializer.serialize_struct("QueryEntry", 8)?;
        entry.serialize_field("time", &self.time)?;
        entry.serialize_field("client", &self.client)?;
        entry.serialize_field("name", &self.name.to_ascii())?;
        entry.serialize_field("record_type", &self.record_type.to_string())?;
        entry.serialize_field(
            "rcode",
            &self
                .rcode
                .map(|code| code.to_string())
                .unwrap_or_else(|| "ServFail".into()),
        )?;
        entry.serialize_field(
            "answers",
            &self
                .answers
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
        )?;
        entry.serialize_field("source", &format!("{:?}", self.source))?;
        entry.serialize_field("elapsed_ms", &self.elapsed_ms)?;
        entry.end()
    }
}

#[derive(Clone, Serialize)]
struct LogEntry {
    time: i64,
    level: String,
    message: String,
}

#[derive(Default)]
struct Recorder {
    queries: Mutex<VecDeque<QueryEntry>>,
    logs: Mutex<VecDeque<LogEntry>>,
}

static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(|| Arc::new(Recorder::default()));

impl Plugin for Recorder {
    fn query_complete(
        &self,
        ctx: &DnsContext,
        request: &DnsRequest,
        result: &Result<DnsResponse, DnsError>,
        elapsed: Duration,
        _mac: Option<&str>,
    ) {
        if !ctx.cfg().webui_enabled()
            || ctx.server_opts.is_background
            || ctx.is_dualstack
            || request.src().port() == 0
        {
            return;
        }
        let rcode = match result {
            Ok(response) => Some(response.response_code()),
            Err(error) => error
                .as_no_records_response(request.query().original())
                .map(|response| response.response_code()),
        };
        let entry = QueryEntry {
            time: chrono::Utc::now().timestamp_millis(),
            client: request.src().ip(),
            name: request.query().name().into(),
            record_type: request.query().query_type(),
            rcode,
            answers: result
                .as_ref()
                .ok()
                .into_iter()
                .flat_map(|response| response.answers())
                .map(|record| QueryAnswer::from(record.data()))
                .collect(),
            source: ctx.source.clone(),
            elapsed_ms: elapsed.as_secs_f64() * 1000.0,
        };
        let mut entries = self.queries.lock().unwrap();
        entries.push_front(entry);
        entries.truncate(QUERY_HISTORY_LIMIT);
    }

    fn log(&self, level: crate::log::Level, message: &str) {
        let mut entries = self.logs.lock().unwrap();
        entries.push_front(LogEntry {
            time: chrono::Utc::now().timestamp_millis(),
            level: level.to_string(),
            message: message.to_owned(),
        });
        entries.truncate(LOG_HISTORY_LIMIT);
    }
}

pub fn register_observer() {
    crate::plugins::register(RECORDER.clone());
}

pub struct Server {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub async fn shutdown(mut self) {
        self.cancel.cancel();
        if tokio::time::timeout(Duration::from_secs(3), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
        }
    }
}

pub async fn serve(app: App, address: SocketAddr) -> std::io::Result<Server> {
    let listener = tokio::net::TcpListener::bind(address).await?;
    let address = listener.local_addr()?;
    let state = Arc::new(ServeState {
        dns_handle: app.dns_handle(),
        app,
    });
    let router = routes()
        .with_state(state)
        .into_make_service_with_connect_info::<SocketAddr>();
    let cancel = CancellationToken::new();
    let stopped = cancel.clone();
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
        {
            crate::log::error!("WebUI server: {error}");
        }
    });
    crate::log::info!("WebUI listening on http://{address}");
    Ok(Server { cancel, task })
}

fn routes() -> Router<Arc<ServeState>> {
    crate::api::routes()
        .route("/", get(index))
        .route("/ui/api/snapshot", get(snapshot))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("index.html"))
}

async fn snapshot(State(state): State<Arc<ServeState>>) -> Json<serde_json::Value> {
    let cfg = state.app.cfg().await;
    let cache = state.app.cache().await;
    let cache_bytes = cache
        .as_ref()
        .map(|cache| cache.memory_bytes())
        .unwrap_or(0);
    let cache_total = match &cache {
        Some(cache) => cache.len(),
        None => 0,
    };
    let cache_entries = match cache {
        Some(cache) => cache.summary_entries(),
        None => vec![],
    };
    let checks = stats::CACHE_CHECKS.load(Ordering::Relaxed);
    let hits = stats::CACHE_HITS.load(Ordering::Relaxed);
    let upstreams = stats::upstreams().iter().map(|upstream| {
        let total = upstream.stats.total.load(Ordering::Relaxed);
        let received = upstream.stats.received.load(Ordering::Relaxed);
        serde_json::json!({
            "server": upstream.config.server.to_string(), "host": upstream.host, "ip": upstream.ip,
            "total": total, "received": received, "average_ms": upstream.stats.average_ms(),
            "alive": upstream.alive.load(Ordering::Relaxed),
        })
    }).collect::<Vec<_>>();
    let listeners = cfg.binds().iter().map(|bind| serde_json::json!({
        "address": bind.sock_addr().to_string(),
        "protocol": match bind {
            crate::config::BindAddrConfig::Udp(_) => "UDP", crate::config::BindAddrConfig::Tcp(_) => "TCP",
            crate::config::BindAddrConfig::Tls(_) => "TLS", crate::config::BindAddrConfig::Http(_) => "HTTP",
            crate::config::BindAddrConfig::Https(_) => "HTTPS", crate::config::BindAddrConfig::H3(_) => "HTTP/3",
            crate::config::BindAddrConfig::Quic(_) => "QUIC",
        }
    })).collect::<Vec<_>>();
    let queries = RECORDER
        .queries
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let logs = RECORDER
        .logs
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    Json(serde_json::json!({
        "version": crate::BUILD_VERSION, "flavor": crate::BUILD_FLAVOR,
        "server_name": cfg.server_name().to_ascii(), "uptime_seconds": state.app.uptime().as_secs(),
        "active_queries": state.app.active_queries(), "total_queries": stats::FROM_CLIENT.load(Ordering::Relaxed),
        "average_ms": stats::REQUESTS.average_ms(), "blocked": stats::BLOCKED.load(Ordering::Relaxed),
        "cache_hits": hits, "cache_checks": checks, "cache_hit_percent": if checks > 0 { hits as f64 * 100.0 / checks as f64 } else { 0.0 },
        "cache_bytes": cache_bytes, "cache_total": cache_total, "cache_capacity": cfg.cache_size(), "cache": cache_entries,
        "queries": queries,
        "logs": logs,
        "upstreams": upstreams, "listeners": listeners,
        "settings": { "serve_expired": cfg.serve_expired(), "prefetch": cfg.prefetch_domain(),
            "query_limit": cfg.max_query_limit.unwrap_or(0), "config_directory": cfg.conf_dir(),
            "webui_address": cfg.webui_address(), "cache": cfg.cache_config() },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::ServerOpts,
        dns_conf::RuntimeConfig,
        libdns::{
            Protocol,
            proto::op::{Message, Query},
        },
    };
    use tower::ServiceExt;

    #[test]
    fn query_history_keeps_address_and_text_answers_in_order() {
        use crate::libdns::proto::rr::rdata::{CNAME, TXT};
        let cfg = Arc::new(
            RuntimeConfig::builder()
                .with("webui-enable yes")
                .build()
                .unwrap(),
        );
        let name: Name = "history.example.".parse().unwrap();
        let request: DnsRequest = Query::query(name.clone(), RecordType::ANY).into();
        let ctx = DnsContext::new(&name, cfg, ServerOpts::default());
        let data = [
            RData::CNAME(CNAME("target.example.".parse().unwrap())),
            "192.0.2.20".parse::<IpAddr>().unwrap().into(),
            "2001:db8::20".parse::<IpAddr>().unwrap().into(),
            RData::TXT(TXT::new(vec!["hello".into(), " world".into()])),
        ];
        let response = DnsResponse::new_with_max_ttl(
            request.query().original().clone(),
            data.into_iter()
                .map(|data| Record::from_rdata(name.clone(), 30, data)),
        );
        let recorder = Recorder::default();
        recorder.query_complete(&ctx, &request, &Ok(response), Duration::ZERO, None);
        let entry = serde_json::to_value(&recorder.queries.lock().unwrap()[0]).unwrap();
        assert_eq!(entry["record_type"], "ANY");
        assert_eq!(
            entry["answers"],
            serde_json::json!([
                "target.example.",
                "192.0.2.20",
                "2001:db8::20",
                "hello world"
            ])
        );
    }

    #[tokio::test]
    async fn test_management_routes_and_typed_query_observer() {
        let cfg = Arc::new(
            RuntimeConfig::builder()
                .with("webui-enable yes")
                .build()
                .unwrap(),
        );
        let name: Name = "native-ui.example.".parse().unwrap();
        let mut message = Message::query();
        message.add_query(Query::query(name.clone(), RecordType::A));
        let request = DnsRequest::new(message, "192.0.2.10:12345".parse().unwrap(), Protocol::Udp);
        let ctx = DnsContext::new(&name, cfg.clone(), ServerOpts::default());
        let mut response = request.to_response();
        response.add_answer(Record::from_rdata(
            name,
            30,
            RData::A("192.0.2.20".parse().unwrap()),
        ));
        let recorder = Recorder::default();
        recorder.query_complete(
            &ctx,
            &request,
            &Ok(response.into()),
            Duration::from_millis(7),
            None,
        );
        {
            let entries = recorder.queries.lock().unwrap();
            let entry = serde_json::to_value(&entries[0]).unwrap();
            assert_eq!(entry["name"], "native-ui.example.");
            assert_eq!(entry["client"], "192.0.2.10");
            assert_eq!(entry["answers"], serde_json::json!(["192.0.2.20"]));
            assert_eq!(entry["record_type"], "A");
            assert_eq!(entry["rcode"], "No Error");
            assert_eq!(entry["source"], "None");
            assert_eq!(entry["elapsed_ms"], 7.0);
        }
        recorder.query_complete(
            &ctx,
            &request,
            &Err(crate::libdns::proto::ProtoError::from("upstream timeout").into()),
            Duration::from_millis(1),
            None,
        );
        assert_eq!(
            serde_json::to_value(&recorder.queries.lock().unwrap()[0]).unwrap()["rcode"],
            "ServFail"
        );
        let app = App::new(cfg);
        let router = routes().with_state(Arc::new(ServeState {
            dns_handle: app.dns_handle(),
            app,
        }));
        for path in ["/", "/ui/api/snapshot"] {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(path)
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            if path == "/" {
                assert!(String::from_utf8_lossy(&body).contains("SmartDNS"));
            } else {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["flavor"],
                    "webui"
                );
            }
        }
    }
}
