//! Process-wide logging built on `tracing`.
//!
//! [`init`] installs one global subscriber with three parts:
//! - a [`Targets`] filter chosen from `SAPPHIRE_LOG`, then `RUST_LOG`, then
//!   [`LogConfig::default_filter`], in `target=level` directive syntax (`info,sapphire=debug`).
//!   Unlike `EnvFilter`, it has no span or field filters, so it needs no regex engine;
//! - an optional formatter writing to stderr;
//! - an optional [`LogSink`], which receives every enabled event as a plain [`LogRecord`].
//!   The platform layer registers one that forwards to `SDL_Log` on mobile, where stderr goes
//!   nowhere (spec §5.3).
//!
//! Records from the `log` crate (wgpu, naga) are bridged into `tracing`, so they reach the same
//! filter and sinks. Engine code logs with the `tracing` macros and a `sapphire::<subsystem>`
//! target (see `docs/conventions.md`).

use std::cell::RefCell;
use std::fmt::{self, Write as _};
use std::io::IsTerminal as _;

use tracing::field::{Field, Visit};
use tracing::level_filters::LevelFilter;
use tracing::{Event, Subscriber};
use tracing_log::{AsLog as _, NormalizeEvent as _};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::{ParseError, Targets};
use tracing_subscriber::layer::{Context, SubscriberExt as _};

/// Filter used when neither environment variable is set.
pub const DEFAULT_FILTER: &str = "info,sapphire=debug";

/// Environment variable that overrides the filter: comma-separated `target=level` directives,
/// plus an optional bare level for everything else.
pub const FILTER_ENV: &str = "SAPPHIRE_LOG";

/// Environment variable consulted when [`FILTER_ENV`] is unset, for familiarity.
pub const FALLBACK_FILTER_ENV: &str = "RUST_LOG";

/// How [`init`] configures logging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogConfig {
    /// Filter directives used when neither [`FILTER_ENV`] nor [`FALLBACK_FILTER_ENV`] is set.
    pub default_filter: String,
    /// Colour the stderr output with ANSI escapes.
    pub ansi: bool,
    /// Also write formatted output to stderr. The platform layer turns this off on mobile.
    pub stderr: bool,
}

impl Default for LogConfig {
    /// [`DEFAULT_FILTER`], stderr output on, and ANSI colour only when stderr is a terminal.
    fn default() -> Self {
        Self {
            default_filter: DEFAULT_FILTER.to_owned(),
            ansi: std::io::stderr().is_terminal(),
            stderr: true,
        }
    }
}

/// Where the active filter directives came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterSource {
    /// The `SAPPHIRE_LOG` environment variable.
    SapphireLog,
    /// The `RUST_LOG` environment variable.
    RustLog,
    /// [`LogConfig::default_filter`].
    Default,
}

impl fmt::Display for FilterSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SapphireLog => FILTER_ENV,
            Self::RustLog => FALLBACK_FILTER_ENV,
            Self::Default => "LogConfig::default_filter",
        })
    }
}

/// Why [`init`] failed.
#[derive(Debug, thiserror::Error)]
pub enum LogInitError {
    /// A global subscriber is already installed, by an earlier [`init`] or by other code.
    #[error("logging is already initialized")]
    AlreadyInitialized,
    /// The filter directives did not parse.
    #[error("invalid log filter {filter:?} from {origin}")]
    InvalidFilter {
        /// Where the directives came from.
        origin: FilterSource,
        /// The directives as given.
        filter: String,
        /// The parser's complaint.
        #[source]
        error: FilterError,
    },
}

/// Why a filter string was rejected.
#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    /// A directive isn't `target=level`, `target` or `level`.
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// `EnvFilter`'s span and field filters (`target[span{field}]=level`) aren't supported;
    /// rejected rather than read as part of a target name that would never match.
    #[error("span and field filters are not supported: {0:?}")]
    Unsupported(String),
    /// Nothing but commas and whitespace.
    #[error("no filter directives")]
    Empty,
}

/// Parses comma-separated directives into a [`Targets`] filter. Whitespace around a directive
/// and empty directives are ignored, as `EnvFilter` did: `Targets` alone would read an empty
/// directive (`info,`) as "everything at ERROR" and keep the space in `info, sapphire=debug`
/// as part of a target name.
pub fn parse_filter(directives: &str) -> Result<Targets, FilterError> {
    let directives: Vec<&str> = directives.split(',').map(str::trim).filter(|d| !d.is_empty()).collect();
    if directives.is_empty() {
        return Err(FilterError::Empty);
    }
    if let Some(unsupported) = directives.iter().find(|d| d.contains(['[', ']', '{', '}', ' ', '\t'])) {
        return Err(FilterError::Unsupported((*unsupported).to_owned()));
    }
    Ok(directives.join(",").parse()?)
}

