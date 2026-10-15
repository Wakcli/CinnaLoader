//! Crash handling and crash reports.
//!
//! A mod installs a panic hook that records the panic and still lets the previous hook
//! report it; the record carries the message, where it happened, a backtrace summary, the
//! thread and the mod that was running. Crashes that share a message and location share a
//! stable signature, so the same failure groups; a policy switches a mod off once it has
//! crashed often enough inside a window.
//!
//! [`std::backtrace::Backtrace::capture`] only fills in frames when the environment asks
//! for them, so a release build may record no frames at all and the summary says so
//! instead of inventing a stack.
//!
//! ```
//! use cinnaloader::crash;
//!
//! crash::clear_history();
//! crash::record_for("hello:probe", "the probe panicked");
//! crash::record_for("hello:probe", "the probe panicked");
//! crash::record_for("hello:probe", "a different failure");
//!
//! let report = crash::get_report();
//! assert_eq!(report.total, 3);
//! assert_eq!(report.groups.len(), 2);
//! assert!(crash::summary().contains("3 crash"));
//!
//! let json = crash::write_report("json").expect("a report");
//! assert!(json.contains("\"total\":3"));
//! ```

use std::fmt;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::serialization::{self, Error as SerializeError, Map, Value};

/// One panic this host recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crash {
    /// Monotonic across the process, assigned when the record is made.
    pub id: u64,
    /// Wall-clock milliseconds since the Unix epoch.
    pub timestamp_millis: u64,
    /// The stable hash of message and location that groups the same failure.
    pub signature: u64,
    pub message: String,
    /// `file:line` from the panic payload, where it carried one.
    pub location: Option<String>,
    /// A short summary of the captured backtrace, or a note that there were no frames.
    pub backtrace: String,
    pub thread: String,
    /// The mod that was running when this happened.
    pub mod_id: String,
}

/// One signature's crashes, with the count and the mods that hit it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub signature: u64,
    pub message: String,
    pub location: Option<String>,
    pub count: usize,
    pub mods: Vec<String>,
}

impl fmt::Display for Group {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "x{} {} [{}] in {}",
            self.count,
            self.message,
            self.signature,
            self.mods.join(", ")
        )
    }
}

/// The records, grouped by signature, with the disable and suspicion lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub total: usize,
    pub records: Vec<Crash>,
    pub groups: Vec<Group>,
    pub disabled: Vec<(String, String)>,
    pub marked: Vec<(String, String)>,
}

/// A point-in-time view of everything this host can reach about a crash.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub at_millis: u64,
    pub thread: String,
    pub records: Vec<Crash>,
    pub groups: Vec<Group>,
    pub thread_dumps: Vec<String>,
    pub recent_events: Vec<String>,
    pub recent_tasks: Vec<String>,
    pub recent_packets: Vec<String>,
    pub recent_resource_changes: Vec<String>,
    pub mod_causes: Vec<(String, usize)>,
    pub changes: Vec<String>,
    pub sections: Vec<(String, String)>,
}

type Handler = Arc<dyn Fn(&Crash) + Send + Sync + 'static>;
type Provider = Arc<dyn Fn() -> Vec<String> + Send + Sync + 'static>;
type Section = Arc<dyn Fn(&Report) -> String + Send + Sync + 'static>;
type Recovery = Arc<dyn Fn(&Crash) -> String + Send + Sync + 'static>;

/// The most backtrace frames a summary keeps.
pub const MAX_FRAMES: usize = 4;
/// How many crashes of one mod inside the window switch it off.
pub const DEFAULT_THRESHOLD: usize = 3;
/// The window the threshold counts over, in milliseconds.
pub const DEFAULT_WINDOW_MILLIS: u64 = 60_000;

struct State {
    next_id: u64,
    records: Vec<Crash>,
    handlers: Vec<(String, Handler)>,
    providers: Vec<(String, Provider)>,
    sections: Vec<(String, Section)>,
    recovery: Vec<(String, Recovery)>,
    marked: Vec<(String, String)>,
    disabled: Vec<(String, String)>,
    threshold: usize,
    window_millis: u64,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            next_id: 0,
            records: Vec::new(),
            handlers: Vec::new(),
            providers: Vec::new(),
            sections: Vec::new(),
            recovery: Vec::new(),
            marked: Vec::new(),
            disabled: Vec::new(),
            threshold: DEFAULT_THRESHOLD,
            window_millis: DEFAULT_WINDOW_MILLIS,
        })
    })
}

