//! Safe mode and recovery startup: what the loader does after a mod crash-loops.
//!
//! A mod that crashes [`DEFAULT_THRESHOLD`] times inside [`DEFAULT_WINDOW_MILLIS`] puts this
//! host into safe mode. Safe mode carries the reason and the mods that caused it, a disable
//! list with a real reason per mod, and an ordered recovery plan; the flag, the reasons and
//! the counts round trip through [`crate::persistence`], so the next start reads back what
//! happened instead of guessing.
//!
//! ```
//! use cinnaloader::{fs, safe_mode};
//!
//! # let root = std::env::temp_dir().join(format!("cinnaloader-doc-safe-{}", std::process::id()));
//! # let _ = std::fs::remove_dir_all(&root);
//! # std::fs::create_dir_all(&root).unwrap();
//! # fs::set_root(root);
//! safe_mode::reset();
//! safe_mode::record_crash("hello:bad", 0);
//! safe_mode::record_crash("hello:bad", 1);
//! assert!(!safe_mode::is_enabled(), "two of three does not trip the loop");
//! safe_mode::record_crash("hello:bad", 2);
//! assert!(safe_mode::is_enabled());
//! assert!(!safe_mode::can_start(), "the cause is still enabled");
//! assert!(safe_mode::disable_mod("hello:bad", "crashed three times"));
//! assert!(safe_mode::can_start(), "disabling the cause lets a start through");
//! assert!(safe_mode::clear_safe_mode("the user asked for a normal start"));
//! assert!(!safe_mode::is_enabled());
//! ```

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::ConfigValue;
use crate::persistence;
use crate::serialization::{self, Map, Value};

/// How many crashes of one mod inside the window put this host into safe mode.
pub const DEFAULT_THRESHOLD: usize = 3;
/// The window the threshold counts over, in milliseconds.
pub const DEFAULT_WINDOW_MILLIS: u64 = 60_000;
/// The persistence store safe mode is written to, under [`crate::persistence`].
pub const STORE: &str = "safe_mode";

const KEY_ENABLED: &str = "enabled";
const KEY_REASON: &str = "reason";
const KEY_ENABLED_AT: &str = "enabled_at";
const KEY_CAUSED_BY: &str = "caused_by";
const KEY_SUSPECTED: &str = "suspected";
const KEY_DISABLED: &str = "disabled";
const KEY_CRASHES: &str = "crashes";
const KEY_CLEARED_REASON: &str = "cleared_reason";
const KEY_CLEARED_AT: &str = "cleared_at";
const KEY_START_MODE: &str = "start_mode";

/// One crash the detector counted, and the windowed count it made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashRecord {
    pub mod_id: String,
    pub at_millis: u64,
    pub count: usize,
}

/// A mod switched off, the reason, and when it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisabledMod {
    pub mod_id: String,
    pub reason: String,
    pub at_millis: u64,
}

/// What the next start should load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartMode {
    #[default]
    Normal,
    WithoutMods,
}

impl StartMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::WithoutMods => "without_mods",
        }
    }
}

/// One step of the recovery plan, in the order it should run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryStep {
    pub order: usize,
    pub action: &'static str,
    pub detail: String,
}

/// Everything the report names, with the real totals.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RecoveryReport {
    pub at_millis: u64,
    pub safe_mode: bool,
    pub reason: Option<String>,
    pub enabled_at: u64,
    pub cleared_reason: Option<String>,
    pub caused_by: Vec<String>,
    pub disabled: Vec<DisabledMod>,
    pub suspected: Vec<String>,
    pub crashes: Vec<(String, usize)>,
    pub previous_crash: Option<CrashRecord>,
    pub start_mode: StartMode,
    pub can_start: bool,
}

impl RecoveryReport {
    #[must_use]
    pub fn total_crashes(&self) -> usize {
        self.crashes.iter().map(|(_, count)| *count).sum()
    }

    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "safe mode {}: {} cause(s), {} disabled mod(s), {} crash(es) recorded, can_start={}",
            if self.safe_mode { "on" } else { "off" },
            self.caused_by.len(),
            self.disabled.len(),
            self.total_crashes(),
            self.can_start,
        )
    }
}

