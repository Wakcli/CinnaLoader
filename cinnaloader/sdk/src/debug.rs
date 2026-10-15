//! Debug and developer tools a mod drives itself, and the client's own developer surface
//! refused.
//!
//! Six things are real here. Named **flags** carry a category and a default, and setting one
//! to the value it already holds reports no change. Named **readouts** are ordered text a mod
//! writes; [`flush`] puts the same text on the loader's overlay line. A named **watch** keeps a
//! ring of a value's history with its last, min, max and average, and flags whether the last
//! sample changed. A **console** queues command lines, dispatches the known ones to registered
//! handlers and refuses an unknown one by name, keeping an output log. **Snapshots** write the
//! module's own state through [`crate::serialization`], read back, and diff. Named
//! **invariants** are checks the caller runs, each answering pass or fail with a message.
//!
//! What needs the client — its debug overlay, its own F3 flags, its console and its internal
//! state dump — is refused in [`known_only_to_the_client`], each with a sentence in
//! [`why_unknown`] and listed by [`refusals`].
//!
//! ```
//! use cinnaloader::debug;
//!
//! assert!(debug::enable());
//! assert!(debug::define_flag("trace_calls", "perf", false));
//! assert!(debug::set_flag("trace_calls", true));
//! assert!(!debug::set_flag("trace_calls", true), "the same value is no change");
//!
//! assert!(debug::set_readout("fps", "FPS", "60", 0));
//! assert_eq!(debug::render_readouts(), vec!["FPS: 60".to_owned()]);
//!
//! assert_eq!(debug::watch("tick_ms", 16.0).count, 1);
//! assert_eq!(debug::watch("tick_ms", 20.0).average, 18.0);
//!
//! assert!(debug::register_command("ping", |_: &[String]| "pong".to_owned()));
//! assert_eq!(debug::dispatch("ping"), "pong");
//! assert_eq!(debug::dispatch("nope"), "unknown command: nope");
//!
//! let before = debug::snapshot();
//! assert!(debug::set_flag("trace_calls", false));
//! assert_eq!(
//!     debug::diff_snapshots(&before, &debug::snapshot()),
//!     vec!["flags.trace_calls.value: true -> false".to_owned()]
//! );
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::serialization::{self, Map, Value};

/// How many samples a watch keeps before the oldest is dropped.
pub const WATCH_HISTORY: usize = 64;
/// How many lines the console's output log keeps.
pub const CONSOLE_LOG: usize = 256;
/// How many invariant results are kept for [`check_all_results`].
pub const RESULT_HISTORY: usize = 256;
/// How many traces are kept.
pub const TRACE_HISTORY: usize = 256;

type Command = Arc<dyn Fn(&[String]) -> String + Send + Sync>;
type Invariant = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;
type Inspector = Arc<dyn Fn() -> String + Send + Sync>;

/// One named on/off flag, with the category it belongs to and the value it started at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub name: String,
    pub category: String,
    pub value: bool,
    pub default: bool,
}

/// One named line a mod shows, ordered by `order` and then by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readout {
    pub name: String,
    pub label: String,
    pub value: String,
    pub unit: String,
    pub order: i32,
}

impl Readout {
    /// The line as text: `label: value`, with ` unit` appended when there is one.
    #[must_use]
    pub fn render(&self) -> String {
        if self.unit.is_empty() {
            format!("{}: {}", self.label, self.value)
        } else {
            format!("{}: {} {}", self.label, self.value, self.unit)
        }
    }
}

/// What a watch's window says right now.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WatchStats {
    pub count: usize,
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub average: f64,
    /// Whether the last sample differs from the one before it.
    pub changed: bool,
}

/// What one invariant answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// One completed call, as [`trace_call`] records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRecord {
    pub name: String,
    pub micros: u128,
}

#[derive(Default)]
struct State {
    enabled: bool,
    flags: BTreeMap<String, Flag>,
    readouts: BTreeMap<String, Readout>,
    watches: BTreeMap<String, Vec<f64>>,
    commands: BTreeMap<String, Command>,
    queue: Vec<String>,
    output: Vec<String>,
    invariants: BTreeMap<String, Invariant>,
    results: Vec<InvariantResult>,
    failures: Vec<(String, String)>,
    breakpoints: Vec<String>,
    traces: Vec<TraceRecord>,
    inspectors: BTreeMap<String, Inspector>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn with<T>(body: impl FnOnce(&mut State) -> T) -> T {
    let mut held = state().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut held)
}

/// Whether the debug tools are on. Off until [`enable`].
#[must_use]
pub fn is_enabled() -> bool {
    with(|s| s.enabled)
}

/// Turns them on, and answers whether that changed anything.
pub fn enable() -> bool {
    with(|s| {
        let changed = !s.enabled;
        s.enabled = true;
        changed
    })
}

/// Turns them off, and answers whether that changed anything.
pub fn disable() -> bool {
    with(|s| {
        let changed = s.enabled;
        s.enabled = false;
        changed
    })
}

