//! Logging this mod can filter, format, and watch from inside itself.
//!
//! A line goes to the loader's log, which is where a mod's output already went; what this
//! adds is everything around it: a level filter, sinks that see records before they are
//! written, a short history, structured fields, and error capture for
//! [`crate::diagnostics`].
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! log::set_level(Level::Debug);
//! log::info("loading world overworld");
//! log::with_context("world=overworld", || log::debug("inside the context"));
//! log::event("block_placed", &[("x", "1"), ("y", "64"), ("z", "-3")]);
//! info!("the macros go through the same pipeline, so filters and sinks see them");
//! ```
//!
//! The macros [`crate::info!`], [`crate::warn!`] and [`crate::error!`] go through the same
//! pipeline, so a filter or a sink sees them too.

use std::error::Error;
use std::fmt;
use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// How loud a line is. The order is the one a filter compares against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    /// Nothing at all.
    Off,
}

impl Level {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Fatal => "fatal",
            Self::Off => "off",
        }
    }

    /// The code the loader's own log takes, which only knows three of these.
    #[must_use]
    pub fn host_code(self) -> u32 {
        match self {
            Self::Error | Self::Fatal => crate::level::ERROR,
            Self::Warn => crate::level::WARN,
            _ => crate::level::INFO,
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One line, as a sink sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub level: Level,
    /// What produced it: the mod's id, or a logger's name.
    pub target: String,
    pub message: String,
    /// Fields, as `key=value`, for a structured line.
    pub fields: Vec<(String, String)>,
    /// The context compiled by [`with_context`].
    pub context: Vec<String>,
    /// Milliseconds since the mod loaded, for ordering lines by hand.
    pub elapsed_millis: u64,
}

impl Record {
    /// The line as text, which is what the loader writes.
    #[must_use]
    pub fn render(&self) -> String {
        let mut line = String::new();
        if self.target.is_empty() {
            line.push_str(&format!("[{}] ", self.level));
        } else {
            line.push_str(&format!("[{} {}] ", self.level, self.target));
        }
        line.push_str(&self.message);
        if !self.fields.is_empty() {
            line.push(' ');
            line.push_str(
                &self
                    .fields
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        if !self.context.is_empty() {
            line.push_str(&format!(" ({})", self.context.join(" ")));
        }
        line
    }
}

impl fmt::Display for Record {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render())
    }
}

type Sink = Box<dyn FnMut(&Record) + Send>;
type Flusher = Box<dyn FnMut() + Send>;

struct SinkEntry {
    name: String,
    write: Sink,
    flush: Option<Flusher>,
}

/// What the log level, the filters and the sinks live in.
#[derive(Default)]
struct State {
    level: Option<Level>,
    filters: Vec<(String, Level)>,
    sinks: Vec<SinkEntry>,
    recent: Vec<Record>,
    errors: Vec<Record>,
    context: Vec<String>,
    capturing: bool,
    written: u64,
    dropped: u64,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn started() -> &'static std::time::Instant {
    static STARTED: OnceLock<std::time::Instant> = OnceLock::new();
    STARTED.get_or_init(std::time::Instant::now)
}

/// How many records the history keeps.
pub const HISTORY: usize = 256;

fn target() -> String {
    crate::metadata::id()
}

fn passes(level: Level, target: &str) -> bool {
    let held = match state().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    let mut allowed = held.level.unwrap_or(Level::Info);
    for (pattern, filter) in &held.filters {
        // A filter can make a target louder than the default, never quieter: silencing
        // another mod is the loader's business, not a mod's.
        if pattern_matches(pattern, target) && *filter < allowed {
            allowed = *filter;
        }
    }
    level >= allowed && allowed != Level::Off
}

fn pattern_matches(pattern: &str, target: &str) -> bool {
    if pattern == "*" || pattern.is_empty() {
        return true;
    }
    match pattern.strip_suffix('*') {
        Some(prefix) => target.starts_with(prefix),
        None => pattern == target,
    }
}

fn emit(record: Record) {
    if !passes(record.level, &record.target) {
        if let Ok(mut held) = state().lock() {
            held.dropped += 1;
        }
        return;
    }
    let line = record.render();
    {
        let mut held = match state().lock() {
            Ok(held) => held,
            Err(error) => error.into_inner(),
        };
        held.written += 1;
        if held.recent.len() == HISTORY {
            held.recent.remove(0);
        }
        held.recent.push(record.clone());
        if held.capturing && record.level >= Level::Error {
            held.errors.push(record.clone());
        }
        for sink in &mut held.sinks {
            (sink.write)(&record);
        }
    }
    crate::log(record.level.host_code(), &line);
}

/// Writes a line at `level`, on this mod's target.
pub fn log(level: Level, message: &str) {
    emit(Record {
        level,
        target: target(),
        message: message.to_owned(),
        fields: Vec::new(),
        context: current_context(),
        elapsed_millis: started().elapsed().as_millis() as u64,
    });
}

/// Writes a line with fields, for a log a tool will read rather than a person.
pub fn log_fields(level: Level, message: &str, fields: &[(&str, &str)]) {
    emit(Record {
        level,
        target: target(),
        message: message.to_owned(),
        fields: fields
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        context: current_context(),
        elapsed_millis: started().elapsed().as_millis() as u64,
    });
}

pub fn trace(message: &str) {
    log(Level::Trace, message);
}

pub fn debug(message: &str) {
    log(Level::Debug, message);
}

pub fn info(message: &str) {
    log(Level::Info, message);
}

pub fn warn(message: &str) {
    log(Level::Warn, message);
}

pub fn error(message: &str) {
    log(Level::Error, message);
}

/// The last thing a mod says before it stops: logged at error level, and kept whether or
/// not error capture is on.
pub fn fatal(message: &str) {
    let record = Record {
        level: Level::Fatal,
        target: target(),
        message: message.to_owned(),
        fields: Vec::new(),
        context: current_context(),
        elapsed_millis: started().elapsed().as_millis() as u64,
    };
    if let Ok(mut held) = state().lock() {
        held.errors.push(record.clone());
    }
    emit(record);
}

/// A line about something that happened, with fields, for a report that reads them back.
pub fn event(name: &str, fields: &[(&str, &str)]) {
    log_fields(Level::Info, name, fields);
}

/// Runs `body` with `context` added to every line it logs, nested contexts included.
pub fn with_context<T>(context: &str, body: impl FnOnce() -> T) -> T {
    push_context(context);
    let value = body();
    pop_context();
    value
}

/// Adds to the context that every line carries until it is popped.
pub fn push_context(context: &str) {
    if let Ok(mut held) = state().lock() {
        held.context.push(context.to_owned());
    }
}

/// Removes the most recent piece of context.
pub fn pop_context() -> Option<String> {
    state()
        .lock()
        .ok()
        .and_then(|mut held| held.context.pop())
}

#[must_use]
pub fn current_context() -> Vec<String> {
    state()
        .lock()
        .map(|held| held.context.clone())
        .unwrap_or_default()
}

/// The level lines are written at, or filtered out below. `Info` until it is set.
#[must_use]
pub fn get_level() -> Level {
    state()
        .lock()
        .ok()
        .and_then(|held| held.level)
        .unwrap_or(Level::Info)
}

pub fn set_level(level: Level) {
    if let Ok(mut held) = state().lock() {
        held.level = Some(level);
    }
}

/// Adds a filter for one target: `"other_mod"` or `"other_mod*"`, matching the target of the
/// record, `"*"` for everything. The most permissive matching filter wins, so a filter can
/// only make a target louder.
pub fn set_filter(pattern: &str, level: Level) {
    if let Ok(mut held) = state().lock() {
        if let Some(existing) = held
            .filters
            .iter_mut()
            .find(|(held, _)| held == pattern)
        {
            existing.1 = level;
        } else {
            held.filters.push((pattern.to_owned(), level));
        }
    }
}

/// The filters in place.
#[must_use]
pub fn get_filters() -> Vec<(String, Level)> {
    state()
        .lock()
        .map(|held| held.filters.clone())
        .unwrap_or_default()
}

/// Removes a filter, and answers whether there was one.
pub fn clear_filter(pattern: &str) -> bool {
    let Ok(mut held) = state().lock() else {
        return false;
    };
    let before = held.filters.len();
    held.filters.retain(|(held, _)| held != pattern);
    held.filters.len() != before
}

/// Registers a sink: every record the filter lets through is handed to it before it is
/// written. The loader's own log is unaffected.
pub fn register_sink(name: &str, sink: impl FnMut(&Record) + Send + 'static) -> bool {
    register_sink_with_flush(name, sink, None::<fn()>)
}

/// The same, with a flush of its own for [`flush`].
pub fn register_sink_with_flush(
    name: &str,
    sink: impl FnMut(&Record) + Send + 'static,
    flush: Option<impl FnMut() + Send + 'static>,
) -> bool {
    let Ok(mut held) = state().lock() else {
        return false;
    };
    if held.sinks.iter().any(|entry| entry.name == name) {
        return false;
    }
    held.sinks.push(SinkEntry {
        name: name.to_owned(),
        write: Box::new(sink),
        flush: flush.map(|flush| Box::new(flush) as Flusher),
    });
    true
}

/// Removes a sink, and answers whether there was one.
pub fn unregister_sink(name: &str) -> bool {
    let Ok(mut held) = state().lock() else {
        return false;
    };
    let before = held.sinks.len();
    held.sinks.retain(|entry| entry.name != name);
    held.sinks.len() != before
}

/// The names of the sinks in place.
#[must_use]
pub fn sinks() -> Vec<String> {
    state()
        .lock()
        .map(|held| held.sinks.iter().map(|entry| entry.name.clone()).collect())
        .unwrap_or_default()
}

/// Flushes every sink that has something to flush, and answers how many did.
pub fn flush() -> usize {
    let Ok(mut held) = state().lock() else {
        return 0;
    };
    let mut flushed = 0;
    for entry in &mut held.sinks {
        if let Some(flush) = entry.flush.as_mut() {
            flush();
            flushed += 1;
        }
    }
    flushed
}

/// The same as [`flush`], for a caller with several loggers in mind.
pub fn flush_all() -> usize {
    flush()
}

/// The last `count` records that were not filtered out, oldest first.
#[must_use]
pub fn get_recent(count: usize) -> Vec<Record> {
    state()
        .lock()
        .map(|held| {
            let start = held.recent.len().saturating_sub(count);
            held.recent[start..].to_vec()
        })
        .unwrap_or_default()
}

/// How many records were written, and how many the filter dropped.
#[must_use]
pub fn counters() -> (u64, u64) {
    state()
        .lock()
        .map(|held| (held.written, held.dropped))
        .unwrap_or((0, 0))
}

/// Every error and fatal line, the newest last. Kept whether or not capture is on for the
/// ones [`capture_errors`] was on for.
#[must_use]
pub fn captured_errors() -> Vec<Record> {
    state()
        .lock()
        .map(|held| held.errors.clone())
        .unwrap_or_default()
}

pub fn clear_captured_errors() -> usize {
    state()
        .lock()
        .map(|mut held| {
            let count = held.errors.len();
            held.errors.clear();
            count
        })
        .unwrap_or_default()
}

/// Starts or stops keeping every error line for [`crate::diagnostics::get_recent_errors`].
pub fn capture_errors(on: bool) {
    if let Ok(mut held) = state().lock() {
        held.capturing = on;
    }
}

#[must_use]
pub fn is_capturing() -> bool {
    state()
        .lock()
        .map(|held| held.capturing)
        .unwrap_or(false)
}

/// A logger that writes under a name of its own, for a mod with more than one part.
#[derive(Debug, Clone)]
pub struct Logger {
    target: String,
}

impl Logger {
    /// The same as [`get_mod_logger`].
    #[must_use]
    pub fn new(target: &str) -> Self {
        Self {
            target: target.to_owned(),
        }
    }

    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn log(&self, level: Level, message: &str) {
        emit(Record {
            level,
            target: self.target.clone(),
            message: message.to_owned(),
            fields: Vec::new(),
            context: current_context(),
            elapsed_millis: started().elapsed().as_millis() as u64,
        });
    }

    /// A line with fields, rendered as `key=value`.
    pub fn structured(&self, level: Level, message: &str, fields: &[(&str, &str)]) {
        emit(Record {
            level,
            target: self.target.clone(),
            message: message.to_owned(),
            fields: fields
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            context: current_context(),
            elapsed_millis: started().elapsed().as_millis() as u64,
        });
    }
}

/// A logger for one named part of this mod, or for another mod's name.
#[must_use]
pub fn get_mod_logger(id: &str) -> Logger {
    Logger::new(id)
}

/// A logger that writes fields, for lines a tool reads.
#[must_use]
pub fn get_structured_logger(name: &str) -> Logger {
    Logger::new(name)
}

/// An error and everything it was caused by, one line per cause.
#[must_use]
pub fn format_error(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        text.push_str(&format!(": {inner}"));
        cause = inner.source();
    }
    text
}