struct State {
    enabled: bool,
    reason: String,
    enabled_at: u64,
    caused_by: Vec<String>,
    cleared_reason: Option<String>,
    cleared_at: u64,
    disabled: Vec<DisabledMod>,
    crashes: Vec<CrashRecord>,
    counts: BTreeMap<String, usize>,
    suspected: Vec<String>,
    previous_crash: Option<CrashRecord>,
    start_mode: StartMode,
    threshold: usize,
    window: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: false,
            reason: String::new(),
            enabled_at: 0,
            caused_by: Vec::new(),
            cleared_reason: None,
            cleared_at: 0,
            disabled: Vec::new(),
            crashes: Vec::new(),
            counts: BTreeMap::new(),
            suspected: Vec::new(),
            previous_crash: None,
            start_mode: StartMode::Normal,
            threshold: DEFAULT_THRESHOLD,
            window: DEFAULT_WINDOW_MILLIS,
        }
    }
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn lock() -> MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|error| error.into_inner())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Empties the detector, the flag and the disable list. The loader calls it on a clean start;
/// a test calls it so one test does not see another's crashes.
pub fn reset() {
    *lock() = State::default();
}

/// Sets how many crashes inside the window put this host into safe mode. Zero stops the
/// detector tripping on its own.
pub fn set_policy(threshold: usize, window_millis: u64) {
    let mut held = lock();
    held.threshold = threshold;
    held.window = window_millis;
}

/// The threshold and the window in place.
#[must_use]
pub fn get_policy() -> (usize, u64) {
    let held = lock();
    (held.threshold, held.window)
}

/// Records one crash of a mod at a wall-clock millisecond, counts how many of that mod's
/// crashes fall inside the window, and puts this host into safe mode when that reaches the
/// threshold. Answers the windowed count.
#[must_use]
pub fn record_crash(mod_id: &str, at_millis: u64) -> usize {
    let mut held = lock();
    let window = held.window;
    held.crashes.retain(|crash| {
        !(crash.mod_id.as_str() == mod_id && at_millis.saturating_sub(crash.at_millis) > window)
    });
    let count = held
        .crashes
        .iter()
        .filter(|crash| crash.mod_id.as_str() == mod_id)
        .count()
        + 1;
    held.crashes.push(CrashRecord {
        mod_id: mod_id.to_owned(),
        at_millis,
        count,
    });
    *held.counts.entry(mod_id.to_owned()).or_insert(0) += 1;
    if !held.suspected.iter().any(|held| held == mod_id) {
        held.suspected.push(mod_id.to_owned());
    }
    held.previous_crash = Some(CrashRecord {
        mod_id: mod_id.to_owned(),
        at_millis,
        count,
    });
    if held.threshold > 0 && count >= held.threshold {
        held.enabled = true;
        held.reason = format!("{mod_id} crashed {count} time(s) within {window} ms");
        held.enabled_at = at_millis;
        if !held.caused_by.iter().any(|held| held == mod_id) {
            held.caused_by.push(mod_id.to_owned());
        }
    }
    count
}

/// How many crashes this mod has recorded in total, over every window.
#[must_use]
pub fn crash_count(mod_id: &str) -> usize {
    lock().counts.get(mod_id).copied().unwrap_or(0)
}

/// Whether safe mode is on.
#[must_use]
pub fn is_enabled() -> bool {
    lock().enabled
}

/// Puts this host into safe mode by hand, when no crash loop tripped it.
pub fn enable() -> bool {
    let mut held = lock();
    if held.enabled {
        return false;
    }
    held.enabled = true;
    held.reason = "safe mode was enabled by hand".to_owned();
    held.enabled_at = now_millis();
    true
}

/// Leaves safe mode without recording why.
pub fn disable() -> bool {
    let mut held = lock();
    if !held.enabled {
        return false;
    }
    held.enabled = false;
    true
}

/// Clears safe mode and records the reason it was cleared, so the next report can say why.
pub fn clear_safe_mode(reason: &str) -> bool {
    let was_enabled = {
        let mut held = lock();
        let was_enabled = held.enabled;
        held.enabled = false;
        held.cleared_reason = Some(reason.to_owned());
        held.cleared_at = now_millis();
        was_enabled
    };
    let _ = save();
    was_enabled
}

/// Why safe mode is on, or `None` while it is off.
#[must_use]
pub fn get_reason() -> Option<String> {
    let held = lock();
    held.enabled.then(|| held.reason.clone())
}

/// The reason safe mode was last cleared, when one was recorded.
#[must_use]
pub fn cleared_reason() -> Option<String> {
    lock().cleared_reason.clone()
}

