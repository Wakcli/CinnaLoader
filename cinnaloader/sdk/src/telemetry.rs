//! Telemetry: opt-in only, kept in this process, and never sent anywhere.
//!
//! Nothing is recorded until the user grants consent. While consent is [`Consent::Unset`] or
//! [`Consent::Denied`] every `record_*` call is refused and no event is stored. A recorded
//! event is a name and typed [`ConfigValue`] fields. A field named by the redaction policy is
//! removed before the event is stored or exported, and the rest of the event is untouched;
//! redaction matches a top-level field name and does not descend, so a field nested inside a
//! map can only be redacted by naming the field that encloses it.
//!
//! Events sit in a local buffer whose cap drops the oldest. [`aggregate`] counts events by
//! name and fields by name and keeps no value, so it cannot leak a payload. [`preview`] is
//! exactly what [`export_local`] would write, as JSON through [`crate::serialization`], with
//! its byte size, so a user can read it before agreeing.
//!
//! This host does not transmit telemetry. [`flush`] is therefore refused, and the client's
//! own telemetry is the client's; see [`why_unknown`] and [`refusals`].
//!
//! ```
//! use cinnaloader::config::ConfigValue;
//! use cinnaloader::telemetry::{self, Consent};
//!
//! assert_eq!(telemetry::get_consent(), Consent::Unset);
//! assert!(!telemetry::record_event("boot", &[]));
//! assert_eq!(telemetry::grant(), Consent::Unset);
//! assert!(telemetry::record_event("boot", &[("phase", ConfigValue::Text("play".into()))]));
//! assert_eq!(telemetry::get_queue_size(), 1);
//! let preview = telemetry::preview();
//! assert!(preview.bytes > 0);
//! assert!(telemetry::flush().is_err());
//! assert_eq!(telemetry::reset_consent(), Consent::Granted);
//! ```

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::config::ConfigValue;
use crate::serialization::{self, Map, Value};

/// How many events the buffer holds unless [`set_configuration`] says otherwise.
pub const DEFAULT_CAP: usize = 128;

/// Whether the user has agreed to telemetry. Off until granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Consent {
    /// Never asked, or reset. Nothing is recorded.
    #[default]
    Unset,
    /// Explicitly granted. Recording is allowed.
    Granted,
    /// Explicitly denied. Nothing is recorded.
    Denied,
}

impl Consent {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Granted => "granted",
            Self::Denied => "denied",
        }
    }

    /// Whether recording is allowed: only when granted.
    #[must_use]
    pub fn is_granted(self) -> bool {
        self == Self::Granted
    }

    /// The sentence a user reads for this state.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Unset => "telemetry is off: nothing is recorded until consent is granted",
            Self::Granted => "telemetry is on: events stay in this process and are never sent",
            Self::Denied => "telemetry is denied: nothing is recorded",
        }
    }
}

impl fmt::Display for Consent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One recorded event: a name and typed fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub name: String,
    pub fields: BTreeMap<String, ConfigValue>,
    pub at_secs: u64,
}

impl Event {
    /// One field, by name, after redaction has run.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&ConfigValue> {
        self.fields.get(name)
    }
}

/// What the buffer keeps, and which top-level field names are redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configuration {
    pub cap: usize,
    pub redact: Vec<String>,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            cap: DEFAULT_CAP,
            redact: Vec::new(),
        }
    }
}

/// Exactly what [`export_local`] would write, and how many bytes that is.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Preview {
    pub text: String,
    pub bytes: usize,
}

/// Counts by event name and by field name, and no value anywhere, so it cannot carry a
/// payload out of the process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Aggregate {
    pub events: BTreeMap<String, u64>,
    pub fields: BTreeMap<String, u64>,
    pub total: u64,
}

