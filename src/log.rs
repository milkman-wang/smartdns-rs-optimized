use std::{cell::RefCell, env, fmt, io, path::Path, sync::OnceLock};

pub use tracing::dispatcher::set_default;
use tracing::{Dispatch, Event, Subscriber, subscriber::DefaultGuard};
use tracing_subscriber::{
    filter::{LevelFilter, Targets},
    fmt::{
        FmtContext, FormatEvent, FormatFields, FormattedFields, MakeWriter, format,
        writer::MakeWriterExt,
    },
    prelude::__tracing_subscriber_SubscriberExt,
    registry::LookupSpan,
};

static INIT_CONSOLE_LEVEL: OnceLock<Level> = OnceLock::new();
type LevelSetter = Box<dyn Fn(Option<Level>) + Send + Sync>;
static LEVEL_SETTER: std::sync::Mutex<Option<LevelSetter>> = std::sync::Mutex::new(None);

pub fn set_runtime_level(level: Option<Level>) {
    if let Some(setter) = LEVEL_SETTER.lock().unwrap().as_ref() {
        setter(level);
    }
}

thread_local! {
    pub static LOG_GUARD: RefCell<Option<DefaultGuard>> = const { RefCell::new(None) };
}

pub use tracing::*;

type MappedFile = crate::infra::mapped_file::MutexMappedFile;

#[allow(clippy::too_many_arguments)]
pub fn make_dispatch<P: AsRef<Path>>(
    path: P,
    enabled: bool,
    level: Option<Level>,
    filter: Option<&str>,
    size: u64,
    num: u64,
    mode: Option<u32>,
    to_console: bool,
    to_syslog: bool,
) -> Dispatch {
    let cli_level = INIT_CONSOLE_LEVEL.get().cloned();
    let level = match (level, cli_level) {
        (Some(cfg), Some(cli)) => cfg.max(cli),
        (Some(cfg), None) => cfg,
        (None, Some(cli)) => cli,
        (None, None) => Level::ERROR,
    };

    let file = MappedFile::open(path.as_ref(), size, Some(num as usize), mode);

    let writable = enabled
        && file
            .0
            .lock()
            .unwrap()
            .touch()
            .map(|_| true)
            .unwrap_or_else(|err| {
                warn!("{:?}, {:?}", path.as_ref(), err);
                false
            });

    let console_level = if to_console {
        level
    } else {
        cli_level.unwrap_or(Level::ERROR)
    };

    let console_writer = io::stdout.with_max_level(console_level);

    if writable {
        // log hello
        {
            let writer = file.with_max_level(level);
            let dispatch = internal_make_dispatch(level, filter, writer, true, to_syslog);

            let _guard = set_default(&dispatch);
            crate::hello_starting();
        }

        let file_writer =
            MappedFile::open(path.as_ref(), size, Some(num as usize), mode).with_max_level(level);

        if to_console {
            internal_make_dispatch(
                level.max(console_level),
                filter,
                file_writer.and(console_writer),
                true,
                to_syslog,
            )
        } else {
            internal_make_dispatch(
                level.max(console_level),
                filter,
                file_writer,
                true,
                to_syslog,
            )
        }
    } else if to_console {
        internal_make_dispatch(console_level, filter, console_writer, true, to_syslog)
    } else if to_syslog {
        internal_make_dispatch(level, filter, io::sink, true, true)
    } else {
        Dispatch::none()
    }
}

pub fn console(console_level: Level) -> DefaultGuard {
    INIT_CONSOLE_LEVEL.get_or_init(|| console_level);
    let console_writer = io::stdout.with_max_level(console_level);
    set_default(&internal_make_dispatch(
        console_level,
        None,
        console_writer,
        false,
        false,
    ))
}

#[inline]
fn internal_make_dispatch<W: for<'writer> MakeWriter<'writer> + 'static + Send + Sync>(
    level: tracing::Level,
    filter: Option<&str>,
    writer: W,
    diagnostic: bool,
    to_syslog: bool,
) -> Dispatch {
    let layer = tracing_subscriber::fmt::layer()
        .event_format(TdnsFormatter)
        .with_writer(writer);

    let syslog = to_syslog.then(|| {
        tracing_subscriber::fmt::layer()
            .event_format(TdnsFormatter)
            .with_ansi(false)
            .with_writer(crate::infra::syslog::SyslogWriter)
    });
    let (filter_layer, handle) = tracing_subscriber::reload::Layer::new(make_filter(level, filter));
    let subscriber = tracing_subscriber::registry()
        .with(crate::plugins::LogLayer)
        .with(layer)
        .with(syslog)
        .with(filter_layer);

    if diagnostic {
        let filter = filter.map(str::to_string);
        *LEVEL_SETTER.lock().unwrap() = Some(Box::new(move |level| {
            let next = level
                .map(|level| make_filter(level, filter.as_deref()))
                .unwrap_or_else(Targets::new);
            let _ = handle.reload(next);
        }));
    }

    if diagnostic {
        #[cfg(feature = "future-diagnostic")]
        let subscriber = subscriber.with({
            // console_subscriber::init();
            let console_layer = console_subscriber::ConsoleLayer::builder()
                .with_default_env()
                .spawn();
            console_layer
        });

        Dispatch::new(subscriber)
    } else {
        Dispatch::new(subscriber)
    }
}

