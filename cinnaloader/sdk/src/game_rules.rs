//! Game rules: the typed values a mod declares, and the world this host cannot reach.
//!
//! A **rule** is data a mod ships: an id, a kind (a boolean, an integer, a float, a string,
//! or an enum with a choice list), a default, optional numeric bounds, a display name and a
//! description. Registering one is real, [`set_value`] checks the kind and the bounds and
//! answers a real error when either is wrong, a change fires the handlers and the §6 bus with
//! the old and the new value, and [`reset`] puts the rule back to its default. A value renders
//! and parses through [`crate::config::ConfigValue`], so a rule survives a round trip.
//!
//! The client's own rules are the part this host cannot reach: reading one, or changing one,
//! changes the client's world, and needs a hook this host does not have. Those entry points
//! answer empty or `false` with the reason from [`why_unknown`].
//!
//! ```
//! use cinnaloader::config::{ConfigType, ConfigValue};
//! use cinnaloader::game_rules;
//!
//! game_rules::register(
//!     game_rules::GameRule::new("hello:speed", ConfigType::Integer, ConfigValue::Integer(3))
//!         .name("Speed")
//!         .bounded(0.0, 10.0),
//! );
//! assert_eq!(game_rules::get_value("hello:speed"), Some(ConfigValue::Integer(3)));
//! assert!(game_rules::set_value("hello:speed", ConfigValue::Integer(7)).unwrap().is_some());
//! assert_eq!(game_rules::reset("hello:speed").unwrap().unwrap().new, ConfigValue::Integer(3));
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};
use crate::events::{self, Event};

/// The §6 kind fired when a rule's value changes.
pub const EVENT_CHANGED: &str = "game_rule_changed";

/// The kinds the wishlist names, present before any mod registers one.
pub const BUILT_IN_TYPES: [ConfigType; 6] = [
    ConfigType::Boolean,
    ConfigType::Integer,
    ConfigType::Float,
    ConfigType::String,
    ConfigType::Text,
    ConfigType::Enum,
];

/// Why a value was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleError {
    WrongKind {
        rule: String,
        wanted: String,
        found: String,
    },
    OutOfRange {
        rule: String,
        min: f64,
        max: f64,
        got: f64,
    },
    BadChoice {
        rule: String,
        choices: Vec<String>,
        got: String,
    },
    UnknownRule {
        rule: String,
    },
}

impl fmt::Display for RuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongKind { rule, wanted, found } => {
                write!(formatter, "{rule} wants {wanted}, got {found}")
            }
            Self::OutOfRange { rule, min, max, got } => {
                write!(formatter, "{rule} is {got}, outside {min}..{max}")
            }
            Self::BadChoice { rule, choices, got } => {
                write!(formatter, "{rule} is {got}, not one of {}", choices.join(", "))
            }
            Self::UnknownRule { rule } => write!(formatter, "{rule} is not a registered rule"),
        }
    }
}

/// One rule: what it holds, what it may hold, and how it is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct GameRule {
    pub id: String,
    pub owner: String,
    pub kind: ConfigType,
    pub default: ConfigValue,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub choices: Vec<String>,
    pub name: String,
    pub description: String,
}