/// Severity of a [`LogRecord`], most severe first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// Something failed.
    Error,
    /// Something is wrong but execution continues.
    Warn,
    /// Normal operational messages.
    Info,
    /// Diagnostics for engine developers.
    Debug,
    /// Very verbose diagnostics.
    Trace,
}

impl From<tracing::Level> for LogLevel {
    fn from(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => Self::Error,
            tracing::Level::WARN => Self::Warn,
            tracing::Level::INFO => Self::Info,
            tracing::Level::DEBUG => Self::Debug,
            tracing::Level::TRACE => Self::Trace,
        }
    }
}

/// One formatted log event, as handed to a [`LogSink`].
#[derive(Clone, Copy, Debug)]
pub struct LogRecord<'a> {
    /// Severity.
    pub level: LogLevel,
    /// The event's target, such as `sapphire::render` or a `log` crate module path.
    pub target: &'a str,
    /// The message followed by any structured fields as ` key=value` pairs.
    pub message: &'a str,
}

/// Receives every event that passes the filter.
///
/// Called synchronously on the thread that logged, so implementations must be cheap and must
/// not block for long. Logging from inside [`LogSink::log`] is allowed but is formatted without
/// the per-thread buffer, so it allocates.
pub trait LogSink: Send + Sync + 'static {
    /// Handles one record. The borrowed strings are only valid for this call.
    fn log(&self, record: &LogRecord<'_>);
}

impl<F> LogSink for F
where
    F: Fn(&LogRecord<'_>) + Send + Sync + 'static,
{
    fn log(&self, record: &LogRecord<'_>) {
        self(record);
    }
}

/// Installs the global subscriber.
///
/// Call once, as early as possible in `main` (after crash reporting). Later calls return
/// [`LogInitError::AlreadyInitialized`] and leave the first configuration in place.
///
/// If another `log`-crate logger is already installed, `log` records are not bridged; that is
/// reported as a warning rather than an error, because `tracing` logging still works.
pub fn init(config: LogConfig, sink: Option<Box<dyn LogSink>>) -> Result<(), LogInitError> {
    let sapphire_log = non_empty_env(FILTER_ENV);
    let rust_log = non_empty_env(FALLBACK_FILTER_ENV);
    let selected = select_filter(&config.default_filter, sapphire_log.as_deref(), rust_log.as_deref())?;
    let SelectedFilter { filter, directives, origin, rejected } = selected;

    let stderr = config
        .stderr
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr).with_ansi(config.ansi));
    let sink = sink.map(|sink| SinkLayer { sink });
    let subscriber = tracing_subscriber::registry().with(filter).with(stderr).with(sink);
    tracing::subscriber::set_global_default(subscriber).map_err(|_| LogInitError::AlreadyInitialized)?;

    // The bridge must be installed after the subscriber so it picks up the filter's max level.
    let bridged = tracing_log::LogTracer::builder().with_max_level(LevelFilter::current().as_log()).init();
    if bridged.is_err() {
        tracing::warn!(
            target: "sapphire::core",
            "another `log` logger is installed; records from the `log` crate are not captured"
        );
    }
    if let Some(rejected) = rejected {
        tracing::warn!(
            target: "sapphire::core",
            origin = %rejected.origin,
            filter = rejected.directives,
            error = %rejected.error,
            "ignoring an invalid log filter; using the default"
        );
    }
    tracing::debug!(target: "sapphire::core", filter = directives, %origin, "logging initialized");
    Ok(())
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.trim().is_empty())
}

/// The filter [`init`] installs, and the environment filter it had to reject, if any.
struct SelectedFilter<'a> {
    filter: Targets,
    directives: &'a str,
    origin: FilterSource,
    rejected: Option<RejectedFilter<'a>>,
}

struct RejectedFilter<'a> {
    origin: FilterSource,
    directives: &'a str,
    error: FilterError,
}