fn lock<T>(held: &'static Mutex<T>) -> MutexGuard<'static, T> {
    held.lock().unwrap_or_else(|error| error.into_inner())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned()
}

/// The stable hash of a message and an optional location: two crashes with the same pair
/// answer the same number, and a different message answers a different one.
#[must_use]
pub fn signature_of(message: &str, location: Option<&str>) -> u64 {
    let mut text = String::from(message);
    text.push('\n');
    text.push_str(location.unwrap_or(""));
    serialization::hash(text.as_bytes())
}

fn no_frames() -> String {
    "no frames recorded (a release build without RUST_BACKTRACE=1)".to_owned()
}

/// Captures a short backtrace summary, or says there were no frames.
fn backtrace_summary() -> String {
    let text = std::backtrace::Backtrace::capture().to_string();
    let frames: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.chars().next().is_some_and(|first| first.is_ascii_digit()))
        .take(MAX_FRAMES)
        .collect();
    if frames.is_empty() {
        no_frames()
    } else {
        frames.join(" | ")
    }
}

fn push_crash(mod_id: &str, message: &str, location: Option<String>, backtrace: String) -> Crash {
    let crash = {
        let mut held = lock(state());
        held.next_id += 1;
        Crash {
            id: held.next_id,
            timestamp_millis: now_millis(),
            signature: signature_of(message, location.as_deref()),
            message: message.to_owned(),
            location,
            backtrace,
            thread: thread_name(),
            mod_id: mod_id.to_owned(),
        }
    };
    lock(state()).records.push(crash.clone());
    crash
}

fn dispatch(handlers: &[(String, Handler)], crash: &Crash) {
    for (_, handler) in handlers {
        handler(crash);
    }
}

fn record_panic(info: &PanicHookInfo<'_>) {
    let message = info
        .payload()
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panicked".to_owned());
    let location = info
        .location()
        .map(|place| format!("{}:{}", place.file(), place.line()));
    let crash = push_crash(&crate::metadata::id(), &message, location, backtrace_summary());
    let handlers: Vec<(String, Handler)> = lock(state()).handlers.clone();
    dispatch(&handlers, &crash);
    apply_policy(&crash.mod_id);
    crate::error!(
        "panic recorded: {} at {} on {}",
        crash.message,
        crash.location.as_deref().unwrap_or("unknown"),
        crash.thread
    );
}

/// Records a crash of this mod by hand, and answers its id.
pub fn record(message: &str) -> u64 {
    record_for(&crate::metadata::id(), message)
}

/// Records a crash against a named mod, for a probe or a report being built by hand.
pub fn record_for(mod_id: &str, message: &str) -> u64 {
    let crash = push_crash(mod_id, message, None, no_frames());
    let handlers: Vec<(String, Handler)> = lock(state()).handlers.clone();
    dispatch(&handlers, &crash);
    apply_policy(mod_id);
    crate::error!("crash recorded: {} in {mod_id}", crash.message);
    crash.id
}

/// Installs the panic hook: it records the panic and then calls the previous hook, so the
/// default reporting still happens. Answers false when it was already installed.
pub fn install() -> bool {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return false;
    }
    let previous = std::panic::take_hook();
    *lock(previous_hook()) = Some(previous);
    std::panic::set_hook(Box::new(|info: &PanicHookInfo<'_>| {
        record_panic(info);
        if let Ok(held) = previous_hook().lock() {
            if let Some(previous) = held.as_ref() {
                previous(info);
            }
        }
    }));
    true
}

/// Removes the panic hook and puts the previous one back, so a test does not leak it.
pub fn uninstall() -> bool {
    if !INSTALLED.swap(false, Ordering::SeqCst) {
        return false;
    }
    let previous = lock(previous_hook()).take();
    let _ = std::panic::take_hook();
    if let Some(previous) = previous {
        std::panic::set_hook(previous);
    }
    true
}