impl GameRule {
    #[must_use]
    pub fn new(id: &str, kind: ConfigType, default: ConfigValue) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            kind,
            default,
            min: None,
            max: None,
            choices: Vec::new(),
            name: id.to_owned(),
            description: String::new(),
        }
    }

    #[must_use]
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    #[must_use]
    pub fn description(mut self, description: &str) -> Self {
        self.description = description.to_owned();
        self
    }

    #[must_use]
    pub fn bounded(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }

    #[must_use]
    pub fn minimum(mut self, min: f64) -> Self {
        self.min = Some(min);
        self
    }

    #[must_use]
    pub fn maximum(mut self, max: f64) -> Self {
        self.max = Some(max);
        self
    }

    #[must_use]
    pub fn choices(mut self, choices: &[&str]) -> Self {
        self.choices = choices.iter().map(|choice| (*choice).to_owned()).collect();
        self
    }

    /// The choice list, when the rule is an enum.
    #[must_use]
    pub fn choice_list(&self) -> Option<&[String]> {
        (self.kind == ConfigType::Enum).then_some(self.choices.as_slice())
    }

    /// Whether `value` may be stored here, and why not when it may not.
    pub fn validate(&self, value: &ConfigValue) -> Result<(), RuleError> {
        if !self.kind.accepts(value.kind()) {
            return Err(RuleError::WrongKind {
                rule: self.id.clone(),
                wanted: self.kind.as_str().to_owned(),
                found: value.kind().as_str().to_owned(),
            });
        }
        if let Some(choices) = self.choice_list() {
            if let Some(text) = value.as_text() {
                if !choices.iter().any(|choice| choice == text) {
                    return Err(RuleError::BadChoice {
                        rule: self.id.clone(),
                        choices: choices.to_vec(),
                        got: text.to_owned(),
                    });
                }
            }
        }
        if let Some(got) = numeric(value) {
            let min = self.min.unwrap_or(f64::NEG_INFINITY);
            let max = self.max.unwrap_or(f64::INFINITY);
            if got < min || got > max {
                return Err(RuleError::OutOfRange {
                    rule: self.id.clone(),
                    min,
                    max,
                    got,
                });
            }
        }
        Ok(())
    }
}

impl fmt::Display for GameRule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({}) = {}", self.id, self.name, self.default.render())
    }
}

/// A change, with the value it was and the value it became.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleChange {
    pub rule: String,
    pub old: ConfigValue,
    pub new: ConfigValue,
}

type ChangeHandler = Box<dyn FnMut(&RuleChange) + Send>;

#[derive(Default)]
struct Held {
    rules: BTreeMap<String, GameRule>,
    values: BTreeMap<String, ConfigValue>,
    types: BTreeSet<ConfigType>,
    handlers: Vec<(u64, ChangeHandler)>,
}

static NEXT_HANDLER: AtomicU64 = AtomicU64::new(1);

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| {
        let mut held = Held::default();
        held.types.extend(BUILT_IN_TYPES);
        Mutex::new(held)
    })
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn numeric(value: &ConfigValue) -> Option<f64> {
    match value {
        ConfigValue::Integer(number) => Some(*number as f64),
        ConfigValue::Float(number) | ConfigValue::Percentage(number) => Some(*number),
        _ => None,
    }
}

/// Registers a rule, whose value starts at its default. `false` when its kind was never
/// registered; the built-in kinds are there already, and [`register_type`] adds another.
pub fn register(rule: GameRule) -> bool {
    events::register_event_type(EVENT_CHANGED);
    with(|held| {
        if !held.types.contains(&rule.kind) {
            return false;
        }
        held.values
            .entry(rule.id.clone())
            .or_insert_with(|| rule.default.clone());
        held.rules.insert(rule.id.clone(), rule);
        true
    })
}

/// Forgets a rule and its value.
pub fn unregister(id: &str) -> bool {
    with(|held| {
        held.values.remove(id);
        held.rules.remove(id).is_some()
    })
}

/// A rule by id, as it was registered.
#[must_use]
pub fn get(id: &str) -> Option<GameRule> {
    with(|held| held.rules.get(id).cloned())
}

/// Every rule, in id order.
#[must_use]
pub fn get_all() -> Vec<GameRule> {
    with(|held| held.rules.values().cloned().collect())
}

/// What a rule holds now.
#[must_use]
pub fn get_value(id: &str) -> Option<ConfigValue> {
    with(|held| {
        held.values
            .get(id)
            .cloned()
            .or_else(|| held.rules.get(id).map(|rule| rule.default.clone()))
    })
}

