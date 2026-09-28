//! Installing the logger: the console and file outputs and their levels.
//!
//! [`LineLayer`] is a `tracing` layer that turns each event of the kernel's log macros into the
//! reference's line: the level from the event's `severity` field (or the nearest reference level
//! of an event from elsewhere; `trace` events are dropped), the message, then any other fields as
//! ` name=value`. An empty message writes nothing, as in the reference; emoji codes are replaced
//! or removed before the line is formatted, which escapes the whole text, fields included
//! ([`format::escape`]). Each output shows the levels at least as severe as its
//! own. [`init`] installs the layer for the process with the console, coloured, and the rotating
//! log file, plain.

use std::fmt::{self, Write as _};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use heartwood_kernel::clock::{Clock, SystemClock};
use heartwood_kernel::log::Level;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_appender::non_blocking::{NonBlocking, NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use crate::errors::LoggerError;
use crate::format;
use crate::rotation::RotatingFile;

/// Which output shows which levels, and how many rotated files are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoggerOptions {
    /// The least severe level the console shows; `None` turns the console output off.
    pub console: Option<Level>,
    /// The least severe level the file shows; `None` turns the file output off.
    pub file: Option<Level>,
    /// The most rotated files kept.
    pub max_files: usize,
    /// The most bytes the rotated files may take together.
    pub max_bytes: u64,
    /// Whether emoji codes such as `:dove_of_peace:` become emoji; when off they are removed.
    pub emojify: bool,
}

impl Default for LoggerOptions {
    /// The reference's defaults: `info` on the console, `debug` in the file, 10 rotated files of
    /// at most 100 MiB together, emoji on.
    fn default() -> LoggerOptions {
        LoggerOptions {
            console: Some(Level::Info),
            file: Some(Level::Debug),
            max_files: 10,
            max_bytes: 100 * 1024 * 1024,
            emojify: true,
        }
    }
}

/// Keeps the outputs running; dropping it flushes and closes the log file.
#[derive(Debug)]
pub struct LoggerGuard {
    _guards: Vec<WorkerGuard>,
}

/// Install the logger for this process, with its files in `log_dir` named after `file_stem`
/// (the reference's `<token>-<network>-<process>`).
pub fn init(
    options: &LoggerOptions,
    log_dir: &Path,
    file_stem: &str,
) -> Result<LoggerGuard, LoggerError> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let mut layer = LineLayer::new(options.emojify, Arc::clone(&clock));
    let mut guards = Vec::new();
    if let Some(level) = options.console {
        let (writer, guard) = lossless(io::stdout());
        guards.push(guard);
        layer = layer.output(level, true, Box::new(writer));
    }
    if let Some(level) = options.file {
        let file = RotatingFile::open(
            log_dir,
            file_stem,
            options.max_files,
            options.max_bytes,
            clock,
        )?;
        let (writer, guard) = lossless(file);
        guards.push(guard);
        layer = layer.output(level, false, Box::new(writer));
    }
    tracing::subscriber::set_global_default(Registry::default().with(layer))
        .map_err(|_| LoggerError::AlreadyInstalled)?;
    Ok(LoggerGuard { _guards: guards })
}

/// A writer thread for `writer` that never drops a line: when its queue is full, the logging
/// thread waits. The guard flushes and ends the thread when dropped.
fn lossless<W: Write + Send + 'static>(writer: W) -> (NonBlocking, WorkerGuard) {
    NonBlockingBuilder::default().lossy(false).finish(writer)
}

/// The layer that writes the reference's lines.
pub struct LineLayer {
    emojify: bool,
    clock: Arc<dyn Clock>,
    outputs: Vec<Output>,
}

struct Output {
    level: Level,
    colour: bool,
    writer: Mutex<Box<dyn Write + Send>>,
}

impl fmt::Debug for LineLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let outputs: Vec<(Level, bool)> =
            self.outputs.iter().map(|o| (o.level, o.colour)).collect();
        f.debug_struct("LineLayer")
            .field("emojify", &self.emojify)
            .field("outputs", &outputs)
            .finish_non_exhaustive()
    }
}

impl LineLayer {
    /// A layer with no output yet, reading the time from `clock`.
    pub fn new(emojify: bool, clock: Arc<dyn Clock>) -> LineLayer {
        LineLayer {
            emojify,
            clock,
            outputs: Vec::new(),
        }
    }

    /// The layer with one more output, which shows `level` and the more severe levels, coloured
    /// when `colour` is set.
    pub fn output(
        mut self,
        level: Level,
        colour: bool,
        writer: Box<dyn Write + Send>,
    ) -> LineLayer {
        self.outputs.push(Output {
            level,
            colour,
            writer: Mutex::new(writer),
        });
        self
    }
}

impl<S: Subscriber> Layer<S> for LineLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        let level = match fields.severity.as_deref() {
            Some(name) => Level::from_name(name),
            None => match *event.metadata().level() {
                tracing::Level::ERROR => Some(Level::Error),
                tracing::Level::WARN => Some(Level::Warning),
                tracing::Level::INFO => Some(Level::Info),
                tracing::Level::DEBUG => Some(Level::Debug),
                tracing::Level::TRACE => None,
            },
        };
        let Some(level) = level else {
            return;
        };
        let text = format!("{}{}", fields.message, fields.extra);
        if text.is_empty() {
            return;
        }
        let text = format::emojify(&text, self.emojify);
        let now = self.clock.now_ms();
        for output in self.outputs.iter().filter(|output| level <= output.level) {
            let Some(line) = format::line(level, now, &text, output.colour) else {
                continue;
            };
            if let Ok(mut writer) = output.writer.lock() {
                // A line that cannot be written cannot be reported either.
                let _ = writer.write_all(line.as_bytes());
            }
        }
    }
}

/// The message, the other fields as ` name=value`, and the reference level of an event.
#[derive(Default)]
struct Fields {
    message: String,
    extra: String,
    severity: Option<String>,
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "severity" => self.severity = Some(value.to_owned()),
            "message" => self.message.push_str(value),
            name => {
                let _ = write!(self.extra, " {name}={value}");
            }
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let _ = match field.name() {
            "message" => write!(self.message, "{value:?}"),
            name => write!(self.extra, " {name}={value:?}"),
        };
    }
}
