//! Logging: a daily-rotated file plus a bounded console queue printed by the main thread.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;
use tracing::Level;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, Registry, filter::LevelFilter, reload};

static CONSOLE: OnceLock<Mutex<ConsoleQueue>> = OnceLock::new();
static GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();
static FILE_LEVEL: OnceLock<reload::Handle<LevelFilter, Registry>> = OnceLock::new();

/// Events with this target go to the log file only (panic backtraces and other bulky diagnostics).
pub const FILE_ONLY: &str = "lb_file_only";

pub struct ConsoleQueue {
    lines: VecDeque<String>,
    dropped: u64,
    min_level: Level,
}

const CONSOLE_CAP: usize = 256;

fn console() -> &'static Mutex<ConsoleQueue> {
    CONSOLE.get_or_init(|| {
        Mutex::new(ConsoleQueue {
            lines: VecDeque::new(),
            dropped: 0,
            min_level: Level::WARN,
        })
    })
}

/// Queues a line for the server console (printed at the end of the frame).
pub fn console_line(line: String) {
    let mut q = console().lock();
    if q.lines.len() >= CONSOLE_CAP {
        q.dropped += 1;
        q.lines.pop_front();
    }
    q.lines.push_back(line);
}

/// Takes at most `max` queued console lines.
pub fn drain_console(max: usize) -> (Vec<String>, u64) {
    let mut q = console().lock();
    let n = q.lines.len().min(max);
    let lines = q.lines.drain(..n).collect();
    let dropped = std::mem::take(&mut q.dropped);
    (lines, dropped)
}

struct ConsoleLayer;

impl<S: tracing::Subscriber> Layer<S> for ConsoleLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        if level > console().lock().min_level || event.metadata().target() == FILE_ONLY {
            return;
        }
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        console_line(format!(
            "[lambdabots] {}: {}",
            level.as_str().to_ascii_lowercase(),
            visitor.0
        ));
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            self.0.push_str(&format!(" {}={value}", field.name()));
        }
    }
}

pub fn parse_level(s: &str) -> Level {
    match s.to_ascii_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "warn" | "warning" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    }
}

pub fn set_console_level(level: &str) {
    console().lock().min_level = parse_level(level);
}

/// Changes the file log level at runtime; false before [`init`] installed a file log.
pub fn set_file_level(level: &str) -> bool {
    FILE_LEVEL
        .get()
        .is_some_and(|h| h.modify(|f| *f = LevelFilter::from_level(parse_level(level))).is_ok())
}

/// Installs the global subscriber once per process; later calls only update the levels.
pub fn init(log_dir: Option<&Path>, file_level: &str, console_level: &str, max_files: usize) {
    set_console_level(console_level);
    if GUARD.get().is_some() {
        set_file_level(file_level);
        return;
    }
    let (level_filter, level_handle) = reload::Layer::new(LevelFilter::from_level(parse_level(file_level)));
    let _ = FILE_LEVEL.set(level_handle);
    let file_layer = log_dir.and_then(|dir| {
        std::fs::create_dir_all(dir).ok()?;
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("lambdabots")
            .filename_suffix("log")
            .max_log_files(max_files.max(1))
            .build(dir)
            .ok()?;
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let _ = GUARD.set(guard);
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_target(false),
        )
    });
    let _ = tracing_subscriber::registry()
        .with(level_filter)
        .with(ConsoleLayer)
        .with(file_layer)
        .try_init();
}