/// Declares a flag with a category and a default. A name is defined once.
pub fn define_flag(name: &str, category: &str, default: bool) -> bool {
    with(|s| {
        if s.flags.contains_key(name) {
            return false;
        }
        s.flags.insert(
            name.to_owned(),
            Flag {
                name: name.to_owned(),
                category: category.to_owned(),
                value: default,
                default,
            },
        );
        true
    })
}

/// Sets a flag, and answers whether that changed it. Setting a flag to the value it already
/// holds reports no change.
pub fn set_flag(name: &str, on: bool) -> bool {
    with(|s| match s.flags.get_mut(name) {
        Some(flag) => {
            let changed = flag.value != on;
            flag.value = on;
            changed
        }
        None => false,
    })
}

/// [`set_flag`] on.
pub fn enable_flag(name: &str) -> bool {
    set_flag(name, true)
}

/// [`set_flag`] off.
pub fn disable_flag(name: &str) -> bool {
    set_flag(name, false)
}

/// A flag's value.
#[must_use]
pub fn get_flag(name: &str) -> Option<bool> {
    with(|s| s.flags.get(name).map(|flag| flag.value))
}

/// Every flag, ordered by name.
#[must_use]
pub fn flags() -> Vec<Flag> {
    with(|s| s.flags.values().cloned().collect())
}

/// The flags whose value differs from their default, ordered by name.
#[must_use]
pub fn changed_flags() -> Vec<String> {
    with(|s| {
        s.flags
            .values()
            .filter(|flag| flag.value != flag.default)
            .map(|flag| flag.name.clone())
            .collect()
    })
}

/// Puts every flag back to its default, and answers how many moved.
pub fn reset_flags() -> usize {
    with(|s| {
        let mut moved = 0;
        for flag in s.flags.values_mut() {
            if flag.value != flag.default {
                flag.value = flag.default;
                moved += 1;
            }
        }
        moved
    })
}

/// Writes a readout with no unit, replacing one of the same name.
pub fn set_readout(name: &str, label: &str, value: &str, order: i32) -> bool {
    set_readout_unit(name, label, value, "", order)
}

/// Writes a readout with a unit, replacing one of the same name. Answers whether it is new.
pub fn set_readout_unit(name: &str, label: &str, value: &str, unit: &str, order: i32) -> bool {
    with(|s| {
        s.readouts
            .insert(
                name.to_owned(),
                Readout {
                    name: name.to_owned(),
                    label: label.to_owned(),
                    value: value.to_owned(),
                    unit: unit.to_owned(),
                    order,
                },
            )
            .is_none()
    })
}

/// Forgets a readout, and answers whether there was one.
pub fn remove_readout(name: &str) -> bool {
    with(|s| s.readouts.remove(name).is_some())
}

/// Every readout, ordered by `order` and then by name.
#[must_use]
pub fn readouts() -> Vec<Readout> {
    let mut all: Vec<Readout> = with(|s| s.readouts.values().cloned().collect());
    all.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.name.cmp(&right.name))
    });
    all
}

/// The readouts rendered as text, in their order, exact.
#[must_use]
pub fn render_readouts() -> Vec<String> {
    readouts().iter().map(Readout::render).collect()
}

/// Puts every readout on the loader's overlay line and answers how many went. This is the
/// half of an overlay that this host has.
pub fn flush() -> usize {
    let lines = render_readouts();
    for line in &lines {
        crate::overlay_line(line);
    }
    lines.len()
}

/// Adds a sample to a watch, creating it if needed, and answers the window's statistics.
pub fn watch(name: &str, value: f64) -> WatchStats {
    with(|s| {
        let history = s.watches.entry(name.to_owned()).or_default();
        if history.len() == WATCH_HISTORY {
            history.remove(0);
        }
        history.push(value);
        stats_of(history)
    })
}

fn stats_of(history: &[f64]) -> WatchStats {
    let Some((&first, _)) = history.split_first() else {
        return WatchStats::default();
    };
    let (mut min, mut max, mut sum) = (first, first, 0.0f64);
    for &value in history {
        if value < min {
            min = value;
        }
        if value > max {
            max = value;
        }
        sum += value;
    }
    let count = history.len();
    // sum() seeds with -0.0 on some targets, so a zero average is normalised here.
    let average = if sum == 0.0 { 0.0 } else { sum / count as f64 };
    let changed = count >= 2 && history[count - 1] != history[count - 2];
    WatchStats {
        count,
        last: history[count - 1],
        min,
        max,
        average,
        changed,
    }
}

/// Every sample a watch still holds, oldest first.
#[must_use]
pub fn watch_history(name: &str) -> Vec<f64> {
    with(|s| s.watches.get(name).cloned().unwrap_or_default())
}

/// A watch's window, or `None` when nothing was watched under that name.
#[must_use]
pub fn watch_stats(name: &str) -> Option<WatchStats> {
    with(|s| s.watches.get(name).map(|history| stats_of(history)))
}

/// Whether a watch's last sample differs from the one before it.
#[must_use]
pub fn watch_changed(name: &str) -> bool {
    watch_stats(name).is_some_and(|stats| stats.changed)
}

/// Forgets a watch, and answers whether there was one.
pub fn clear_watch(name: &str) -> bool {
    with(|s| s.watches.remove(name).is_some())
}