/// Parses the filter chosen by [`resolve_filter`]. A malformed environment filter falls back
/// to the default, so a typo in `SAPPHIRE_LOG` can't leave the process without any logging (or
/// silence crash-capture warnings); only an invalid default is an error.
fn select_filter<'a>(
    default: &'a str,
    sapphire_log: Option<&'a str>,
    rust_log: Option<&'a str>,
) -> Result<SelectedFilter<'a>, LogInitError> {
    let invalid_default = |error| LogInitError::InvalidFilter {
        origin: FilterSource::Default,
        filter: default.to_owned(),
        error,
    };
    let (directives, origin) = resolve_filter(default, sapphire_log, rust_log);
    match parse_filter(directives) {
        Ok(filter) => Ok(SelectedFilter { filter, directives, origin, rejected: None }),
        Err(error) if origin == FilterSource::Default => Err(invalid_default(error)),
        Err(error) => Ok(SelectedFilter {
            filter: parse_filter(default).map_err(invalid_default)?,
            directives: default,
            origin: FilterSource::Default,
            rejected: Some(RejectedFilter { origin, directives, error }),
        }),
    }
}

/// Applies the `SAPPHIRE_LOG` → `RUST_LOG` → default precedence.
fn resolve_filter<'a>(
    default: &'a str,
    sapphire_log: Option<&'a str>,
    rust_log: Option<&'a str>,
) -> (&'a str, FilterSource) {
    match (sapphire_log, rust_log) {
        (Some(directives), _) => (directives, FilterSource::SapphireLog),
        (None, Some(directives)) => (directives, FilterSource::RustLog),
        (None, None) => (default, FilterSource::Default),
    }
}

/// Forwards events to a [`LogSink`].
struct SinkLayer {
    sink: Box<dyn LogSink>,
}

thread_local! {
    /// Reused per thread so steady-state logging does not allocate for formatting.
    static MESSAGE_BUFFER: RefCell<String> = const { RefCell::new(String::new()) };
}

impl<S: Subscriber> Layer<S> for SinkLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let emit = |buffer: &mut String| {
            buffer.clear();
            format_event(event, buffer);
            // Bridged `log` records carry their real target in the normalized metadata.
            let normalized = event.normalized_metadata();
            let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
            self.sink.log(&LogRecord {
                level: (*metadata.level()).into(),
                target: metadata.target(),
                message: buffer,
            });
        };
        // The buffer is busy if the sink itself logs, and gone during thread teardown.
        let reused = MESSAGE_BUFFER.try_with(|cell| match cell.try_borrow_mut() {
            Ok(mut buffer) => {
                emit(&mut buffer);
                true
            }
            Err(_) => false,
        });
        if reused != Ok(true) {
            emit(&mut String::new());
        }
    }
}

/// Writes the message, then other fields as ` key=value`, regardless of declaration order.
fn format_event(event: &Event<'_>, out: &mut String) {
    let skip_log_fields = event.is_log();
    event.record(&mut FieldWriter { out: &mut *out, pass: Pass::Message, skip_log_fields });
    event.record(&mut FieldWriter { out, pass: Pass::Fields, skip_log_fields });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Message,
    Fields,
}

struct FieldWriter<'a> {
    out: &'a mut String,
    pass: Pass,
    /// Bridged `log` records carry `log.target`, `log.file` and so on as fields; the target is
    /// reported separately and the rest is noise for a sink.
    skip_log_fields: bool,
}

impl FieldWriter<'_> {
    fn write(&mut self, field: &Field, value: fmt::Arguments<'_>) {
        let is_message = field.name() == "message";
        // Writing to a `String` cannot fail.
        let _ = match self.pass {
            Pass::Message if is_message => self.out.write_fmt(value),
            Pass::Fields if !is_message && !(self.skip_log_fields && field.name().starts_with("log.")) => {
                let separator = if self.out.is_empty() { "" } else { " " };
                write!(self.out, "{separator}{}={value}", field.name())
            }
            Pass::Message | Pass::Fields => Ok(()),
        };
    }
}