/// Logs an exception, with the same shape a caught panic gets.
pub fn report_exception(kind: &str, message: &str) {
    log_fields(Level::Error, "exception", &[("kind", kind), ("message", message)]);
}

/// Logs a warning with what and why, so the reader does not have to guess.
pub fn report_warning(what: &str, why: &str) {
    log_fields(Level::Warn, what, &[("reason", why)]);
}

/// Logs that an API is on its way out, and what to use instead.
pub fn report_deprecation(api: &str, replacement: &str) {
    log_fields(
        Level::Warn,
        "deprecated",
        &[("api", api), ("use_instead", replacement)],
    );
}

/// Whether a panic hook has been installed, so it is installed once.
static HOOK: AtomicBool = AtomicBool::new(false);

/// Notes every panic in this mod's log, in addition to the loader's own report. The loader
/// catches the panic and switches the mod off; this only makes sure the line is at hand.
pub fn capture_panics() {
    if HOOK.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panicked".to_owned());
        let place = info
            .location()
            .map(|location| format!("{}:{}", location.file(), location.line()))
            .unwrap_or_else(|| "unknown".to_owned());
        log_fields(Level::Fatal, "panic", &[("message", &message), ("at", &place)]);
        previous(info);
    }));
}

/// The level a code stands for, for a mod reading the loader's own numbering.
#[must_use]
pub fn level_from_code(code: u32) -> Level {
    match code {
        crate::level::ERROR => Level::Error,
        crate::level::WARN => Level::Warn,
        _ => Level::Info,
    }
}