/// The watches in place, ordered by name.
#[must_use]
pub fn watches() -> Vec<String> {
    with(|s| s.watches.keys().cloned().collect())
}

/// Registers a command handler. A name is taken once.
pub fn register_command<F>(name: &str, handler: F) -> bool
where
    F: Fn(&[String]) -> String + Send + Sync + 'static,
{
    with(|s| {
        if s.commands.contains_key(name) {
            return false;
        }
        s.commands.insert(name.to_owned(), Arc::new(handler));
        true
    })
}

/// Forgets a command, and answers whether there was one.
pub fn unregister_command(name: &str) -> bool {
    with(|s| s.commands.remove(name).is_some())
}

/// The registered command names, ordered by name.
#[must_use]
pub fn commands() -> Vec<String> {
    with(|s| s.commands.keys().cloned().collect())
}

/// Queues a command line and answers how many are waiting.
pub fn queue_command(line: &str) -> usize {
    with(|s| {
        s.queue.push(line.to_owned());
        s.queue.len()
    })
}

/// The queued command lines, oldest first.
#[must_use]
pub fn pending_commands() -> Vec<String> {
    with(|s| s.queue.clone())
}

/// Dispatches every queued line: a known name goes to its handler, an unknown one is refused
/// by name. Every line's output is recorded. Answers how many ran.
pub fn run_commands() -> usize {
    let queued = with(|s| std::mem::take(&mut s.queue));
    let mut ran = 0;
    for line in queued {
        let mut parts = line.split_whitespace();
        let name = parts.next().unwrap_or("");
        let args: Vec<String> = parts.map(str::to_owned).collect();
        let handler = with(|s| s.commands.get(name).cloned());
        let output = match handler {
            Some(handler) => handler(&args),
            None => format!("unknown command: {name}"),
        };
        with(|s| {
            if s.output.len() == CONSOLE_LOG {
                s.output.remove(0);
            }
            s.output.push(output);
        });
        ran += 1;
    }
    ran
}

/// Queues one line, runs it and answers its output.
pub fn dispatch(line: &str) -> String {
    with(|s| s.queue.push(line.to_owned()));
    run_commands();
    with(|s| s.output.last().cloned().unwrap_or_default())
}

/// The console's output log, oldest first.
#[must_use]
pub fn console_output() -> Vec<String> {
    with(|s| s.output.clone())
}

/// Empties the output log, and answers how many lines went.
pub fn clear_console() -> usize {
    with(|s| std::mem::take(&mut s.output).len())
}

/// Registers an inspector: a named provider of text this host can call. The client's own
/// inspector screen cannot be opened; see [`why_unknown`].
pub fn register_inspector<F>(name: &str, provider: F) -> bool
where
    F: Fn() -> String + Send + Sync + 'static,
{
    with(|s| {
        if s.inspectors.contains_key(name) {
            return false;
        }
        s.inspectors.insert(name.to_owned(), Arc::new(provider));
        true
    })
}

/// Forgets an inspector, and answers whether there was one.
pub fn unregister_inspector(name: &str) -> bool {
    with(|s| s.inspectors.remove(name).is_some())
}

/// The inspector names, ordered by name.
#[must_use]
pub fn inspectors() -> Vec<String> {
    with(|s| s.inspectors.keys().cloned().collect())
}

/// Asks an inspector for its text.
#[must_use]
pub fn inspect(name: &str) -> Option<String> {
    let provider = with(|s| s.inspectors.get(name).cloned())?;
    Some(provider())
}

/// Registers a named invariant check. A name is registered once.
pub fn register_invariant<F>(name: &str, check: F) -> bool
where
    F: Fn() -> Result<(), String> + Send + Sync + 'static,
{
    with(|s| {
        if s.invariants.contains_key(name) {
            return false;
        }
        s.invariants.insert(name.to_owned(), Arc::new(check));
        true
    })
}

/// Forgets an invariant, and answers whether there was one.
pub fn unregister_invariant(name: &str) -> bool {
    with(|s| s.invariants.remove(name).is_some())
}

/// The invariant names, ordered by name.
#[must_use]
pub fn invariants() -> Vec<String> {
    with(|s| s.invariants.keys().cloned().collect())
}

fn evaluate(name: &str, check: &Invariant) -> InvariantResult {
    match check() {
        Ok(()) => InvariantResult {
            name: name.to_owned(),
            passed: true,
            message: "ok".to_owned(),
        },
        Err(message) => InvariantResult {
            name: name.to_owned(),
            passed: false,
            message,
        },
    }
}

fn record_result(s: &mut State, result: InvariantResult) {
    if !result.passed {
        s.failures
            .push((result.name.clone(), result.message.clone()));
    }
    if s.results.len() == RESULT_HISTORY {
        s.results.remove(0);
    }
    s.results.push(result);
}

/// Runs one invariant, records what it answered, and answers that.
#[must_use]
pub fn run_invariant(name: &str) -> Option<InvariantResult> {
    let check = with(|s| s.invariants.get(name).cloned())?;
    let result = evaluate(name, &check);
    with(|s| record_result(s, result.clone()));
    Some(result)
}