#[must_use]
pub fn is_installed() -> bool {
    INSTALLED.load(Ordering::SeqCst)
}

type Hook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

static INSTALLED: AtomicBool = AtomicBool::new(false);

fn previous_hook() -> &'static Mutex<Option<Hook>> {
    static PREVIOUS: OnceLock<Mutex<Option<Hook>>> = OnceLock::new();
    PREVIOUS.get_or_init(|| Mutex::new(None))
}

/// Registers a named handler called for every crash, and makes sure the panic hook is in
/// place so a real panic reaches it. A name is registered once.
pub fn register_handler<F>(name: &str, handler: F) -> bool
where
    F: Fn(&Crash) + Send + Sync + 'static,
{
    {
        let mut held = lock(state());
        if held.handlers.iter().any(|(held, _)| held == name) {
            return false;
        }
        held.handlers.push((name.to_owned(), Arc::new(handler)));
    }
    let _ = install();
    true
}

pub fn unregister_handler(name: &str) -> bool {
    let mut held = lock(state());
    let before = held.handlers.len();
    held.handlers.retain(|(held, _)| held != name);
    held.handlers.len() != before
}

/// The handler names in place.
#[must_use]
pub fn handlers() -> Vec<String> {
    lock(state())
        .handlers
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

fn count_recent(mod_id: &str, at: u64, window: u64) -> usize {
    lock(state())
        .records
        .iter()
        .filter(|crash| {
            crash.mod_id == mod_id && at.saturating_sub(crash.timestamp_millis) <= window
        })
        .count()
}

fn apply_policy(mod_id: &str) -> Option<String> {
    let (threshold, window) = get_policy();
    if threshold == 0 {
        return None;
    }
    let count = count_recent(mod_id, now_millis(), window);
    if count < threshold {
        return None;
    }
    let reason = {
        let mut held = lock(state());
        if held.disabled.iter().any(|(held, _)| held == mod_id) {
            return None;
        }
        let reason = format!("{mod_id} crashed {count} time(s) within {window} ms");
        held.disabled.push((mod_id.to_owned(), reason.clone()));
        reason
    };
    crate::warn!("disabling {mod_id}: {reason}");
    Some(reason)
}

/// Sets how many crashes inside the window switch a mod off. Zero disables the policy.
pub fn set_policy(threshold: usize, window_millis: u64) {
    let mut held = lock(state());
    held.threshold = threshold;
    held.window_millis = window_millis;
}

/// The threshold and the window in place.
#[must_use]
pub fn get_policy() -> (usize, u64) {
    let held = lock(state());
    (held.threshold, held.window_millis)
}

/// Whether this mod has now crashed enough times inside the window to be switched off.
#[must_use]
pub fn should_disable(mod_id: &str) -> bool {
    let (threshold, window) = get_policy();
    threshold > 0 && count_recent(mod_id, now_millis(), window) >= threshold
}

#[must_use]
pub fn is_disabled(mod_id: &str) -> bool {
    lock(state()).disabled.iter().any(|(held, _)| held == mod_id)
}

#[must_use]
pub fn disabled_reason(mod_id: &str) -> Option<String> {
    lock(state())
        .disabled
        .iter()
        .find(|(held, _)| held == mod_id)
        .map(|(_, reason)| reason.clone())
}

/// Every disabled mod and the real reason it was switched off.
#[must_use]
pub fn disabled_mods() -> Vec<(String, String)> {
    lock(state()).disabled.clone()
}

pub fn reenable(mod_id: &str) -> bool {
    let mut held = lock(state());
    let before = held.disabled.len();
    held.disabled.retain(|(held, _)| held != mod_id);
    held.disabled.len() != before
}

/// Marks a mod as worth suspecting, with the reason. A mod is marked once.
pub fn mark_suspicious_mod(mod_id: &str, reason: &str) -> bool {
    let mut held = lock(state());
    if held.marked.iter().any(|(held, _)| held == mod_id) {
        return false;
    }
    held.marked.push((mod_id.to_owned(), reason.to_owned()));
    true
}

#[must_use]
pub fn is_suspicious(mod_id: &str) -> bool {
    lock(state()).marked.iter().any(|(held, _)| held == mod_id)
}

/// Every marked mod and the reason it was marked.
#[must_use]
pub fn suspicious_mods() -> Vec<(String, String)> {
    lock(state()).marked.clone()
}

pub fn clear_suspicious(mod_id: &str) -> bool {
    let mut held = lock(state());
    let before = held.marked.len();
    held.marked.retain(|(held, _)| held != mod_id);
    held.marked.len() != before
}

/// Registers a named source of recent lines a report reads back, for example the events a
/// mod saw. A name is registered once.
pub fn register_context_provider<F>(name: &str, provider: F) -> bool
where
    F: Fn() -> Vec<String> + Send + Sync + 'static,
{
    let mut held = lock(state());
    if held.providers.iter().any(|(held, _)| held == name) {
        return false;
    }
    held.providers.push((name.to_owned(), Arc::new(provider)));
    true
}

pub fn unregister_context_provider(name: &str) -> bool {
    let mut held = lock(state());
    let before = held.providers.len();
    held.providers.retain(|(held, _)| held != name);
    held.providers.len() != before
}

#[must_use]
pub fn context_providers() -> Vec<String> {
    lock(state())
        .providers
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

fn provider_lines(name: &str) -> Vec<String> {
    lock(state())
        .providers
        .iter()
        .find(|(held, _)| held == name)
        .map(|(_, provider)| provider())
        .unwrap_or_default()
}

/// The events a mod recorded, newest last.
#[must_use]
pub fn get_recent_events() -> Vec<String> {
    provider_lines("events")
}

/// What the host's own task queue is doing, then the tasks a mod recorded.
#[must_use]
pub fn get_recent_tasks() -> Vec<String> {
    let metrics = crate::tasks::get_metrics();
    let mut lines = vec![format!(
        "host tasks: {} scheduled, {} pending, {} worker(s) started",
        metrics.scheduled, metrics.pending, metrics.background
    )];
    lines.extend(provider_lines("tasks"));
    lines
}

/// The packets a mod recorded. The client's own packet log is not readable here.
#[must_use]
pub fn get_recent_packets() -> Vec<String> {
    provider_lines("packets")
}

/// The resource changes a mod recorded. The client's own reload history is out of reach.
#[must_use]
pub fn get_recent_resource_changes() -> Vec<String> {
    provider_lines("resources")
}

/// The threads this host knows: the one it runs on and the pool's workers.
#[must_use]
pub fn get_thread_dumps() -> Vec<String> {
    let metrics = crate::tasks::get_metrics();
    vec![
        format!("host main: {}", crate::tasks::current_thread()),
        format!("host workers started: {}", metrics.background),
        format!(
            "host tasks scheduled: {} pending: {}",
            metrics.scheduled, metrics.pending
        ),
    ]
}

/// The last crash's backtrace summary, or a fresh capture when there is none yet.
#[must_use]
pub fn get_stack_trace() -> String {
    match get_last_crash() {
        Some(crash) if !crash.backtrace.is_empty() => crash.backtrace,
        _ => backtrace_summary(),
    }
}

/// The last crash this host recorded, if any.
#[must_use]
pub fn get_last_crash() -> Option<Crash> {
    lock(state()).records.last().cloned()
}

/// Every record, oldest first.
#[must_use]
pub fn get_crash_history() -> Vec<Crash> {
    lock(state()).records.clone()
}

/// Empties the records, the disable list and the suspicion list, and answers how many
/// records went.
pub fn clear_history() -> usize {
    let mut held = lock(state());
    let count = held.records.len();
    held.records.clear();
    held.disabled.clear();
    held.marked.clear();
    count
}

/// How many crashes each mod caused, most first.
#[must_use]
pub fn get_mod_causes() -> Vec<(String, usize)> {
    let mut causes: Vec<(String, usize)> = Vec::new();
    for crash in lock(state()).records.iter() {
        match causes.iter_mut().find(|(mod_id, _)| *mod_id == crash.mod_id) {
            Some((_, count)) => *count += 1,
            None => causes.push((crash.mod_id.clone(), 1)),
        }
    }
    causes.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    causes
}

/// The loader's own timeline, the one change record a crash report can point at.
#[must_use]
pub fn identify_recent_changes() -> Vec<String> {
    let timeline = crate::diagnostics::get_timeline();
    let start = timeline.len().saturating_sub(8);
    timeline[start..]
        .iter()
        .map(|(secs, name)| format!("{secs}s {name}"))
        .collect()
}

/// Registers a named section rendered into a report. A name is registered once.
pub fn register_diagnostic_section<F>(name: &str, section: F) -> bool
where
    F: Fn(&Report) -> String + Send + Sync + 'static,
{
    let mut held = lock(state());
    if held.sections.iter().any(|(held, _)| held == name) {
        return false;
    }
    held.sections.push((name.to_owned(), Arc::new(section)));
    true
}

#[must_use]
pub fn diagnostic_sections() -> Vec<String> {
    lock(state())
        .sections
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

/// Renders every registered section against the current report.
#[must_use]
pub fn render_sections() -> Vec<(String, String)> {
    let report = get_report();
    let sections: Vec<(String, Section)> = lock(state()).sections.clone();
    sections
        .into_iter()
        .map(|(name, section)| {
            let text = section(&report);
            (name, text)
        })
        .collect()
}

/// Registers a named action a caller may run to try to recover. A name is registered once.
pub fn register_recovery_action<F>(name: &str, action: F) -> bool
where
    F: Fn(&Crash) -> String + Send + Sync + 'static,
{
    let mut held = lock(state());
    if held.recovery.iter().any(|(held, _)| held == name) {
        return false;
    }
    held.recovery.push((name.to_owned(), Arc::new(action)));
    true
}

#[must_use]
pub fn recovery_actions() -> Vec<String> {
    lock(state())
        .recovery
        .iter()
        .map(|(name, _)| name.clone())
        .collect()
}

/// Runs a registered recovery against the last crash, and answers what it said.
pub fn run_recovery(name: &str) -> Option<String> {
    let action = lock(state())
        .recovery
        .iter()
        .find(|(held, _)| held == name)
        .map(|(_, action)| Arc::clone(action))?;
    let crash = get_last_crash().unwrap_or_else(|| Crash {
        id: 0,
        timestamp_millis: 0,
        signature: 0,
        message: "no crash recorded".to_owned(),
        location: None,
        backtrace: String::new(),
        thread: String::new(),
        mod_id: String::new(),
    });
    Some(action(&crash))
}

fn group(records: &[Crash]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for crash in records {
        match groups.iter_mut().find(|entry| entry.signature == crash.signature) {
            Some(entry) => {
                entry.count += 1;
                if !entry.mods.contains(&crash.mod_id) {
                    entry.mods.push(crash.mod_id.clone());
                }
            }
            None => groups.push(Group {
                signature: crash.signature,
                message: crash.message.clone(),
                location: crash.location.clone(),
                count: 1,
                mods: vec![crash.mod_id.clone()],
            }),
        }
    }
    groups
}

/// The records grouped by signature, with the real totals.
#[must_use]
pub fn get_report() -> Report {
    let records = get_crash_history();
    let groups = group(&records);
    let (disabled, marked) = {
        let held = lock(state());
        (held.disabled.clone(), held.marked.clone())
    };
    Report {
        total: records.len(),
        records,
        groups,
        disabled,
        marked,
    }
}

fn summary_of(report: &Report) -> String {
    format!(
        "{} crash(es) in {} group(s), {} disabled mod(s), {} marked mod(s)",
        report.total,
        report.groups.len(),
        report.disabled.len(),
        report.marked.len()
    )
}

/// The one-line summary of the current history.
#[must_use]
pub fn summary() -> String {
    summary_of(&get_report())
}

fn report_value(report: &Report) -> Value {
    let mut map = Map::new();
    map.insert("summary", Value::str(summary_of(report)));
    map.insert("total", Value::Int(report.total as i64));
    let groups = report
        .groups
        .iter()
        .map(|entry| {
            let mut group = Map::new();
            group.insert("signature", Value::Int(entry.signature as i64));
            group.insert("message", Value::str(entry.message.as_str()));
            group.insert(
                "location",
                entry
                    .location
                    .as_deref()
                    .map_or(Value::Null, Value::str),
            );
            group.insert("count", Value::Int(entry.count as i64));
            group.insert(
                "mods",
                Value::List(
                    entry
                        .mods
                        .iter()
                        .map(|mod_id| Value::str(mod_id.as_str()))
                        .collect(),
                ),
            );
            Value::Map(group)
        })
        .collect();
    map.insert("groups", Value::List(groups));
    Value::Map(map)
}

/// The report as text or JSON, through [`crate::serialization`]. Any other name is JSON.
pub fn write_report(format: &str) -> Result<String, SerializeError> {
    let value = report_value(&get_report());
    match format {
        "text" => Ok(serialization::to_text(&value)),
        _ => serialization::to_json(&value),
    }
}

/// Writes the report to a file, and answers where it went. With no path, it goes into the
/// mod's data directory as `crash-report.txt`.
pub fn export_report(path: Option<&Path>) -> std::io::Result<PathBuf> {
    let path = match path {
        Some(path) => path.to_owned(),
        None => {
            let dir = crate::metadata::data_directory().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no directory is known for this mod",
                )
            })?;
            std::fs::create_dir_all(&dir)?;
            dir.join("crash-report.txt")
        }
    };
    let text = write_report("text")
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    std::fs::write(&path, text)?;
    Ok(path)
}