/// The `target=level` directives of the filter and of `RUST_LOG`, with
/// targets that no directive names at WARN unless a bare level says
/// otherwise. Span and field filters are not supported; such directives are
/// skipped.
fn make_filter(level: tracing::Level, filter: Option<&str>) -> Targets {
    let directives = all_smart_dns(level, filter);
    let supported = directives
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .filter(|directive| {
            let ok = directive.parse::<Targets>().is_ok();
            if !ok {
                eprintln!("ignoring unsupported log filter directive: {directive}");
            }
            ok
        })
        .collect::<Vec<_>>()
        .join(",");
    let targets = supported.parse::<Targets>().unwrap_or_default();
    if targets.default_level().is_some() {
        targets
    } else {
        targets.with_default(LevelFilter::WARN)
    }
}

#[inline]
fn all_smart_dns(level: impl ToString, filter: Option<&str>) -> String {
    filter
        .unwrap_or("named={level},smartdns={level},{env}")
        .replace("{level}", level.to_string().to_uppercase().as_str())
        .replace("{env}", get_env().as_str())
}

#[inline]
fn get_env() -> String {
    env::var("RUST_LOG").unwrap_or_default()
}

struct TdnsFormatter;

impl<S, N> FormatEvent<S, N> for TdnsFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: format::Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let now = chrono::Local::now();
        let now_msecs = now.timestamp_millis() % 1000;
        let date = now.format("%Y-%m-%d %H:%M:%S");

        // Format values from the event's's metadata:
        let metadata = event.metadata();

        if metadata.level() == &tracing::Level::INFO {
            write!(&mut writer, "{}.{}:{}", date, now_msecs, metadata.level())?;
        } else {
            write!(
                &mut writer,
                "{}.{}:{}:{}",
                date,
                now_msecs,
                metadata.level(),
                metadata.target()
            )?;
            if let Some(line) = metadata.line() {
                write!(&mut writer, ":{line}")?;
            }
        }

        // Format all the spans in the event's span context.
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                write!(writer, ":{}", span.name())?;

                let ext = span.extensions();
                let fields = &ext
                    .get::<FormattedFields<N>>()
                    .expect("will never be `None`");

                // Skip formatting the fields if the span had no fields.
                if !fields.is_empty() {
                    write!(writer, "{{{fields}}}")?;
                }
            }
        }

        // Write fields on the event
        write!(writer, ": ")?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;

        writeln!(writer)
    }
}

impl<'a> MakeWriter<'a> for MappedFile {
    type Writer = &'a MappedFile;
    fn make_writer(&'a self) -> Self::Writer {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_log_level_cmp() {
        assert_eq!(Level::INFO.max(Level::DEBUG), Level::DEBUG);
    }

    #[test]
    fn test_filter_targets_and_default_level() {
        let filter = make_filter(Level::INFO, None);
        assert!(filter.would_enable("smartdns::dns_mw_ns", &Level::INFO));
        assert!(!filter.would_enable("smartdns::dns_mw_ns", &Level::DEBUG));
        assert!(filter.would_enable("named", &Level::INFO));
        assert!(filter.would_enable("hickory_proto::udp", &Level::WARN));
        assert!(!filter.would_enable("hickory_proto::udp", &Level::INFO));

        // Unsupported span filters are skipped; the rest still applies.
        let filter = make_filter(
            Level::DEBUG,
            Some("smartdns={level},hickory_proto=error,[lookup]=trace"),
        );
        assert!(filter.would_enable("smartdns", &Level::DEBUG));
        assert!(!filter.would_enable("hickory_proto", &Level::WARN));
        assert!(filter.would_enable("other", &Level::WARN));

        // A bare level replaces the WARN default.
        let filter = make_filter(Level::INFO, Some("smartdns={level},error"));
        assert!(!filter.would_enable("other", &Level::WARN));
    }
}