/// Runs every invariant, records the results, and answers how many failed.
pub fn check_all() -> usize {
    let checks: Vec<(String, Invariant)> = with(|s| {
        s.invariants
            .iter()
            .map(|(name, check)| (name.clone(), check.clone()))
            .collect()
    });
    let results: Vec<InvariantResult> = checks
        .iter()
        .map(|(name, check)| evaluate(name, check))
        .collect();
    let failed = results.iter().filter(|result| !result.passed).count();
    with(|s| {
        for result in results {
            record_result(s, result);
        }
    });
    failed
}

/// The invariant results recorded so far, oldest first.
#[must_use]
pub fn check_all_results() -> Vec<InvariantResult> {
    with(|s| s.results.clone())
}

/// The recorded failures, oldest first: assertions and invariants together.
#[must_use]
pub fn failures() -> Vec<(String, String)> {
    with(|s| s.failures.clone())
}

/// Empties the failure list, and answers how many went.
pub fn clear_failures() -> usize {
    with(|s| std::mem::take(&mut s.failures).len())
}

/// Records the caller's file and line, and answers how many breakpoints have been noted.
#[track_caller]
pub fn breakpoint() -> usize {
    let at = std::panic::Location::caller();
    with(|s| {
        s.breakpoints.push(format!("{}:{}", at.file(), at.line()));
        s.breakpoints.len()
    })
}

/// The breakpoints noted so far, oldest first.
#[must_use]
pub fn breakpoints() -> Vec<String> {
    with(|s| s.breakpoints.clone())
}

/// Checks a condition. A false one is recorded and warned about, and `false` is answered.
#[track_caller]
#[must_use]
pub fn assert(condition: bool, message: &str) -> bool {
    if condition {
        return true;
    }
    let at = std::panic::Location::caller();
    with(|s| {
        s.failures.push((
            format!("assert at {}:{}", at.file(), at.line()),
            message.to_owned(),
        ))
    });
    crate::warn!("assertion failed at {}:{}: {message}", at.file(), at.line());
    false
}

/// Checks that this is the thread named `expected`.
#[track_caller]
#[must_use]
pub fn assert_thread(expected: &str) -> bool {
    let actual = crate::tasks::current_thread();
    if actual == expected {
        return true;
    }
    let at = std::panic::Location::caller();
    with(|s| {
        s.failures.push((
            format!("thread at {}:{}", at.file(), at.line()),
            format!("expected {expected}, on {actual}"),
        ))
    });
    crate::warn!("thread check failed at {}:{}: expected {expected}, this is {actual}", at.file(), at.line());
    false
}

/// Checks that this is the thread a mod runs on.
#[must_use]
pub fn assert_main_thread() -> bool {
    if crate::tasks::is_main_thread() {
        return true;
    }
    let actual = crate::tasks::current_thread();
    with(|s| {
        s.failures
            .push(("main thread".to_owned(), format!("this is {actual}")));
    });
    crate::warn!("main-thread check failed: this is {actual}");
    false
}

/// A call being traced, from [`trace_call`] until it is dropped.
pub struct Trace {
    name: String,
    at: std::time::Instant,
}

impl Trace {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        let micros = self.at.elapsed().as_micros();
        with(|s| {
            if s.traces.len() == TRACE_HISTORY {
                s.traces.remove(0);
            }
            s.traces.push(TraceRecord {
                name: self.name.clone(),
                micros,
            });
        });
    }
}

/// Starts tracing a call. The record is written when the guard is dropped.
#[must_use]
pub fn trace_call(name: &str) -> Trace {
    Trace {
        name: name.to_owned(),
        at: std::time::Instant::now(),
    }
}

/// The traces recorded so far, oldest first.
#[must_use]
pub fn traces() -> Vec<TraceRecord> {
    with(|s| s.traces.clone())
}

fn number(value: f64) -> Value {
    if value.is_finite() {
        Value::Float(value)
    } else {
        Value::Null
    }
}

fn snapshot_value() -> Value {
    with(|s| {
        let mut root = Map::new();
        root.insert("enabled", Value::Bool(s.enabled));

        let mut flags = Map::new();
        for flag in s.flags.values() {
            let mut entry = Map::new();
            entry.insert("value", Value::Bool(flag.value));
            entry.insert("default", Value::Bool(flag.default));
            entry.insert("category", Value::Str(flag.category.clone()));
            flags.insert(flag.name.clone(), Value::Map(entry));
        }
        root.insert("flags", Value::Map(flags));

        let mut readouts = Map::new();
        for readout in s.readouts.values() {
            let mut entry = Map::new();
            entry.insert("label", Value::Str(readout.label.clone()));
            entry.insert("value", Value::Str(readout.value.clone()));
            entry.insert("unit", Value::Str(readout.unit.clone()));
            entry.insert("order", Value::Int(i64::from(readout.order)));
            readouts.insert(readout.name.clone(), Value::Map(entry));
        }
        root.insert("readouts", Value::Map(readouts));

        let mut watches = Map::new();
        for (name, history) in &s.watches {
            let stats = stats_of(history);
            let mut entry = Map::new();
            entry.insert("count", Value::Int(stats.count as i64));
            entry.insert("last", number(stats.last));
            entry.insert("min", number(stats.min));
            entry.insert("max", number(stats.max));
            entry.insert("average", number(stats.average));
            entry.insert("changed", Value::Bool(stats.changed));
            watches.insert(name.clone(), Value::Map(entry));
        }
        root.insert("watches", Value::Map(watches));

        root.insert(
            "invariants",
            Value::List(
                s.invariants
                    .keys()
                    .map(|name| Value::Str(name.clone()))
                    .collect(),
            ),
        );

        Value::Map(root)
    })
}