/// The mods that tripped safe mode, in the order they did.
#[must_use]
pub fn get_suspected_mods() -> Vec<String> {
    lock().suspected.clone()
}

/// The last crash the detector counted, with its windowed count.
#[must_use]
pub fn get_previous_crash() -> Option<CrashRecord> {
    lock().previous_crash.clone()
}

/// Switches a mod off with a reason. A mod already off is not a change.
pub fn disable_mod(mod_id: &str, reason: &str) -> bool {
    let mut held = lock();
    if held.disabled.iter().any(|held| held.mod_id.as_str() == mod_id) {
        return false;
    }
    held.disabled.push(DisabledMod {
        mod_id: mod_id.to_owned(),
        reason: reason.to_owned(),
        at_millis: now_millis(),
    });
    true
}

/// Switches a mod back on, and answers whether it was off.
pub fn enable_mod(mod_id: &str) -> bool {
    let mut held = lock();
    let before = held.disabled.len();
    held.disabled.retain(|held| held.mod_id.as_str() != mod_id);
    held.disabled.len() != before
}

/// Switches every suspected mod off, and answers how many that added.
pub fn disable_all() -> usize {
    let mut held = lock();
    let candidates = held.suspected.clone();
    let mut added = 0;
    for mod_id in candidates {
        if !held.disabled.iter().any(|held| held.mod_id == mod_id) {
            held.disabled.push(DisabledMod {
                mod_id,
                reason: "suspected of a crash loop".to_owned(),
                at_millis: now_millis(),
            });
            added += 1;
        }
    }
    added
}

/// Whether a mod is on the disable list.
#[must_use]
pub fn is_disabled(mod_id: &str) -> bool {
    lock()
        .disabled
        .iter()
        .any(|held| held.mod_id.as_str() == mod_id)
}

/// Why a mod was switched off.
#[must_use]
pub fn disabled_reason(mod_id: &str) -> Option<String> {
    lock()
        .disabled
        .iter()
        .find(|held| held.mod_id.as_str() == mod_id)
        .map(|held| held.reason.clone())
}

/// Every disabled mod and the reason it was switched off.
#[must_use]
pub fn get_disabled_mods() -> Vec<(String, String)> {
    lock()
        .disabled
        .iter()
        .map(|held| (held.mod_id.clone(), held.reason.clone()))
        .collect()
}

/// Every disabled mod with the reason and the time it went.
#[must_use]
pub fn disabled_entries() -> Vec<DisabledMod> {
    lock().disabled.clone()
}

/// Whether a start can proceed: true while safe mode is off, and true in safe mode once every
/// mod that caused it is off. A start is refused while a known cause is still enabled.
#[must_use]
pub fn can_start() -> bool {
    can_start_of(&lock())
}

fn can_start_of(held: &State) -> bool {
    if !held.enabled {
        return true;
    }
    held.caused_by
        .iter()
        .all(|cause| held.disabled.iter().any(|held| held.mod_id == *cause))
}

/// The ordered recovery plan: switch the crashing mods off, keep the rest, and fall back to a
/// start with no mods. Always three steps, in that order.
#[must_use]
pub fn recovery_plan() -> Vec<RecoveryStep> {
    let held = lock();
    let causes = if held.caused_by.is_empty() {
        held.suspected.clone()
    } else {
        held.caused_by.clone()
    };
    let first = if causes.is_empty() {
        "no mod is suspected yet".to_owned()
    } else {
        format!("disable {}", causes.join(", "))
    };
    vec![
        RecoveryStep {
            order: 1,
            action: "disable_crashing_mods",
            detail: first,
        },
        RecoveryStep {
            order: 2,
            action: "keep_other_mods",
            detail: format!(
                "leave every other mod enabled; {} mod(s) are off",
                held.disabled.len()
            ),
        },
        RecoveryStep {
            order: 3,
            action: "start_without_mods",
            detail: "last resort: start with no mods at all".to_owned(),
        },
    ]
}

/// Asks for the next start to load no mods. Answers whether that changed the mode.
pub fn start_without_mod() -> bool {
    let mut held = lock();
    if held.start_mode == StartMode::WithoutMods {
        return false;
    }
    held.start_mode = StartMode::WithoutMods;
    true
}

/// Asks for the next start to load mods as usual. Answers whether that changed the mode.
pub fn start_with_mods() -> bool {
    let mut held = lock();
    if held.start_mode == StartMode::Normal {
        return false;
    }
    held.start_mode = StartMode::Normal;
    true
}