impl Aggregate {
    /// The aggregate as JSON, through [`crate::serialization`].
    #[must_use]
    pub fn render(&self) -> String {
        let mut events = Map::new();
        for (name, count) in &self.events {
            events.insert(name.clone(), Value::Int(*count as i64));
        }
        let mut fields = Map::new();
        for (name, count) in &self.fields {
            fields.insert(name.clone(), Value::Int(*count as i64));
        }
        let mut root = Map::new();
        root.insert("total", Value::Int(self.total as i64));
        root.insert("events", Value::Map(events));
        root.insert("fields", Value::Map(fields));
        serialization::to_json(&Value::Map(root)).unwrap_or_else(|_| "{}".to_owned())
    }
}

#[derive(Default)]
struct Held {
    consent: Consent,
    buffer: VecDeque<Event>,
    config: Configuration,
    providers: Vec<String>,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn uptime_secs() -> u64 {
    static STARTED: OnceLock<std::time::Instant> = OnceLock::new();
    STARTED
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

/// The consent state, whether it was never set, granted or denied.
#[must_use]
pub fn get_consent() -> Consent {
    with(|held| held.consent)
}

/// Sets consent, and answers the state before.
#[must_use]
pub fn set_consent(consent: Consent) -> Consent {
    with(|held| std::mem::replace(&mut held.consent, consent))
}

/// Grants consent, and answers the state before.
#[must_use]
pub fn grant() -> Consent {
    set_consent(Consent::Granted)
}

/// Denies consent, and answers the state before.
#[must_use]
pub fn deny() -> Consent {
    set_consent(Consent::Denied)
}

/// Puts consent back to unset, and answers the state before.
#[must_use]
pub fn reset_consent() -> Consent {
    set_consent(Consent::Unset)
}

/// [`reset_consent`] under the short name.
#[must_use]
pub fn reset() -> Consent {
    reset_consent()
}

/// Whether telemetry is recording: only when consent is granted.
#[must_use]
pub fn is_enabled() -> bool {
    get_consent().is_granted()
}

/// Grants consent, and answers the state before. The same as [`grant`].
#[must_use]
pub fn enable() -> Consent {
    grant()
}

/// Denies consent, and answers the state before. The same as [`deny`].
#[must_use]
pub fn disable() -> Consent {
    deny()
}

/// Records an event, if consent allows. Redaction runs before storage. Answers whether it
/// was stored; while consent is unset or denied it is refused and nothing is stored.
pub fn record_event(name: &str, fields: &[(&str, ConfigValue)]) -> bool {
    with(|held| {
        if !held.consent.is_granted() {
            return false;
        }
        let cap = held.config.cap.max(1);
        let mut event = Event {
            name: name.to_owned(),
            fields: BTreeMap::new(),
            at_secs: uptime_secs(),
        };
        for (key, value) in fields {
            if held.config.redact.iter().any(|name| name == key) {
                continue;
            }
            event.fields.insert((*key).to_owned(), value.clone());
        }
        while held.buffer.len() >= cap {
            held.buffer.pop_front();
        }
        held.buffer.push_back(event);
        true
    })
}

/// Records a metric as an event named `metric.<name>` with one `value` field.
pub fn record_metric(name: &str, value: ConfigValue) -> bool {
    record_event(&format!("metric.{name}"), &[("value", value)])
}

/// Records an error as an event named `error.<name>` with one `message` field.
pub fn record_error(name: &str, message: &str) -> bool {
    record_event(
        &format!("error.{name}"),
        &[("message", ConfigValue::Text(message.to_owned()))],
    )
}

/// Records a duration as an event named `perf.<name>` with one `millis` field.
pub fn record_performance(name: &str, millis: u64) -> bool {
    record_event(
        &format!("perf.{name}"),
        &[("millis", ConfigValue::Integer(millis as i64))],
    )
}

/// How many events the buffer holds.
#[must_use]
pub fn get_queue_size() -> usize {
    with(|held| held.buffer.len())
}

/// The buffered events, oldest first.
#[must_use]
pub fn get_events() -> Vec<Event> {
    with(|held| held.buffer.iter().cloned().collect())
}

/// Empties the buffer, and answers how many events it held.
pub fn clear() -> usize {
    with(|held| {
        let count = held.buffer.len();
        held.buffer.clear();
        count
    })
}

/// Sending the buffer. Refused: this host does not transmit telemetry.
pub fn flush() -> Result<usize, String> {
    let why = why_unknown("send");
    crate::warn!("a mod asked this host to send telemetry: {why}");
    Err(why)
}

/// Everything buffered, as a value tree for the codecs.
fn document() -> Value {
    with(|held| {
        let events = held
            .buffer
            .iter()
            .map(|event| {
                let mut fields = Map::new();
                for (key, value) in &event.fields {
                    fields.insert(key.clone(), to_value(value));
                }
                let mut map = Map::new();
                map.insert("name", Value::Str(event.name.clone()));
                map.insert("at_secs", Value::Int(event.at_secs as i64));
                map.insert("fields", Value::Map(fields));
                Value::Map(map)
            })
            .collect::<Vec<_>>();
        let mut root = Map::new();
        root.insert("events", Value::List(events));
        Value::Map(root)
    })
}

/// Exactly what [`export_local`] would write, as JSON, with its byte size.
#[must_use]
pub fn preview() -> Preview {
    let text =
        serialization::to_json(&document()).unwrap_or_else(|_| "{\"events\":[]}".to_owned());
    let bytes = text.len();
    Preview { text, bytes }
}

/// The buffered events as JSON bytes, kept in this process.
#[must_use]
pub fn export_local() -> Vec<u8> {
    preview().text.into_bytes()
}

/// Counts by event name and by field name, and nothing else.
#[must_use]
pub fn aggregate() -> Aggregate {
    with(|held| {
        let mut aggregate = Aggregate::default();
        for event in &held.buffer {
            *aggregate.events.entry(event.name.clone()).or_insert(0) += 1;
            for name in event.fields.keys() {
                *aggregate.fields.entry(name.clone()).or_insert(0) += 1;
            }
            aggregate.total += 1;
        }
        aggregate
    })
}

/// The buffer cap and the redaction policy.
#[must_use]
pub fn get_configuration() -> Configuration {
    with(|held| held.config.clone())
}

/// Sets the configuration, and answers the one before. Lowering the cap drops the oldest
/// events that no longer fit, and a cap of zero is raised to one.
pub fn set_configuration(config: Configuration) -> Configuration {
    with(|held| {
        let previous = held.config.clone();
        held.config = Configuration {
            cap: config.cap.max(1),
            redact: config.redact,
        };
        while held.buffer.len() > held.config.cap {
            held.buffer.pop_front();
        }
        previous
    })
}

/// Declares a sink an export would go to. This host never sends to one: a provider is only a
/// name the mod can list, and [`flush`] is still refused.
pub fn register_provider(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    with(|held| {
        if held.providers.iter().any(|held| held == name) {
            return false;
        }
        held.providers.push(name.to_owned());
        true
    })
}

/// Removes a provider by name, and answers whether it was there.
pub fn unregister_provider(name: &str) -> bool {
    with(|held| {
        let before = held.providers.len();
        held.providers.retain(|held| held != name);
        held.providers.len() != before
    })
}

/// The declared providers.
#[must_use]
pub fn get_providers() -> Vec<String> {
    with(|held| held.providers.clone())
}

/// Why transmitting telemetry has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "send" => {
            "this host does not transmit telemetry: the endpoint, the wire and the user's \
             consent are the client's, so a mod that has to phone home must carry its own \
             client and its own opt-in"
                .to_owned()
        }
        "upload" => {
            "there is nowhere to upload to: this host has no telemetry server, and the \
             client's own telemetry is the client's to send"
                .to_owned()
        }
        _ => format!("{what} is not something this host transmits"),
    }
}