/// What a rule was registered with.
#[must_use]
pub fn get_default(id: &str) -> Option<ConfigValue> {
    with(|held| held.rules.get(id).map(|rule| rule.default.clone()))
}

/// Whether `value` may be stored in `id`, without storing it.
pub fn validate(id: &str, value: &ConfigValue) -> Result<(), RuleError> {
    match get(id) {
        Some(rule) => rule.validate(value),
        None => Err(RuleError::UnknownRule { rule: id.to_owned() }),
    }
}

/// Sets a rule's value, answering the change it made or `None` when it already held it.
pub fn set_value(id: &str, value: ConfigValue) -> Result<Option<RuleChange>, RuleError> {
    let change = with(|held| -> Result<Option<RuleChange>, RuleError> {
        let rule = held
            .rules
            .get(id)
            .cloned()
            .ok_or_else(|| RuleError::UnknownRule {
                rule: id.to_owned(),
            })?;
        rule.validate(&value)?;
        let old = held
            .values
            .get(id)
            .cloned()
            .unwrap_or_else(|| rule.default.clone());
        if old == value {
            return Ok(None);
        }
        held.values.insert(id.to_owned(), value.clone());
        Ok(Some(RuleChange {
            rule: id.to_owned(),
            old,
            new: value,
        }))
    })?;
    if let Some(change) = &change {
        notify(change);
    }
    Ok(change)
}

/// Puts a rule back to its default, answering the change it made.
pub fn reset(id: &str) -> Result<Option<RuleChange>, RuleError> {
    match get_default(id) {
        Some(default) => set_value(id, default),
        None => Err(RuleError::UnknownRule { rule: id.to_owned() }),
    }
}

/// The current value as the text [`crate::config::ConfigValue::render`] writes.
#[must_use]
pub fn export_value(id: &str) -> Option<String> {
    get_value(id).map(|value| value.render())
}

/// Reads a value back from its text form, checks it, and stores it.
pub fn import_value(id: &str, text: &str) -> Result<ConfigValue, RuleError> {
    let rule = get(id).ok_or_else(|| RuleError::UnknownRule {
        rule: id.to_owned(),
    })?;
    let value = ConfigValue::parse(text, rule.kind).ok_or_else(|| RuleError::WrongKind {
        rule: id.to_owned(),
        wanted: rule.kind.as_str().to_owned(),
        found: text.to_owned(),
    })?;
    set_value(id, value.clone())?;
    Ok(value)
}

/// Teaches the registry a kind a mod's rules may use. `false` when it was already known.
pub fn register_type(kind: ConfigType) -> bool {
    with(|held| held.types.insert(kind))
}

/// Every kind a rule may currently be registered with.
#[must_use]
pub fn known_types() -> Vec<ConfigType> {
    with(|held| held.types.iter().copied().collect())
}

/// Calls `handler` whenever a value changes. Returns an id for [`unregister_change_handler`].
pub fn register_change_handler(handler: impl FnMut(&RuleChange) + Send + 'static) -> u64 {
    events::register_event_type(EVENT_CHANGED);
    let id = NEXT_HANDLER.fetch_add(1, Ordering::Relaxed);
    with(|held| held.handlers.push((id, Box::new(handler))));
    id
}

/// Stops calling a handler, and answers whether it existed.
pub fn unregister_change_handler(id: u64) -> bool {
    with(|held| {
        let before = held.handlers.len();
        held.handlers.retain(|(held, _)| *held != id);
        held.handlers.len() != before
    })
}

fn notify(change: &RuleChange) {
    with(|held| {
        for (_, handler) in &mut held.handlers {
            handler(change);
        }
    });
    let text = format!("{} -> {}", change.old.render(), change.new.render());
    events::publish(Event::new(EVENT_CHANGED).from(&change.rule).with_text(&text));
}

