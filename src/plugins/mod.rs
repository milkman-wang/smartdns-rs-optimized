//! Rust extensions compiled with SmartDNS. No dynamic C ABI is exposed.
use crate::{app::App, dns::*, dns_conf::RuntimeConfig};
use std::sync::{
    Arc, LazyLock, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

/// Typed event hooks for extensions built into the executable. Use the existing
/// `Middleware` trait when an extension needs to change a DNS response.
pub trait Plugin: Send + Sync {
    fn query_complete(
        &self,
        _ctx: &DnsContext,
        _request: &DnsRequest,
        _response: &Result<DnsResponse, DnsError>,
        _elapsed: Duration,
        _mac: Option<&str>,
    ) {
    }
    fn log(&self, _level: crate::log::Level, _message: &str) {}
    fn audit(&self, _message: &str) {}
    fn needs_mac(&self) -> bool {
        false
    }
}

type Plugins = Arc<[Arc<dyn Plugin>]>;
static PLUGINS: LazyLock<RwLock<Plugins>> = LazyLock::new(|| RwLock::new(Arc::from([])));
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Register a Rust extension during startup. Callbacks run without the registry
/// lock and may retain owned Rust values; they must return promptly.
pub fn register(plugin: Arc<dyn Plugin>) {
    let mut plugins = PLUGINS.write().unwrap();
    let mut updated = plugins.iter().cloned().collect::<Vec<_>>();
    updated.push(plugin);
    *plugins = updated.into();
    ACTIVE.store(true, Ordering::Release);
}

fn snapshot() -> Plugins {
    PLUGINS.read().unwrap().clone()
}

pub async fn initialize(_cfg: Arc<RuntimeConfig>, _app: App) -> anyhow::Result<()> {
    #[cfg(feature = "webui")]
    crate::webui::register_observer();
    Ok(())
}

pub fn shutdown() {
    *PLUGINS.write().unwrap() = Arc::from([]);
    ACTIVE.store(false, Ordering::Release);
}
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}
pub fn wants_mac() -> bool {
    active() && snapshot().iter().any(|plugin| plugin.needs_mac())
}
pub fn completed(
    ctx: &DnsContext,
    req: &DnsRequest,
    response: &Result<DnsResponse, DnsError>,
    start: Instant,
    mac: Option<&str>,
) {
    if !active() {
        return;
    }
    for plugin in snapshot().iter() {
        plugin.query_complete(ctx, req, response, start.elapsed(), mac);
    }
}
pub fn log(level: crate::log::Level, message: &str) {
    thread_local! { static LOGGING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
    LOGGING.with(|logging| {
        if logging.replace(true) {
            return;
        }
        for plugin in snapshot().iter() {
            plugin.log(level, message);
        }
        logging.set(false);
    });
}
pub fn audit(message: &str) {
    for plugin in snapshot().iter() {
        plugin.audit(message);
    }
}

pub struct LogLayer;
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if !active() {
            return;
        }
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write;
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                if field.name() == "message" {
                    let _ = write!(&mut self.0, "{value:?}");
                } else {
                    let _ = write!(&mut self.0, "{}={value:?}", field.name());
                }
            }
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        log(*event.metadata().level(), &fields.0);
    }
}