/// What this host cannot do about telemetry.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["send", "upload"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn float_value(number: f64) -> Value {
    if number.is_finite() {
        Value::Float(number)
    } else {
        Value::Str(number.to_string())
    }
}

fn to_value(value: &ConfigValue) -> Value {
    match value {
        ConfigValue::Boolean(flag) => Value::Bool(*flag),
        ConfigValue::Integer(number) => Value::Int(*number),
        ConfigValue::Float(number) | ConfigValue::Percentage(number) => float_value(*number),
        ConfigValue::Duration(millis) => Value::Int(*millis as i64),
        ConfigValue::Color(colour) => Value::Str(format!("#{colour:06x}")),
        ConfigValue::String(text)
        | ConfigValue::Text(text)
        | ConfigValue::Enum(text)
        | ConfigValue::Keybind(text)
        | ConfigValue::ResourceId(text)
        | ConfigValue::ItemId(text)
        | ConfigValue::BlockId(text)
        | ConfigValue::EntityId(text) => Value::Str(text.clone()),
        ConfigValue::FilePath(path) | ConfigValue::DirectoryPath(path) => {
            Value::Str(path.display().to_string())
        }
        ConfigValue::Position(x, y, z) | ConfigValue::Vector3(x, y, z) => {
            Value::List(vec![float_value(*x), float_value(*y), float_value(*z)])
        }
        ConfigValue::Vector2(x, y) => Value::List(vec![float_value(*x), float_value(*y)]),
        ConfigValue::Range(from, to) => Value::List(vec![Value::Int(*from), Value::Int(*to)]),
        ConfigValue::List(items) | ConfigValue::Set(items) => {
            Value::List(items.iter().map(to_value).collect())
        }
        ConfigValue::Map(fields) | ConfigValue::Object(fields) | ConfigValue::Compound(fields) => {
            let mut map = Map::new();
            for (key, held) in fields {
                map.insert(key.clone(), to_value(held));
            }
            Value::Map(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            held.consent = Consent::Unset;
            held.buffer.clear();
            held.config = Configuration::default();
            held.providers.clear();
        });
    }

    #[test]
    fn nothing_is_recorded_until_consent_is_granted() {
        let _serial = serial();
        fresh();
        assert_eq!(get_consent(), Consent::Unset);
        assert!(!record_event("boot", &[("phase", ConfigValue::Text("load".into()))]));
        assert_eq!(get_queue_size(), 0, "unset consent stored nothing");

        assert_eq!(deny(), Consent::Unset);
        assert_eq!(get_consent(), Consent::Denied);
        assert!(!record_event("boot", &[("phase", ConfigValue::Text("load".into()))]));
        assert_eq!(get_queue_size(), 0, "denied consent stored nothing");

        assert_eq!(grant(), Consent::Denied);
        assert!(is_enabled());
        assert!(record_event("boot", &[("phase", ConfigValue::Text("play".into()))]));
        assert_eq!(get_queue_size(), 1);
    }

    #[test]
    fn a_recorded_event_keeps_its_name_and_typed_fields() {
        let _serial = serial();
        fresh();
        grant();
        assert!(record_event(
            "combat",
            &[
                ("damage", ConfigValue::Integer(7)),
                ("critical", ConfigValue::Boolean(true)),
            ]
        ));
        let events = get_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "combat");
        assert_eq!(events[0].field("damage"), Some(&ConfigValue::Integer(7)));
        assert_eq!(events[0].field("critical"), Some(&ConfigValue::Boolean(true)));
    }

    #[test]
    fn redaction_removes_a_field_and_leaves_the_rest() {
        let _serial = serial();
        fresh();
        grant();
        set_configuration(Configuration {
            cap: DEFAULT_CAP,
            redact: vec!["player".to_owned()],
        });
        assert!(record_event(
            "move",
            &[
                ("player", ConfigValue::Text("Alice".into())),
                ("x", ConfigValue::Integer(3)),
            ]
        ));
        let event = get_events().into_iter().next().expect("one event");
        assert_eq!(event.field("player"), None, "the redacted field is gone");
        assert_eq!(event.field("x"), Some(&ConfigValue::Integer(3)));

        // nested rule: redaction does not descend, so the enclosing field keeps its map
        let mut context = BTreeMap::new();
        context.insert("player".to_owned(), ConfigValue::Text("Bob".into()));
        assert!(record_event("chat", &[("context", ConfigValue::Map(context))]));
        let chat = get_events()
            .into_iter()
            .find(|event| event.name == "chat")
            .expect("one event");
        assert!(chat.field("context").is_some(), "redaction does not descend");
        assert_eq!(get_queue_size(), 2);
    }

    #[test]
    fn the_buffer_drops_the_oldest_at_its_cap() {
        let _serial = serial();
        fresh();
        grant();
        set_configuration(Configuration {
            cap: 3,
            redact: Vec::new(),
        });
        for name in ["one", "two", "three", "four"] {
            assert!(record_event(name, &[]));
        }
        let names: Vec<String> = get_events().into_iter().map(|event| event.name).collect();
        assert_eq!(names, ["two", "three", "four"]);
        assert_eq!(get_queue_size(), 3);
        assert_eq!(clear(), 3);
        assert_eq!(get_queue_size(), 0);
    }

    #[test]
    fn the_aggregate_counts_names_and_never_a_value() {
        let _serial = serial();
        fresh();
        grant();
        assert!(record_event(
            "boot",
            &[("secret", ConfigValue::Text("hunter2".into()))]
        ));
        assert!(record_event(
            "boot",
            &[("secret", ConfigValue::Text("hunter2".into()))]
        ));
        assert!(record_event(
            "tick",
            &[
                ("secret", ConfigValue::Text("hunter2".into())),
                ("ms", ConfigValue::Integer(4)),
            ]
        ));
        let aggregate = aggregate();
        assert_eq!(aggregate.total, 3);
        assert_eq!(aggregate.events["boot"], 2);
        assert_eq!(aggregate.events["tick"], 1);
        assert_eq!(aggregate.fields["secret"], 3);
        assert_eq!(aggregate.fields["ms"], 1);
        let rendered = aggregate.render();
        assert!(rendered.contains("\"secret\":3"), "{rendered}");
        assert!(!rendered.contains("hunter2"), "no value leaks: {rendered}");
    }

    #[test]
    fn the_preview_is_what_would_be_exported_with_a_byte_size() {
        let _serial = serial();
        fresh();
        grant();
        assert!(record_event(
            "boot",
            &[("phase", ConfigValue::Text("play".into()))]
        ));
        let preview = preview();
        let exported = export_local();
        assert_eq!(preview.text.as_bytes(), exported.as_slice());
        assert_eq!(preview.bytes, exported.len());
        assert!(preview.bytes > 0);
        assert!(preview.text.contains("\"name\":\"boot\""), "{}", preview.text);
    }

    #[test]
    fn sending_is_refused_with_a_sentence() {
        let _serial = serial();
        fresh();
        grant();
        assert!(record_event("boot", &[]));
        let refused = flush();
        assert!(refused.is_err());
        assert!(refused.unwrap_err().contains("does not transmit"));
        assert!(why_unknown("send").contains("does not transmit"));
        let listed = refusals();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|(what, _)| what == "send"));
        assert!(!why_unknown("something else").is_empty());
    }

    #[test]
    fn providers_are_declared_and_never_sent_to() {
        let _serial = serial();
        fresh();
        assert!(register_provider("console"));
        assert!(!register_provider("console"), "a duplicate is refused");
        assert!(!register_provider(""));
        assert_eq!(get_providers(), ["console"]);
        assert!(unregister_provider("console"));
        assert!(!unregister_provider("console"));
        assert_eq!(get_providers().len(), 0);
    }

    #[test]
    fn configuration_round_trips_and_a_short_cap_drops_the_oldest() {
        let _serial = serial();
        fresh();
        assert_eq!(get_configuration(), Configuration::default());
        let previous = set_configuration(Configuration {
            cap: 2,
            redact: vec!["secret".to_owned()],
        });
        assert_eq!(previous, Configuration::default());
        assert_eq!(get_configuration().redact, ["secret"]);
        grant();
        for name in ["a", "b", "c"] {
            assert!(record_event(name, &[]));
        }
        assert_eq!(get_queue_size(), 2);
        set_configuration(Configuration {
            cap: 0,
            redact: Vec::new(),
        });
        assert_eq!(get_configuration().cap, 1, "a cap of zero is raised to one");
        assert_eq!(get_queue_size(), 1);
    }
}