/// A snapshot of the records, the host threads and everything a provider recorded.
#[must_use]
pub fn capture_snapshot() -> Snapshot {
    let report = get_report();
    Snapshot {
        at_millis: now_millis(),
        thread: thread_name(),
        records: report.records,
        groups: report.groups,
        thread_dumps: get_thread_dumps(),
        recent_events: get_recent_events(),
        recent_tasks: get_recent_tasks(),
        recent_packets: get_recent_packets(),
        recent_resource_changes: get_recent_resource_changes(),
        mod_causes: get_mod_causes(),
        changes: identify_recent_changes(),
        sections: render_sections(),
    }
}

/// Why a question about the client's own crash surface has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "crash_reporter" => {
            "the client's own crash reporter, the window it shows when it dies, belongs to \
             the client; this host cannot write a line into it"
                .to_owned()
        }
        "crash_log" => {
            "the client's crash log is a file the client owns in its own directory; this \
             host cannot read or write it"
                .to_owned()
        }
        other => format!("{other} is not something this host can reach about a crash"),
    }
}

/// The two things this host refuses because the client owns them.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["crash_reporter", "crash_log"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own crash surface. Every entry warns and answers nothing.
pub mod known_only_to_the_client {
    use super::why_unknown;

    /// Writes a line into the client's crash reporter.
    pub fn write_crash_report(_text: &str) -> bool {
        crate::warn!(
            "a mod asked this host to write into the client's crash reporter: {}",
            why_unknown("crash_reporter")
        );
        false
    }