/// Why a question about the client's own rules has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client" => {
            "the client's own game rules live in its world; reading one, or changing one, \
             changes that world, which needs a hook this host does not have"
                .to_owned()
        }
        "hook" => {
            "the client reads its game rules as its world runs; nothing here sees that call, \
             so a change a mod makes is real for the mod and not for the client"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about game rules.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client", "hook"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refuse(what: &str) {
    crate::warn!("a mod asked this host about the client's game rules: {}", why_unknown(what));
}

/// The client's own game rules. Empty: [`why_unknown`] says why.
#[must_use]
pub fn client_rules() -> Vec<(String, String)> {
    refuse("client");
    Vec::new()
}

/// Changes one of the client's game rules. `false`, always: [`why_unknown`] says why.
pub fn set_client_rule(_id: &str, _value: ConfigValue) -> bool {
    refuse("client");
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            held.rules.clear();
            held.values.clear();
            held.handlers.clear();
            held.types.clear();
            held.types.extend(BUILT_IN_TYPES);
        });
        events::unsubscribe_all(EVENT_CHANGED);
    }

    fn speed() -> GameRule {
        GameRule::new("hello:speed", ConfigType::Integer, ConfigValue::Integer(3))
            .name("Speed")
            .description("how fast the thing moves")
            .bounded(0.0, 10.0)
    }

    #[test]
    fn a_rule_is_registered_read_back_and_removed() {
        let _serial = serial();
        fresh();
        assert!(register(speed()));
        assert_eq!(
            get("hello:speed"),
            Some(speed()),
            "the declaration reads back as registered"
        );
        assert_eq!(get_value("hello:speed"), Some(ConfigValue::Integer(3)));
        assert_eq!(get_default("hello:speed"), Some(ConfigValue::Integer(3)));
        assert_eq!(get_all().len(), 1);
        assert_eq!(get_all()[0].name, "Speed");
        assert!(unregister("hello:speed"));
        assert_eq!(get("hello:speed"), None);
        assert!(!unregister("hello:speed"));
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_refused_with_what_it_wanted() {
        let _serial = serial();
        fresh();
        register(speed());
        assert_eq!(
            set_value("hello:speed", ConfigValue::Text("fast".to_owned())),
            Err(RuleError::WrongKind {
                rule: "hello:speed".to_owned(),
                wanted: "integer".to_owned(),
                found: "text".to_owned(),
            })
        );
        assert_eq!(get_value("hello:speed"), Some(ConfigValue::Integer(3)));
    }

    #[test]
    fn a_value_outside_the_bounds_is_refused_with_the_bounds() {
        let _serial = serial();
        fresh();
        register(speed());
        assert_eq!(
            set_value("hello:speed", ConfigValue::Integer(99)),
            Err(RuleError::OutOfRange {
                rule: "hello:speed".to_owned(),
                min: 0.0,
                max: 10.0,
                got: 99.0,
            })
        );
        assert_eq!(validate("hello:speed", &ConfigValue::Integer(-1)), Err(RuleError::OutOfRange {
            rule: "hello:speed".to_owned(),
            min: 0.0,
            max: 10.0,
            got: -1.0,
        }));
        assert!(validate("hello:speed", &ConfigValue::Integer(10)).is_ok());
    }

    #[test]
    fn an_enum_value_off_the_choice_list_is_refused() {
        let _serial = serial();
        fresh();
        register(
            GameRule::new("hello:mode", ConfigType::Enum, ConfigValue::Enum("easy".to_owned()))
                .choices(&["easy", "hard"]),
        );
        assert_eq!(
            set_value("hello:mode", ConfigValue::Enum("wild".to_owned())),
            Err(RuleError::BadChoice {
                rule: "hello:mode".to_owned(),
                choices: vec!["easy".to_owned(), "hard".to_owned()],
                got: "wild".to_owned(),
            })
        );
        assert!(set_value("hello:mode", ConfigValue::Enum("hard".to_owned()))
            .expect("valid")
            .is_some());
    }

    #[test]
    fn a_change_fires_the_handler_and_the_bus_once() {
        let _serial = serial();
        fresh();
        register(speed());
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&heard);
        let handler = register_change_handler(move |change| {
            into.lock()
                .unwrap()
                .push(format!("{} {}->{}", change.rule, change.old.render(), change.new.render()));
        });
        let bus = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&bus);
        events::subscribe(EVENT_CHANGED, events::Priority::Normal, move |event| {
            seen.lock().unwrap().push(event.text().unwrap_or_default().to_owned());
        });

        let change = set_value("hello:speed", ConfigValue::Integer(4))
            .expect("valid")
            .expect("it changed");
        assert_eq!(change.old, ConfigValue::Integer(3));
        assert_eq!(change.new, ConfigValue::Integer(4));
        assert_eq!(
            set_value("hello:speed", ConfigValue::Integer(4)).expect("valid"),
            None,
            "setting the value it already holds fires nothing"
        );
        assert_eq!(*heard.lock().unwrap(), vec!["hello:speed 3->4".to_owned()]);
        assert_eq!(*bus.lock().unwrap(), vec!["3 -> 4".to_owned()]);
        assert!(events::is_registered_kind(EVENT_CHANGED));
        assert!(unregister_change_handler(handler));
        assert!(!unregister_change_handler(handler));
    }

    #[test]
    fn reset_puts_a_rule_back_to_its_default() {
        let _serial = serial();
        fresh();
        register(speed());
        set_value("hello:speed", ConfigValue::Integer(7)).expect("valid");
        let change = reset("hello:speed").expect("known").expect("it changed");
        assert_eq!(change.old, ConfigValue::Integer(7));
        assert_eq!(change.new, ConfigValue::Integer(3));
        assert_eq!(get_value("hello:speed"), Some(ConfigValue::Integer(3)));
        assert_eq!(reset("hello:speed").expect("known"), None, "already at its default");
        assert_eq!(
            reset("nothing").unwrap_err(),
            RuleError::UnknownRule {
                rule: "nothing".to_owned()
            }
        );
    }

    #[test]
    fn a_value_round_trips_through_the_config_form() {
        let _serial = serial();
        fresh();
        register(speed());
        assert_eq!(export_value("hello:speed"), Some("3".to_owned()));
        assert_eq!(import_value("hello:speed", "7"), Ok(ConfigValue::Integer(7)));
        assert_eq!(get_value("hello:speed"), Some(ConfigValue::Integer(7)));
        assert_eq!(
            import_value("hello:speed", "fast").unwrap_err(),
            RuleError::WrongKind {
                rule: "hello:speed".to_owned(),
                wanted: "integer".to_owned(),
                found: "fast".to_owned(),
            }
        );
        assert_eq!(export_value("nothing"), None);
    }

    #[test]
    fn a_kind_can_be_registered_and_the_client_is_refused() {
        let _serial = serial();
        fresh();
        assert!(
            !register(GameRule::new(
                "hello:share",
                ConfigType::Percentage,
                ConfigValue::Percentage(50.0),
            )),
            "percentage is not a registered kind yet"
        );
        assert!(register_type(ConfigType::Percentage));
        assert!(!register_type(ConfigType::Percentage), "already known");
        assert!(known_types().contains(&ConfigType::Percentage));
        assert!(register(GameRule::new(
            "hello:share",
            ConfigType::Percentage,
            ConfigValue::Percentage(50.0),
        )));
        assert_eq!(
            validate("nothing", &ConfigValue::Integer(1)).unwrap_err(),
            RuleError::UnknownRule {
                rule: "nothing".to_owned()
            }
        );
        assert!(client_rules().is_empty());
        assert!(!set_client_rule("hello:speed", ConfigValue::Integer(1)));
        assert_eq!(refusals().len(), 2);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("client").contains("client's own game rules"));
    }
}