/// The level the filter is at, as a number, for a cheap comparison in a hot loop.
#[must_use]
pub fn level_code() -> u8 {
    get_level() as u8
}

/// Caches whether a level is worth formatting, for a hot path.
#[must_use]
pub fn is_enabled(level: Level) -> bool {
    passes(level, &target())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_drops_a_line_below_its_level() {
        let _serial = crate::test_lock::serial();
        set_level(Level::Warn);
        let (_, dropped_before) = counters();
        log(Level::Info, "not worth writing");
        let (_, dropped_after) = counters();
        assert_eq!(dropped_after, dropped_before + 1);
        log(Level::Error, "worth writing");
        assert!(get_recent(4).iter().any(|record| record.message == "worth writing"));
        set_level(Level::Info);
    }

    #[test]
    fn a_sink_sees_a_record_before_it_is_written() {
        let _serial = crate::test_lock::serial();
        set_level(Level::Info);
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&seen);
        assert!(register_sink("probe", move |record: &Record| {
            into.lock().unwrap().push(record.message.clone());
        }));
        assert!(!register_sink("probe", |_| {}), "names are unique");
        log(Level::Info, "through the sink");
        assert!(seen.lock().unwrap().iter().any(|line| line == "through the sink"));
        assert!(sinks().contains(&"probe".to_owned()));
        assert!(unregister_sink("probe"));
        assert!(!unregister_sink("probe"));
    }

    #[test]
    fn context_travels_with_the_line() {
        let _serial = crate::test_lock::serial();
        set_level(Level::Trace);
        let captured = std::sync::Arc::new(Mutex::new(String::new()));
        let into = std::sync::Arc::clone(&captured);
        register_sink("context-probe", move |record: &Record| {
            if record.message == "inside" {
                *into.lock().unwrap() = record.render();
            }
        });
        with_context("world=overworld", || {
            with_context("tick=7", || log(Level::Debug, "inside"));
        });
        let line = captured.lock().unwrap().clone();
        assert!(line.contains("world=overworld"), "{line}");
        assert!(line.contains("tick=7"), "{line}");
        assert!(current_context().is_empty(), "the context is popped again");
        unregister_sink("context-probe");
        set_level(Level::Info);
    }

    #[test]
    fn an_event_carries_its_fields() {
        let _serial = crate::test_lock::serial();
        set_level(Level::Info);
        event("block_placed", &[("x", "1"), ("y", "64")]);
        let record = get_recent(1).pop().expect("the event was recorded");
        assert_eq!(record.message, "block_placed");
        assert!(record.render().contains("x=1"));
        assert!(record.render().contains("y=64"));
    }

    #[test]
    fn a_per_target_filter_wins_over_the_level() {
        let _serial = crate::test_lock::serial();
        set_level(Level::Error);
        set_filter("noisy*", Level::Trace);
        assert!(passes(Level::Trace, "noisy_mod"));
        assert!(!passes(Level::Trace, "quiet_mod"));
        assert_eq!(get_filters().len(), 1);
        assert!(clear_filter("noisy*"));
        assert!(!passes(Level::Trace, "noisy_mod"));
        set_level(Level::Info);
    }

    #[test]
    fn an_error_chain_reads_as_one_line() {
        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Inner {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("inner")
            }
        }
        impl Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);
        impl fmt::Display for Outer {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("outer")
            }
        }
        impl Error for Outer {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(&self.0)
            }
        }

        assert_eq!(format_error(&Outer(Inner)), "outer: inner");
    }

    #[test]
    fn a_logger_writes_under_its_own_name() {
        let _serial = crate::test_lock::serial();
        set_level(Level::Info);
        let logger = get_structured_logger("hello:assets");
        logger.structured(Level::Info, "loaded", &[("count", "3")]);
        let record = get_recent(1).pop().expect("the line was recorded");
        assert_eq!(record.target, "hello:assets");
        assert_eq!(record.fields, vec![("count".to_owned(), "3".to_owned())]);
    }

    #[test]
    fn captured_errors_are_kept_when_capture_is_on() {
        let _serial = crate::test_lock::serial();
        set_level(Level::Info);
        clear_captured_errors();
        capture_errors(true);
        assert!(is_capturing());
        error("something went wrong");
        assert!(captured_errors().iter().any(|record| record.message == "something went wrong"));
        capture_errors(false);
        assert!(!captured_errors().is_empty(), "the history is not thrown away");
        clear_captured_errors();
    }
}