    /// Reads the client's crash log.
    #[must_use]
    pub fn read_crash_log() -> Option<String> {
        crate::warn!(
            "a mod asked this host for the client's crash log: {}",
            why_unknown("crash_log")
        );
        None
    }

    /// The path of the client's crash log.
    #[must_use]
    pub fn crash_log_path() -> Option<std::path::PathBuf> {
        crate::warn!(
            "a mod asked this host for the client's crash log path: {}",
            why_unknown("crash_log")
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_hook_records_the_panic_and_still_reports() {
        let _serial = crate::test_lock::serial();
        clear_history();
        assert!(install());
        assert!(!install(), "the hook is installed once");
        let caught = std::panic::catch_unwind(|| panic!("boom in the probe"));
        assert!(caught.is_err());
        let crash = get_last_crash().expect("the panic was recorded");
        assert_eq!(crash.message, "boom in the probe");
        assert_eq!(crash.mod_id, crate::metadata::id());
        assert!(crash.location.is_some(), "the payload carried a location");
        assert!(!crash.thread.is_empty());
        assert!(!crash.backtrace.is_empty(), "the summary is never blank");
        assert!(uninstall(), "the previous hook is put back");
    }

    #[test]
    fn the_same_crash_signs_the_same_and_a_different_one_does_not() {
        let _serial = crate::test_lock::serial();
        clear_history();
        record_for("hello:same", "same failure");
        record_for("hello:same", "same failure");
        record_for("hello:same", "different failure");
        let history = get_crash_history();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].signature, history[1].signature);
        assert_ne!(history[0].signature, history[2].signature);
        let report = get_report();
        assert_eq!(report.total, 3);
        assert_eq!(report.groups.len(), 2);
        assert_eq!(report.groups[0].count, 2);
        assert_eq!(report.groups[1].count, 1);
    }