/// What the next start should load.
#[must_use]
pub fn start_mode() -> StartMode {
    lock().start_mode
}

/// Clears safe mode and asks for a normal start, as one action.
pub fn request_normal_startup() -> bool {
    let mut held = lock();
    let was_enabled = held.enabled;
    held.enabled = false;
    held.cleared_reason = Some("a normal startup was requested".to_owned());
    held.cleared_at = now_millis();
    let was_mode = held.start_mode != StartMode::Normal;
    held.start_mode = StartMode::Normal;
    drop(held);
    let _ = save();
    was_enabled || was_mode
}

/// What is wrong with the mods from safe mode's point of view: a cause still enabled, a mod
/// off with no reason, or safe mode on with no cause recorded.
#[must_use]
pub fn validate_all_mods() -> Vec<String> {
    let held = lock();
    let mut problems = Vec::new();
    for cause in &held.caused_by {
        if !held.disabled.iter().any(|held| held.mod_id == *cause) {
            problems.push(format!("{cause} caused safe mode and is still enabled"));
        }
    }
    for entry in &held.disabled {
        if entry.reason.trim().is_empty() {
            problems.push(format!("{} is disabled with no reason", entry.mod_id));
        }
    }
    if held.enabled && held.caused_by.is_empty() {
        problems.push("safe mode is on with no recorded cause".to_owned());
    }
    problems
}

/// What is wrong with the persisted safe-mode state: the flag in memory against the flag on
/// disk, and whether anything is written at all. Only the store this host owns is checked.
#[must_use]
pub fn validate_all_resources() -> Vec<String> {
    let mut problems = Vec::new();
    if persistence::current_store().as_deref() != Some(STORE) {
        problems.push(format!("the {STORE} store is not open"));
        return problems;
    }
    let held = lock();
    let persisted = persistence::get(KEY_ENABLED)
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if persisted != held.enabled {
        problems.push("the persisted flag differs from the one in memory".to_owned());
    }
    if held.enabled && persistence::keys().is_empty() {
        problems.push("safe mode is on but nothing is persisted".to_owned());
    }
    problems
}

/// Resets the detector, the flag and the disable list, clears the store and answers whether
/// anything changed.
pub fn reset_settings() -> bool {
    let changed = {
        let mut held = lock();
        let changed = held.enabled
            || !held.disabled.is_empty()
            || !held.crashes.is_empty()
            || !held.suspected.is_empty()
            || !held.counts.is_empty();
        *held = State::default();
        changed
    };
    if ensure_store() {
        let _ = persistence::clear();
    }
    changed
}

/// Reads the store's backup back over it, and loads it, so a start can undo a bad change.
pub fn restore_previous_config() -> bool {
    if !ensure_store() {
        return false;
    }
    match persistence::restore() {
        Ok(_) => reload(),
        Err(error) => {
            crate::warn!("safe mode could not restore the previous config: {error}");
            false
        }
    }
}

fn ensure_store() -> bool {
    if persistence::current_store().as_deref() == Some(STORE) {
        return true;
    }
    match persistence::open(STORE) {
        Ok(_) => true,
        Err(error) => {
            crate::warn!("safe mode could not open its store: {error}");
            false
        }
    }
}

/// Writes the flag, the reason, the causes, the disable list and the crash counts to the
/// store. Answers whether it went.
pub fn save() -> bool {
    if !ensure_store() {
        return false;
    }
    let held = lock();
    let _ = persistence::set(KEY_ENABLED, ConfigValue::Boolean(held.enabled));
    let _ = persistence::set(KEY_REASON, ConfigValue::Text(held.reason.clone()));
    let _ = persistence::set(KEY_ENABLED_AT, ConfigValue::Integer(held.enabled_at as i64));
    let _ = persistence::set(KEY_CAUSED_BY, text_list_value(&held.caused_by));
    let _ = persistence::set(KEY_SUSPECTED, text_list_value(&held.suspected));
    let _ = persistence::set(KEY_DISABLED, disabled_value(&held.disabled));
    let _ = persistence::set(KEY_CRASHES, counts_value(&held.counts));
    let _ = persistence::set(
        KEY_CLEARED_REASON,
        held.cleared_reason
            .clone()
            .map_or(ConfigValue::Text(String::new()), ConfigValue::Text),
    );
    let _ = persistence::set(KEY_CLEARED_AT, ConfigValue::Integer(held.cleared_at as i64));
    let _ = persistence::set(
        KEY_START_MODE,
        ConfigValue::Enum(held.start_mode.as_str().to_owned()),
    );
    drop(held);
    persistence::save()
}