/// The module's own state as JSON, through [`crate::serialization`].
#[must_use]
pub fn snapshot() -> String {
    let value = snapshot_value();
    serialization::to_json(&value).unwrap_or_else(|_| serialization::to_text(&value))
}

/// The same state as [`snapshot`], under the name the wishlist uses.
#[must_use]
pub fn dump_state() -> String {
    snapshot()
}

/// Reads a snapshot back into a value tree.
pub fn load_snapshot(text: &str) -> Result<Value, serialization::Error> {
    serialization::from_json(text)
}

fn flatten(prefix: &str, value: &Value, out: &mut BTreeMap<String, Value>) {
    match value {
        Value::Map(map) => {
            for (key, held) in map.iter() {
                let path = if prefix.is_empty() {
                    key.to_owned()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&path, held, out);
            }
        }
        other => {
            out.insert(prefix.to_owned(), other.clone());
        }
    }
}

/// Names what changed between two snapshots, one line per leaf: `path: before -> after`.
/// A key only in one side is spelled `(removed)` or `(added)`.
#[must_use]
pub fn diff_snapshots(before: &str, after: &str) -> Vec<String> {
    let (Ok(before), Ok(after)) = (
        serialization::from_json(before),
        serialization::from_json(after),
    ) else {
        return Vec::new();
    };
    let mut left = BTreeMap::new();
    let mut right = BTreeMap::new();
    flatten("", &before, &mut left);
    flatten("", &after, &mut right);

    let keys: BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    let mut lines = Vec::new();
    for key in keys {
        match (left.get(key), right.get(key)) {
            (Some(was), Some(now)) if was == now => {}
            (Some(was), Some(now)) => {
                lines.push(format!("{key}: {} -> {}", text(was), text(now)));
            }
            (Some(was), None) => lines.push(format!("{key}: {} -> (removed)", text(was))),
            (None, Some(now)) => lines.push(format!("{key}: (added) -> {}", text(now))),
            (None, None) => {}
        }
    }
    lines
}

fn text(value: &Value) -> String {
    serialization::to_text(value)
}

/// Writes a snapshot to a file, and answers where it went. With no path it goes into the
/// mod's data directory as `debug-snapshot.json`.
pub fn export_snapshot(path: Option<&Path>) -> std::io::Result<PathBuf> {
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
            dir.join("debug-snapshot.json")
        }
    };
    std::fs::write(&path, snapshot())?;
    Ok(path)
}

/// The other mods the loader worked out: their names, ids and states.
#[must_use]
pub fn dump_mods() -> String {
    let mods = crate::mods::all();
    let entries: Vec<String> = mods
        .iter()
        .map(|entry| format!("{} ({}) {}", entry.name, entry.id, entry.state))
        .collect();
    format!("{} mods: {}", entries.len(), entries.join(", "))
}

/// The load order and what each mod waits for.
#[must_use]
pub fn dump_dependencies() -> String {
    let order = crate::mods::get_load_order();
    let entries: Vec<String> = order
        .iter()
        .map(|name| {
            let dependencies = crate::mods::get_dependencies(name);
            if dependencies.is_empty() {
                format!("{name} <- none")
            } else {
                format!("{name} <- {}", dependencies.join(", "))
            }
        })
        .collect();
    let missing = crate::mods::get_missing_dependencies();
    format!(
        "order: {}; missing: {}",
        entries.join(" | "),
        missing.join(", ")
    )
}

/// The task queue's counters.
#[must_use]
pub fn dump_tasks() -> String {
    let metrics = crate::tasks::get_metrics();
    format!(
        "scheduled={} ran={} cancelled={} failed={} repeating={} background={} pending={} queue={}",
        metrics.scheduled,
        metrics.ran,
        metrics.cancelled,
        metrics.failed,
        metrics.repeating,
        metrics.background,
        metrics.pending,
        metrics.queue_length,
    )
}

/// The event kinds and the listeners on them.
#[must_use]
pub fn dump_events() -> String {
    let listeners = crate::events::get_listeners();
    let entries: Vec<String> = listeners
        .iter()
        .map(|listener| {
            format!(
                "{}#{} ({})",
                listener.kind,
                listener.id,
                listener.priority.as_str()
            )
        })
        .collect();
    format!(
        "kinds={} listeners={} {}",
        crate::events::known_kinds().len(),
        listeners.len(),
        entries.join(", ")
    )
}

/// The config keys and their rendered values.
#[must_use]
pub fn dump_config() -> String {
    let keys = crate::config::keys();
    let entries: Vec<String> = keys
        .iter()
        .map(|key| {
            let value = crate::config::get(key)
                .map(|value| value.render())
                .unwrap_or_default();
            format!("{key}={value}")
        })
        .collect();
    format!("{} keys: {}", keys.len(), entries.join(", "))
}