    #[test]
    fn the_policy_disables_at_the_threshold_not_before() {
        let _serial = crate::test_lock::serial();
        clear_history();
        set_policy(3, 60_000);
        record_for("hello:policy", "flaky");
        record_for("hello:policy", "flaky");
        assert!(!should_disable("hello:policy"), "two of three does not disable");
        assert!(!is_disabled("hello:policy"));
        record_for("hello:policy", "flaky");
        assert!(should_disable("hello:policy"), "the third does");
        assert!(is_disabled("hello:policy"));
        assert!(disabled_reason("hello:policy").is_some());
        assert_eq!(disabled_mods().len(), 1);
        assert!(reenable("hello:policy"));
        assert!(!is_disabled("hello:policy"));
        set_policy(DEFAULT_THRESHOLD, DEFAULT_WINDOW_MILLIS);
    }

    #[test]
    fn the_report_groups_and_totals() {
        let _serial = crate::test_lock::serial();
        clear_history();
        record_for("hello:report", "the same");
        record_for("hello:report", "the same");
        record_for("hello:report", "another");
        assert_eq!(get_report().total, 3);
        assert_eq!(get_report().groups.len(), 2);
        assert!(summary().contains("3 crash"), "{}", summary());
        let json = write_report("json").expect("a json report");
        assert!(json.contains("\"total\":3"), "{json}");
        assert!(json.contains("\"count\":2"), "{json}");
        let text = write_report("text").expect("a text report");
        assert!(text.contains("the same"), "{text}");
        assert!(text.contains("3"), "{text}");
    }