/// Reads the store back over what is in memory. Answers whether there was a store to read.
pub fn reload() -> bool {
    if !ensure_store() {
        return false;
    }
    if !persistence::load() {
        return false;
    }
    let enabled = persistence::get(KEY_ENABLED)
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let reason = persistence::get(KEY_REASON)
        .and_then(|value| value.as_text().map(str::to_owned))
        .unwrap_or_default();
    let enabled_at = persistence::get(KEY_ENABLED_AT)
        .and_then(|value| value.as_integer())
        .and_then(|number| u64::try_from(number).ok())
        .unwrap_or(0);
    let caused_by = text_list(persistence::get(KEY_CAUSED_BY));
    let suspected = text_list(persistence::get(KEY_SUSPECTED));
    let disabled = disabled_from(persistence::get(KEY_DISABLED));
    let counts = counts_from(persistence::get(KEY_CRASHES));
    let cleared_reason = persistence::get(KEY_CLEARED_REASON)
        .and_then(|value| value.as_text().map(str::to_owned))
        .filter(|text| !text.is_empty());
    let cleared_at = persistence::get(KEY_CLEARED_AT)
        .and_then(|value| value.as_integer())
        .and_then(|number| u64::try_from(number).ok())
        .unwrap_or(0);
    let start_mode = match persistence::get(KEY_START_MODE)
        .and_then(|value| value.as_text().map(str::to_owned))
        .as_deref()
    {
        Some("without_mods") => StartMode::WithoutMods,
        _ => StartMode::Normal,
    };
    let mut held = lock();
    held.enabled = enabled;
    held.reason = reason;
    held.enabled_at = enabled_at;
    held.caused_by = caused_by;
    held.suspected = suspected;
    held.disabled = disabled;
    held.counts = counts;
    held.cleared_reason = cleared_reason;
    held.cleared_at = cleared_at;
    held.start_mode = start_mode;
    true
}

fn text_list_value(items: &[String]) -> ConfigValue {
    ConfigValue::List(items.iter().cloned().map(ConfigValue::Text).collect())
}

fn text_list(value: Option<ConfigValue>) -> Vec<String> {
    value
        .and_then(|value| {
            value
                .as_list()
                .map(|items| items.iter().filter_map(ConfigValue::as_text).map(str::to_owned).collect())
        })
        .unwrap_or_default()
}

fn disabled_value(entries: &[DisabledMod]) -> ConfigValue {
    ConfigValue::List(entries.iter().map(disabled_entry_value).collect())
}

fn disabled_entry_value(entry: &DisabledMod) -> ConfigValue {
    let mut fields = BTreeMap::new();
    fields.insert("mod".to_owned(), ConfigValue::Text(entry.mod_id.clone()));
    fields.insert("reason".to_owned(), ConfigValue::Text(entry.reason.clone()));
    fields.insert("at".to_owned(), ConfigValue::Integer(entry.at_millis as i64));
    ConfigValue::Compound(fields)
}

fn disabled_from(value: Option<ConfigValue>) -> Vec<DisabledMod> {
    value
        .and_then(|value| value.as_list().map(|items| items.to_vec()))
        .unwrap_or_default()
        .iter()
        .filter_map(disabled_entry_from)
        .collect()
}

fn disabled_entry_from(value: &ConfigValue) -> Option<DisabledMod> {
    let fields = value.as_map()?;
    Some(DisabledMod {
        mod_id: fields.get("mod")?.as_text()?.to_owned(),
        reason: fields.get("reason")?.as_text()?.to_owned(),
        at_millis: fields
            .get("at")
            .and_then(ConfigValue::as_integer)
            .and_then(|number| u64::try_from(number).ok())
            .unwrap_or(0),
    })
}

fn counts_value(counts: &BTreeMap<String, usize>) -> ConfigValue {
    let mut fields = BTreeMap::new();
    for (mod_id, count) in counts {
        fields.insert(mod_id.clone(), ConfigValue::Integer(*count as i64));
    }
    ConfigValue::Compound(fields)
}