/// The registered registries and their sizes.
#[must_use]
pub fn dump_registry() -> String {
    let names = crate::registry::registered();
    let entries: Vec<String> = names
        .iter()
        .map(|name| {
            let size = crate::registry::get_registry(name)
                .map(|registry| registry.size())
                .unwrap_or(0);
            format!("{name}({size})")
        })
        .collect();
    format!("{} registries: {}", names.len(), entries.join(", "))
}

/// The GPU ledger's resources.
#[must_use]
pub fn dump_resources() -> String {
    let resources = crate::graphics::get_resources();
    let live = resources.iter().filter(|resource| !resource.released).count();
    let entries: Vec<String> = resources
        .iter()
        .map(|resource| format!("{}:{}", resource.label, resource.kind.as_str()))
        .collect();
    format!(
        "{} resources, {} live, {} bytes: {}",
        resources.len(),
        live,
        crate::graphics::get_memory_usage(),
        entries.join(", ")
    )
}

/// Why a question about the client's own developer surface has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "overlay" => {
            "the client draws its own debug overlay, so a mod's lines, boxes, paths, rays and \
             chunk bounds cannot be drawn here; the loader's overlay line is the one place a \
             mod host can put text, and the readouts use it"
                .to_owned()
        }
        "flags" => {
            "hitboxes, entity ids, block ids, memory, fps and tps are the client's own debug \
             flags, switched from its F3 screen; this host cannot flip them"
                .to_owned()
        }
        "console" => {
            "the client's developer console, its profiler and its inspector are the client's \
             own screens; this host has its own console and inspectors a mod drives instead"
                .to_owned()
        }
        "state" => {
            "the client's internal state — its entities, its blocks, its network and its \
             render state — is the client's own; this host can only dump what it owns"
                .to_owned()
        }
        other => format!("{other} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["overlay", "flags", "console", "state"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own developer surface, which this host can neither read nor draw on. Every
/// entry warns and answers nothing.
pub mod known_only_to_the_client {
    use super::why_unknown;

    /// The client's entities.
    #[must_use]
    pub fn dump_entities() -> Option<String> {
        crate::warn!(
            "a mod asked this host for the client's entities: {}",
            why_unknown("state")
        );
        None
    }

    /// The client's blocks.
    #[must_use]
    pub fn dump_blocks() -> Option<String> {
        crate::warn!(
            "a mod asked this host for the client's blocks: {}",
            why_unknown("state")
        );
        None
    }

    /// The client's network state.
    #[must_use]
    pub fn dump_network() -> Option<String> {
        crate::warn!(
            "a mod asked this host for the client's network state: {}",
            why_unknown("state")
        );
        None
    }

    /// The client's render state.
    #[must_use]
    pub fn dump_render_state() -> Option<String> {
        crate::warn!(
            "a mod asked this host for the client's render state: {}",
            why_unknown("state")
        );
        None
    }

    /// Draws a line on the client's debug overlay.
    pub fn draw_line(_from: (f32, f32), _to: (f32, f32)) -> bool {
        crate::warn!(
            "a mod asked this host to draw a debug line: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws a box on the client's debug overlay.
    pub fn draw_box(_x: f32, _y: f32, _width: f32, _height: f32) -> bool {
        crate::warn!(
            "a mod asked this host to draw a debug box: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws text on the client's debug overlay.
    pub fn draw_text(_x: f32, _y: f32, _text: &str) -> bool {
        crate::warn!(
            "a mod asked this host to draw debug text: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws a path on the client's debug overlay.
    pub fn draw_path(_points: &[(f32, f32)]) -> bool {
        crate::warn!(
            "a mod asked this host to draw a debug path: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws collision shapes on the client's debug overlay.
    pub fn draw_collision(_id: &str) -> bool {
        crate::warn!(
            "a mod asked this host to draw debug collision: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws a bounding box on the client's debug overlay.
    pub fn draw_bounding_box(_id: &str) -> bool {
        crate::warn!(
            "a mod asked this host to draw a bounding box: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws a chunk's bounds on the client's debug overlay.
    pub fn draw_chunk_bounds(_chunk: (i32, i32)) -> bool {
        crate::warn!(
            "a mod asked this host to draw chunk bounds: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws a chunk's sections on the client's debug overlay.
    pub fn draw_chunk_sections(_chunk: (i32, i32)) -> bool {
        crate::warn!(
            "a mod asked this host to draw chunk sections: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws a navigation path on the client's debug overlay.
    pub fn draw_navigation(_path: &[(f32, f32)]) -> bool {
        crate::warn!(
            "a mod asked this host to draw a navigation path: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Draws a ray on the client's debug overlay.
    pub fn draw_ray(_origin: (f32, f32, f32), _target: (f32, f32, f32)) -> bool {
        crate::warn!(
            "a mod asked this host to draw a debug ray: {}",
            why_unknown("overlay")
        );
        false
    }

    /// Flips the client's hitbox flag.
    pub fn show_hitboxes(_on: bool) -> bool {
        crate::warn!(
            "a mod asked this host to toggle the client's hitboxes: {}",
            why_unknown("flags")
        );
        false
    }

    /// Flips the client's entity-id flag.
    pub fn show_entity_ids(_on: bool) -> bool {
        crate::warn!(
            "a mod asked this host to toggle the client's entity ids: {}",
            why_unknown("flags")
        );
        false
    }

    /// Flips the client's block-id flag.
    pub fn show_block_ids(_on: bool) -> bool {
        crate::warn!(
            "a mod asked this host to toggle the client's block ids: {}",
            why_unknown("flags")
        );
        false
    }

    /// Flips the client's memory flag.
    pub fn show_memory(_on: bool) -> bool {
        crate::warn!(
            "a mod asked this host to toggle the client's memory display: {}",
            why_unknown("flags")
        );
        false
    }

    /// Flips the client's fps flag.
    pub fn show_fps(_on: bool) -> bool {
        crate::warn!(
            "a mod asked this host to toggle the client's fps display: {}",
            why_unknown("flags")
        );
        false
    }

    /// Flips the client's tps flag.
    pub fn show_tps(_on: bool) -> bool {
        crate::warn!(
            "a mod asked this host to toggle the client's tps display: {}",
            why_unknown("flags")
        );
        false
    }

    /// Opens the client's developer console.
    pub fn open_console() -> bool {
        crate::warn!(
            "a mod asked this host to open the client's console: {}",
            why_unknown("console")
        );
        false
    }

    /// Opens the client's profiler.
    pub fn open_profiler() -> bool {
        crate::warn!(
            "a mod asked this host to open the client's profiler: {}",
            why_unknown("console")
        );
        false
    }

    /// Opens the client's inspector.
    pub fn open_inspector() -> bool {
        crate::warn!(
            "a mod asked this host to open the client's inspector: {}",
            why_unknown("console")
        );
        false
    }
}

pub use known_only_to_the_client::{
    draw_bounding_box, draw_box, draw_chunk_bounds, draw_chunk_sections, draw_collision, draw_line,
    draw_navigation, draw_path, draw_ray, draw_text, dump_blocks, dump_entities, dump_network,
    dump_render_state, open_console, open_inspector, open_profiler, show_block_ids,
    show_entity_ids, show_fps, show_hitboxes, show_memory, show_tps,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|s| *s = State::default());
    }

    #[test]
    fn a_flag_reports_a_change_and_then_no_change() {
        let _serial = serial();
        fresh();
        assert!(!is_enabled());
        assert!(enable());
        assert!(!enable(), "already on");
        assert!(is_enabled());
        assert!(define_flag("trace_calls", "perf", false));
        assert!(!define_flag("trace_calls", "perf", true), "a name is defined once");
        assert_eq!(get_flag("trace_calls"), Some(false));
        assert!(set_flag("trace_calls", true));
        assert!(!set_flag("trace_calls", true), "the same value reports no change");
        assert_eq!(changed_flags(), vec!["trace_calls".to_owned()]);
        assert_eq!(flags().len(), 1);
        assert!(!set_flag("missing", true), "an unknown flag cannot change");
        assert_eq!(reset_flags(), 1);
        assert!(changed_flags().is_empty());
        assert!(disable());
    }

    #[test]
    fn readouts_render_in_their_order_and_flush_to_the_overlay() {
        let _serial = serial();
        fresh();
        assert!(set_readout("z", "Zed", "1", 10));
        assert!(set_readout_unit("a", "Alpha", "2", "ms", -5));
        assert!(set_readout("m", "Mid", "3", 0));
        assert_eq!(
            render_readouts(),
            vec![
                "Alpha: 2 ms".to_owned(),
                "Mid: 3".to_owned(),
                "Zed: 1".to_owned()
            ]
        );
        assert!(!set_readout("m", "Mid", "4", 0), "replacing is not new");
        assert_eq!(render_readouts()[1], "Mid: 4");
        assert_eq!(flush(), 3);
        assert!(remove_readout("z"));
        assert!(!remove_readout("z"));
    }

    #[test]
    fn a_watch_keeps_a_window_and_its_statistics() {
        let _serial = serial();
        fresh();
        assert_eq!(watch("tick_ms", 16.0).count, 1);
        assert_eq!(watch("tick_ms", 20.0).count, 2);
        let stats = watch("tick_ms", 12.0);
        assert_eq!(stats.last, 12.0);
        assert_eq!(stats.min, 12.0);
        assert_eq!(stats.max, 20.0);
        assert_eq!(stats.average, 16.0);
        assert!(stats.changed, "12 follows 20");
        assert!(!watch("tick_ms", 12.0).changed, "the same value did not change");
        assert_eq!(watch_history("tick_ms"), vec![16.0, 20.0, 12.0, 12.0]);
        assert!(!watch_changed("tick_ms"));
        assert_eq!(watch_stats("missing"), None);
        for value in 0..WATCH_HISTORY as i32 + 5 {
            let _ = watch("ring", f64::from(value));
        }
        assert_eq!(watch_history("ring").len(), WATCH_HISTORY);
        assert_eq!(watch_history("ring")[0], 5.0);
        assert!(clear_watch("tick_ms"));
        assert!(!clear_watch("tick_ms"));
    }

    #[test]
    fn the_console_dispatches_known_commands_and_refuses_unknown_by_name() {
        let _serial = serial();
        fresh();
        assert!(register_command("sum", |args: &[String]| {
            args.iter()
                .filter_map(|arg| arg.parse::<i64>().ok())
                .sum::<i64>()
                .to_string()
        }));
        assert!(
            !register_command("sum", |_: &[String]| String::new()),
            "a name is taken once"
        );
        assert_eq!(queue_command("sum 2 3 4"), 1);
        assert_eq!(pending_commands(), vec!["sum 2 3 4".to_owned()]);
        assert_eq!(run_commands(), 1);
        assert_eq!(dispatch("nope"), "unknown command: nope");
        assert_eq!(
            console_output(),
            vec!["9".to_owned(), "unknown command: nope".to_owned()]
        );
        assert!(commands().contains(&"sum".to_owned()));
        assert!(unregister_command("sum"));
        assert!(!unregister_command("sum"));
        assert_eq!(clear_console(), 2);
        assert!(console_output().is_empty());
    }

    #[test]
    fn a_snapshot_round_trips_and_a_diff_names_the_changed_key() {
        let _serial = serial();
        fresh();
        assert!(define_flag("verbose", "log", false));
        assert!(set_flag("verbose", true));
        let before = snapshot();
        assert!(load_snapshot(&before).is_ok());
        assert!(set_flag("verbose", false));
        let after = snapshot();
        assert_eq!(
            diff_snapshots(&before, &after),
            vec!["flags.verbose.value: true -> false".to_owned()]
        );
        assert!(diff_snapshots(&before, &before).is_empty());

        let path = std::env::temp_dir().join("cinnaloader-debug-snapshot-probe.json");
        let written = export_snapshot(Some(&path)).expect("the snapshot is writable");
        let text = std::fs::read_to_string(&written).expect("and readable");
        assert!(load_snapshot(&text).is_ok());
        let _ = std::fs::remove_file(written);
    }

    #[test]
    fn an_invariant_failure_is_recorded_and_check_all_counts_it() {
        let _serial = serial();
        fresh();
        assert!(register_invariant("size_positive", || Ok(())));
        assert!(register_invariant("has_id", || Err("the id is empty".to_owned())));
        assert!(
            !register_invariant("has_id", || Ok(())),
            "a name is registered once"
        );
        assert_eq!(run_invariant("size_positive").map(|result| result.passed), Some(true));
        assert_eq!(run_invariant("missing"), None);
        assert_eq!(check_all(), 1);
        assert_eq!(
            failures(),
            vec![("has_id".to_owned(), "the id is empty".to_owned())]
        );
        assert_eq!(check_all_results().len(), 3);
        assert_eq!(invariants().len(), 2);
        assert!(unregister_invariant("has_id"));
        assert_eq!(clear_failures(), 1);
        assert!(failures().is_empty());
    }

    #[test]
    fn the_client_own_tools_are_refused_with_a_sentence() {
        let _serial = serial();
        fresh();
        assert_eq!(refusals().len(), 4);
        assert!(!draw_line((0.0, 0.0), (1.0, 1.0)));
        assert!(!draw_text(0.0, 0.0, "hi"));
        assert!(!draw_ray((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)));
        assert!(!show_hitboxes(true));
        assert!(!show_fps(true));
        assert!(!open_console());
        assert_eq!(dump_entities(), None);
        assert_eq!(dump_render_state(), None);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has a reason");
            assert_eq!(why_unknown(&what), why);
        }
        assert!(why_unknown("something_else").contains("something_else"));
    }

    #[test]
    fn assertions_breakpoints_traces_and_inspectors_are_recorded() {
        let _serial = serial();
        fresh();
        assert!(assert(true, "fine"));
        assert!(!assert(false, "the probe broke"));
        assert!(assert_thread(&crate::tasks::current_thread()));
        assert!(!assert_thread("no such thread"));
        assert_eq!(failures().len(), 2);
        assert_eq!(breakpoint(), 1);
        assert_eq!(breakpoints().len(), 1);
        {
            let _call = trace_call("probe");
        }
        assert_eq!(traces().len(), 1);
        assert_eq!(traces()[0].name, "probe");
        assert!(register_inspector("player", || "x=1".to_owned()));
        assert!(!register_inspector("player", String::new));
        assert_eq!(inspect("player"), Some("x=1".to_owned()));
        assert_eq!(inspect("missing"), None);
        assert_eq!(inspectors(), vec!["player".to_owned()]);
        assert!(unregister_inspector("player"));
    }

    #[test]
    fn the_dumps_this_host_can_answer_carry_real_state() {
        let _serial = serial();
        fresh();
        assert!(dump_mods().contains("mods"));
        assert!(dump_tasks().contains("scheduled="));
        assert!(dump_dependencies().contains("order:"));
        assert!(dump_events().contains("kinds="));
        assert!(dump_config().contains("keys:"));
        assert!(dump_registry().contains("registries:"));
        assert!(dump_resources().contains("resources"));
        assert_eq!(dump_state(), snapshot());
        assert!(load_snapshot(&dump_state()).is_ok());
    }
}