    #[test]
    fn a_context_provider_feeds_a_recent_list() {
        let _serial = crate::test_lock::serial();
        clear_history();
        assert!(register_context_provider("events", || vec!["tick 7".to_owned()]));
        assert!(
            !register_context_provider("events", Vec::<String>::new),
            "a name is taken once"
        );
        assert_eq!(get_recent_events(), vec!["tick 7".to_owned()]);
        let tasks = get_recent_tasks();
        assert!(tasks[0].contains("host tasks:"), "{tasks:?}");
        assert!(unregister_context_provider("events"));
        assert!(get_recent_events().is_empty());
    }

    #[test]
    fn marking_a_suspicious_mod_keeps_the_reason() {
        let _serial = crate::test_lock::serial();
        clear_history();
        assert!(mark_suspicious_mod("hello:bad", "it writes outside its directory"));
        assert!(!mark_suspicious_mod("hello:bad", "twice"));
        assert!(is_suspicious("hello:bad"));
        assert_eq!(suspicious_mods()[0].1, "it writes outside its directory");
        assert!(clear_suspicious("hello:bad"));
        assert!(!is_suspicious("hello:bad"));
    }

    #[test]
    fn the_client_crash_reporter_and_log_are_refused() {
        let refused = refusals();
        assert_eq!(refused.len(), 2);
        assert!(refused.iter().any(|(what, _)| what == "crash_reporter"));
        assert!(refused.iter().any(|(what, _)| what == "crash_log"));
        assert!(!why_unknown("crash_log").is_empty());
        assert!(!known_only_to_the_client::write_crash_report("x"));
        assert_eq!(known_only_to_the_client::read_crash_log(), None);
        assert_eq!(known_only_to_the_client::crash_log_path(), None);
    }

    #[test]
    fn exporting_writes_the_report_somewhere() {
        let _serial = crate::test_lock::serial();
        clear_history();
        record_for("hello:export", "exported");
        let path = std::env::temp_dir().join("cinnaloader-crash-probe.txt");
        let written = export_report(Some(&path)).expect("the report is writable");
        let text = std::fs::read_to_string(&written).expect("and readable");
        assert!(text.contains("exported"), "{text}");
        let _ = std::fs::remove_file(written);
    }
}