fn counts_from(value: Option<ConfigValue>) -> BTreeMap<String, usize> {
    value
        .and_then(|value| {
            value.as_map().map(|fields| {
                fields
                    .iter()
                    .filter_map(|(mod_id, count)| {
                        count
                            .as_integer()
                            .and_then(|number| usize::try_from(number).ok())
                            .map(|count| (mod_id.clone(), count))
                    })
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// The state as one value, for [`create_recovery_report`].
#[must_use]
pub fn recovery_report() -> RecoveryReport {
    let held = lock();
    RecoveryReport {
        at_millis: now_millis(),
        safe_mode: held.enabled,
        reason: held.enabled.then(|| held.reason.clone()),
        enabled_at: held.enabled_at,
        cleared_reason: held.cleared_reason.clone(),
        caused_by: held.caused_by.clone(),
        disabled: held.disabled.clone(),
        suspected: held.suspected.clone(),
        crashes: held
            .counts
            .iter()
            .map(|(mod_id, count)| (mod_id.clone(), *count))
            .collect(),
        previous_crash: held.previous_crash.clone(),
        start_mode: held.start_mode,
        can_start: can_start_of(&held),
    }
}

fn report_value(report: &RecoveryReport) -> Value {
    let mut map = Map::new();
    map.insert("summary", Value::str(report.summary()));
    map.insert("safe_mode", Value::Bool(report.safe_mode));
    map.insert(
        "reason",
        report.reason.as_deref().map_or(Value::Null, Value::str),
    );
    map.insert("enabled_at", Value::Int(report.enabled_at as i64));
    map.insert(
        "cleared_reason",
        report
            .cleared_reason
            .as_deref()
            .map_or(Value::Null, Value::str),
    );
    map.insert(
        "caused_by",
        Value::List(report.caused_by.iter().map(Value::str).collect()),
    );
    map.insert(
        "suspected",
        Value::List(report.suspected.iter().map(Value::str).collect()),
    );
    map.insert(
        "disabled",
        Value::List(
            report
                .disabled
                .iter()
                .map(|entry| {
                    let mut fields = Map::new();
                    fields.insert("mod", Value::str(entry.mod_id.as_str()));
                    fields.insert("reason", Value::str(entry.reason.as_str()));
                    fields.insert("at", Value::Int(entry.at_millis as i64));
                    Value::Map(fields)
                })
                .collect(),
        ),
    );
    map.insert(
        "crashes",
        Value::List(
            report
                .crashes
                .iter()
                .map(|(mod_id, count)| {
                    let mut fields = Map::new();
                    fields.insert("mod", Value::str(mod_id.as_str()));
                    fields.insert("count", Value::Int(*count as i64));
                    Value::Map(fields)
                })
                .collect(),
        ),
    );
    map.insert(
        "previous_crash",
        match &report.previous_crash {
            Some(crash) => {
                let mut fields = Map::new();
                fields.insert("mod", Value::str(crash.mod_id.as_str()));
                fields.insert("at", Value::Int(crash.at_millis as i64));
                fields.insert("count", Value::Int(crash.count as i64));
                Value::Map(fields)
            }
            None => Value::Null,
        },
    );
    map.insert("start_mode", Value::str(report.start_mode.as_str()));
    map.insert("can_start", Value::Bool(report.can_start));
    Value::Map(map)
}

/// The report as text or JSON, through [`crate::serialization`]. Any name but `text` is JSON.
#[must_use]
pub fn create_recovery_report(format: &str) -> String {
    let value = report_value(&recovery_report());
    match format {
        "text" => serialization::to_text(&value),
        _ => serialization::to_json(&value).unwrap_or_else(|_| serialization::to_text(&value)),
    }
}

/// Why a question about the client's own safe mode or recovery has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client_safe_mode" => {
            "the client's own safe mode belongs to the client and never crosses the mod \
             boundary, so this host can neither read nor set it"
                .to_owned()
        }
        "client_recovery" => {
            "the client's recovery screen and the automatic recovery it runs belong to the \
             client, and this host cannot request one"
                .to_owned()
        }
        other => format!("{other} is not something this host can reach about recovery"),
    }
}

/// The two things this host refuses because the client owns them.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client_safe_mode", "client_recovery"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own safe mode and recovery. Every entry warns and answers nothing.
pub mod known_only_to_the_client {
    use super::why_unknown;

    /// Whether the client is in its own safe mode.
    #[must_use]
    pub fn is_in_client_safe_mode() -> bool {
        crate::warn!(
            "a mod asked this host about the client's safe mode: {}",
            why_unknown("client_safe_mode")
        );
        false
    }

    /// Asks the client to run its recovery.
    pub fn request_client_recovery() -> bool {
        crate::warn!(
            "a mod asked this host to run the client's recovery: {}",
            why_unknown("client_recovery")
        );
        false
    }

    /// The client's recovery screen, which this host cannot see.
    #[must_use]
    pub fn client_recovery_screen() -> Option<String> {
        crate::warn!(
            "a mod asked this host for the client's recovery screen: {}",
            why_unknown("client_recovery")
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs;
    use std::path::PathBuf;

    fn rooted(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cinnaloader-safe-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        fs::set_root(root.clone());
        persistence::close();
        reset();
        root
    }

    #[test]
    fn the_detector_trips_at_the_threshold_and_not_before() {
        let _serial = crate::test_lock::serial();
        reset();
        set_policy(3, 60_000);
        assert_eq!(record_crash("hello:bad", 0), 1);
        assert_eq!(record_crash("hello:bad", 1), 2);
        assert!(!is_enabled(), "two of three does not trip the loop");
        assert_eq!(record_crash("hello:bad", 2), 3);
        assert!(is_enabled(), "the third does");
        assert_eq!(
            get_reason().as_deref(),
            Some("hello:bad crashed 3 time(s) within 60000 ms")
        );
        assert_eq!(get_suspected_mods(), vec!["hello:bad".to_owned()]);
        assert_eq!(crash_count("hello:bad"), 3);
        assert_eq!(
            get_previous_crash(),
            Some(CrashRecord {
                mod_id: "hello:bad".to_owned(),
                at_millis: 2,
                count: 3,
            })
        );
    }

    #[test]
    fn a_crash_outside_the_window_ages_out_and_does_not_count() {
        let _serial = crate::test_lock::serial();
        reset();
        set_policy(3, 1_000);
        let _ = record_crash("hello:aged", 0);
        let _ = record_crash("hello:aged", 0);
        assert_eq!(
            record_crash("hello:aged", 1_500),
            1,
            "both old crashes aged out of the 1000 ms window"
        );
        assert!(!is_enabled());

        reset();
        set_policy(3, 1_000);
        let _ = record_crash("hello:edge", 0);
        let _ = record_crash("hello:edge", 0);
        assert_eq!(
            record_crash("hello:edge", 1_000),
            3,
            "a crash exactly a window old still counts"
        );
        assert!(is_enabled());
    }

    #[test]
    fn safe_mode_round_trips_through_persistence() {
        let _serial = crate::test_lock::serial();
        let root = rooted("round-trip");
        set_policy(3, 60_000);
        let _ = record_crash("hello:bad", 0);
        let _ = record_crash("hello:bad", 1);
        let _ = record_crash("hello:bad", 2);
        assert!(disable_mod("hello:bad", "crashed three times"));
        assert!(save(), "the flag is written");

        disable();
        assert!(!is_enabled(), "the in-memory flag is off");

        assert!(reload(), "the store reads back");
        assert!(is_enabled(), "the persisted flag came back");
        assert_eq!(
            get_reason().as_deref(),
            Some("hello:bad crashed 3 time(s) within 60000 ms")
        );
        assert_eq!(get_disabled_mods(), vec![("hello:bad".to_owned(), "crashed three times".to_owned())]);
        assert_eq!(recovery_report().crashes, vec![("hello:bad".to_owned(), 3)]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_disable_list_adds_removes_and_reports_no_change() {
        let _serial = crate::test_lock::serial();
        reset();
        assert!(disable_mod("hello:a", "loop"));
        assert!(!disable_mod("hello:a", "again"), "disabling twice is no change");
        assert!(disable_mod("hello:b", "loop too"));
        assert!(is_disabled("hello:a"));
        assert_eq!(disabled_reason("hello:a").as_deref(), Some("loop"));
        assert_eq!(get_disabled_mods().len(), 2);
        assert_eq!(disabled_entries().len(), 2);
        assert!(enable_mod("hello:a"));
        assert!(!enable_mod("hello:a"), "removing what is gone is no change");
        assert!(!is_disabled("hello:a"));
        assert_eq!(get_disabled_mods().len(), 1);
    }

    #[test]
    fn the_recovery_plan_is_ordered_and_can_start_follows_the_cause() {
        let _serial = crate::test_lock::serial();
        reset();
        set_policy(3, 60_000);
        let _ = record_crash("hello:bad", 0);
        let _ = record_crash("hello:bad", 1);
        let _ = record_crash("hello:bad", 2);
        assert!(!can_start(), "the cause is still enabled");

        let plan = recovery_plan();
        assert_eq!(plan.len(), 3, "three ordered actions");
        assert_eq!(
            plan.iter().map(|step| step.order).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(plan[0].action, "disable_crashing_mods");
        assert!(plan[0].detail.contains("hello:bad"));
        assert_eq!(plan[2].action, "start_without_mods");

        assert_eq!(disable_all(), 1, "one suspected mod was switched off");
        assert!(can_start(), "disabling every cause lets a start through");
        assert_eq!(disable_all(), 0, "nothing left to switch off");
    }

    #[test]
    fn the_report_names_what_was_disabled_and_the_counts() {
        let _serial = crate::test_lock::serial();
        let root = rooted("report");
        set_policy(3, 60_000);
        let _ = record_crash("hello:bad", 0);
        let _ = record_crash("hello:bad", 1);
        let _ = record_crash("hello:bad", 2);
        assert!(disable_mod("hello:bad", "crashed three times"));
        assert!(save(), "the store is written so the resources validate");

        let report = recovery_report();
        assert!(report.safe_mode);
        assert_eq!(report.total_crashes(), 3);
        assert_eq!(report.summary(), "safe mode on: 1 cause(s), 1 disabled mod(s), 3 crash(es) recorded, can_start=true");
        assert_eq!(
            validate_all_mods(),
            Vec::<String>::new(),
            "the cause is off and everything has a reason"
        );

        let text = create_recovery_report("text");
        assert!(text.contains("hello:bad"), "{text}");
        assert!(text.contains("crashed three times"), "{text}");
        assert!(text.contains("3"), "{text}");
        let json = create_recovery_report("json");
        assert!(json.contains("\"safe_mode\":true"), "{json}");
        assert!(json.contains("\"count\":3"), "{json}");

        assert_eq!(validate_all_resources(), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clearing_records_a_reason_and_the_flag_really_goes() {
        let _serial = crate::test_lock::serial();
        let root = rooted("clear");
        assert!(enable());
        assert!(!enable(), "enabling twice is no change");
        assert!(is_enabled());
        assert!(clear_safe_mode("the user asked for a normal start"));
        assert!(!is_enabled(), "the flag is gone");
        assert_eq!(
            cleared_reason().as_deref(),
            Some("the user asked for a normal start")
        );
        assert!(!clear_safe_mode("already off"), "clearing an off flag is no change");

        assert!(start_without_mod());
        assert!(!start_without_mod());
        assert_eq!(start_mode(), StartMode::WithoutMods);
        assert!(request_normal_startup());
        assert_eq!(start_mode(), StartMode::Normal);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reset_settings_and_restore_previous_config_move_the_state() {
        let _serial = crate::test_lock::serial();
        let root = rooted("recover");
        set_policy(3, 60_000);
        let _ = record_crash("hello:bad", 0);
        let _ = record_crash("hello:bad", 1);
        let _ = record_crash("hello:bad", 2);
        assert!(save());
        assert!(persistence::backup().is_ok(), "a backup to restore from");

        assert!(clear_safe_mode("a bad change"));
        assert!(!is_enabled());
        assert!(restore_previous_config(), "the backup is read back");
        assert!(is_enabled(), "the flag before the change is back");

        assert!(reset_settings());
        assert!(!is_enabled());
        assert!(get_disabled_mods().is_empty());
        assert_eq!(crash_count("hello:bad"), 0);
        assert!(!reset_settings(), "nothing left to reset");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_client_safe_mode_and_recovery_are_refused() {
        let _serial = crate::test_lock::serial();
        let rows = refusals();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|(what, _)| what == "client_safe_mode"));
        assert!(rows.iter().any(|(what, _)| what == "client_recovery"));
        assert!(why_unknown("client_safe_mode").contains("client"));
        assert!(!why_unknown("nonsense").is_empty());
        assert!(!known_only_to_the_client::is_in_client_safe_mode());
        assert!(!known_only_to_the_client::request_client_recovery());
        assert_eq!(known_only_to_the_client::client_recovery_screen(), None);
    }
}