impl Visit for FieldWriter<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.write(field, format_args!("{value}"));
        } else {
            self.write(field, format_args!("{value:?}"));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.write(field, format_args!("{value:?}"));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Owned {
        level: LogLevel,
        target: String,
        message: String,
    }

    fn capture(f: impl FnOnce()) -> Vec<Owned> {
        let records = Arc::new(Mutex::new(Vec::new()));
        let sink_records = Arc::clone(&records);
        let sink = move |record: &LogRecord<'_>| {
            sink_records.lock().expect("test mutex").push(Owned {
                level: record.level,
                target: record.target.to_owned(),
                message: record.message.to_owned(),
            });
        };
        let subscriber = tracing_subscriber::registry()
            .with("debug".parse::<Targets>().expect("valid filter"))
            .with(SinkLayer { sink: Box::new(sink) });
        tracing::subscriber::with_default(subscriber, f);
        let records = records.lock().expect("test mutex");
        records.clone()
    }

    #[test]
    fn filter_precedence() {
        assert_eq!(resolve_filter("info", None, None), ("info", FilterSource::Default));
        assert_eq!(resolve_filter("info", None, Some("warn")), ("warn", FilterSource::RustLog));
        assert_eq!(resolve_filter("info", Some("trace"), Some("warn")), ("trace", FilterSource::SapphireLog));
    }

    #[test]
    fn invalid_environment_filter_falls_back_to_the_default() {
        let selected = select_filter("info", Some("sapphire=nonsense"), None).unwrap();
        assert_eq!((selected.directives, selected.origin), ("info", FilterSource::Default));
        let rejected = selected.rejected.expect("the bad filter is reported");
        assert_eq!((rejected.origin, rejected.directives), (FilterSource::SapphireLog, "sapphire=nonsense"));

        let valid = select_filter("info", None, Some("warn")).unwrap();
        assert_eq!((valid.directives, valid.origin), ("warn", FilterSource::RustLog));
        assert!(valid.rejected.is_none());

        assert!(matches!(
            select_filter("sapphire=loud", None, None),
            Err(LogInitError::InvalidFilter { origin: FilterSource::Default, .. })
        ));
    }

    #[test]
    fn filters_tolerate_spacing_and_reject_span_syntax() {
        let enabled = |filter: &Targets, target: &str, level| filter.would_enable(target, &level);
        let filter = parse_filter(" info, sapphire=debug ,").expect("spaces and a trailing comma");
        assert!(enabled(&filter, "sapphire::render", tracing::Level::DEBUG));
        assert!(enabled(&filter, "naga", tracing::Level::INFO), "a trailing comma isn't `error`");
        assert!(!enabled(&filter, "naga", tracing::Level::DEBUG));
        assert!(matches!(parse_filter("sapphire[frame]=debug"), Err(FilterError::Unsupported(_))));
        assert!(matches!(parse_filter("sapphire = debug"), Err(FilterError::Unsupported(_))));
        assert!(matches!(parse_filter(" , "), Err(FilterError::Empty)));
        assert!(matches!(parse_filter("sapphire=loud"), Err(FilterError::Parse(_))));
    }

    #[test]
    fn default_filter_parses() {
        assert!(parse_filter(DEFAULT_FILTER).is_ok());
        assert_eq!(LogConfig::default().default_filter, DEFAULT_FILTER);
    }

    #[test]
    fn sink_receives_message_and_fields() {
        let records = capture(|| {
            let frame = 42;
            tracing::info!(target: "sapphire::test", frame, name = "boss", "spawned {}", "enemy");
        });
        assert_eq!(
            records,
            [Owned {
                level: LogLevel::Info,
                target: "sapphire::test".into(),
                message: r#"spawned enemy frame=42 name="boss""#.into(),
            }]
        );
    }

    #[test]
    fn sink_respects_filter() {
        let records = capture(|| {
            tracing::trace!("too verbose");
            tracing::error!("kept");
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].level, LogLevel::Error);
        assert_eq!(records[0].message, "kept");
    }

    #[test]
    fn fields_without_message() {
        let records = capture(|| tracing::warn!(a = 1, b = true));
        assert_eq!(records[0].message, "a=1 b=true");
    }

    #[test]
    fn busy_buffer_falls_back_instead_of_panicking() {
        // A sink that logs re-enters `on_event` while the thread's buffer is borrowed.
        let records = MESSAGE_BUFFER.with(|cell| {
            let _held = cell.borrow_mut();
            capture(|| tracing::info!("while borrowed"))
        });
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].message, "while borrowed");
    }

    #[test]
    fn levels_map_from_tracing() {
        assert_eq!(LogLevel::from(tracing::Level::ERROR), LogLevel::Error);
        assert_eq!(LogLevel::from(tracing::Level::TRACE), LogLevel::Trace);
        assert!(LogLevel::Error < LogLevel::Trace);
    }
}
