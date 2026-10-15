//! Gameplay hooks: the hooks a mod declares, drives itself, and the profile that sets a
//! whole rule set at once.
//!
//! A hook is a mod's own declaration: a family (a tick, a spawn, a loot roll, a damage
//! step, and the rest of the wishlist's list), a name, a priority and a phase. Registering
//! one stores it in an ordered list; [`dispatch`] runs every enabled hook of a family in
//! phase then priority order, handing each the same [`Payload`] and counting how many ran.
//! Nothing here is called by the client: the client runs its own hooks as its world ticks,
//! and a hook registered here runs when the mod dispatches it.
//!
//! A [`RuleProfile`] is a named set of values for [`crate::game_rules`] — a "hard" profile,
//! say — and [`RuleProfile::apply`] reports the rules it actually changed.
//!
//! ```
//! use cinnaloader::events::Payload;
//! use cinnaloader::gameplay::{self, Phase};
//!
//! gameplay::register_damage_rule("hello:crit", 10, Phase::Before, |payload| {
//!     if let Payload::Value(damage) = payload {
//!         *damage *= 2.0;
//!     }
//! });
//! let mut payload = Payload::Value(4.0);
//! assert_eq!(gameplay::dispatch("damage", &mut payload), 1);
//! assert_eq!(payload, Payload::Value(8.0));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::config::ConfigValue;
use crate::events::Payload;
use crate::game_rules::{self, RuleChange};

/// The wishlist's hook families, the names [`dispatch`] takes.
pub const FAMILIES: [&str; 18] = [
    "tick",
    "spawn",
    "loot",
    "interaction",
    "damage",
    "drop",
    "experience",
    "movement",
    "item_use",
    "block_break",
    "block_place",
    "entity_spawn",
    "respawn",
    "sleep",
    "weather",
    "time",
    "explosion",
    "fire",
];

/// Which side of the operation a hook runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    Before,
    After,
}

impl Phase {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A declared hook, without the callback, which is what read-back answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    pub id: u64,
    pub owner: String,
    pub family: String,
    pub name: String,
    pub priority: i32,
    pub phase: Phase,
    pub enabled: bool,
}

type HookRun = Box<dyn FnMut(&mut Payload) + Send>;

struct Registered {
    hook: Hook,
    run: HookRun,
}

#[derive(Default)]
struct Held {
    hooks: BTreeMap<u64, Registered>,
    profiles: BTreeMap<String, RuleProfile>,
}

static NEXT_HOOK: AtomicU64 = AtomicU64::new(1);

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn sort_hooks(hooks: &mut [Hook]) {
    hooks.sort_by_key(|hook| {
        (
            hook.family.clone(),
            hook.phase,
            std::cmp::Reverse(hook.priority),
            hook.id,
        )
    });
}

/// Registers a hook of any family. Returns the id for [`unregister_rule`].
pub fn register_rule(
    family: &str,
    name: &str,
    priority: i32,
    phase: Phase,
    run: impl FnMut(&mut Payload) + Send + 'static,
) -> u64 {
    let id = NEXT_HOOK.fetch_add(1, Ordering::Relaxed);
    let hook = Hook {
        id,
        owner: crate::metadata::id(),
        family: family.to_owned(),
        name: name.to_owned(),
        priority,
        phase,
        enabled: true,
    };
    with(|held| held.hooks.insert(id, Registered { hook, run: Box::new(run) }));
    id
}

/// Forgets a hook.
pub fn unregister_rule(id: u64) -> bool {
    with(|held| held.hooks.remove(&id).is_some())
}

/// A hook by id.
#[must_use]
pub fn get_hook(id: u64) -> Option<Hook> {
    with(|held| held.hooks.get(&id).map(|registered| registered.hook.clone()))
}

/// The hooks of a family in dispatch order, or every hook when `family` is empty.
#[must_use]
pub fn get_hooks(family: &str) -> Vec<Hook> {
    let mut hooks: Vec<Hook> = with(|held| {
        held.hooks
            .values()
            .filter(|registered| family.is_empty() || registered.hook.family == family)
            .map(|registered| registered.hook.clone())
            .collect()
    });
    sort_hooks(&mut hooks);
    hooks
}

/// Turns a hook on or off without forgetting it.
pub fn set_enabled(id: u64, enabled: bool) -> bool {
    with(|held| match held.hooks.get_mut(&id) {
        Some(registered) => {
            registered.hook.enabled = enabled;
            true
        }
        None => false,
    })
}

/// Runs every enabled hook of `family` in phase then priority order, handing each the same
/// payload, and answers how many ran.
pub fn dispatch(family: &str, payload: &mut Payload) -> usize {
    if family.is_empty() {
        return 0;
    }
    let mut chosen: Vec<Registered> = with(|held| {
        let taken: Vec<Registered> = std::mem::take(&mut held.hooks).into_values().collect();
        let mut chosen = Vec::new();
        let mut rest = BTreeMap::new();
        for registered in taken {
            if registered.hook.family == family {
                chosen.push(registered);
            } else {
                rest.insert(registered.hook.id, registered);
            }
        }
        held.hooks = rest;
        chosen
    });
    chosen.sort_by_key(|registered| {
        (
            registered.hook.phase,
            std::cmp::Reverse(registered.hook.priority),
            registered.hook.id,
        )
    });
    let mut ran = 0;
    let mut keep = Vec::with_capacity(chosen.len());
    for mut registered in chosen {
        if registered.hook.enabled {
            (registered.run)(payload);
            ran += 1;
        }
        keep.push(registered);
    }
    with(|held| {
        for registered in keep {
            held.hooks.insert(registered.hook.id, registered);
        }
    });
    ran
}

// The wishlist's families, one registration each.

pub fn register_tick_handler(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("tick", name, priority, phase, run)
}

pub fn register_spawn_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("spawn", name, priority, phase, run)
}

pub fn register_loot_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("loot", name, priority, phase, run)
}

pub fn register_interaction_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("interaction", name, priority, phase, run)
}

pub fn register_damage_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("damage", name, priority, phase, run)
}

pub fn register_drop_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("drop", name, priority, phase, run)
}

pub fn register_experience_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("experience", name, priority, phase, run)
}

pub fn register_movement_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("movement", name, priority, phase, run)
}

pub fn register_item_use_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("item_use", name, priority, phase, run)
}

pub fn register_block_break_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("block_break", name, priority, phase, run)
}

pub fn register_block_place_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("block_place", name, priority, phase, run)
}

pub fn register_entity_spawn_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("entity_spawn", name, priority, phase, run)
}

pub fn register_respawn_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("respawn", name, priority, phase, run)
}

pub fn register_sleep_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("sleep", name, priority, phase, run)
}

pub fn register_weather_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("weather", name, priority, phase, run)
}

pub fn register_time_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("time", name, priority, phase, run)
}

pub fn register_explosion_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("explosion", name, priority, phase, run)
}

pub fn register_fire_rule(name: &str, priority: i32, phase: Phase, run: impl FnMut(&mut Payload) + Send + 'static) -> u64 {
    register_rule("fire", name, priority, phase, run)
}

/// A named set of rule values, applied to the rule set as a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleProfile {
    pub name: String,
    pub values: Vec<(String, ConfigValue)>,
}

impl RuleProfile {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            values: Vec::new(),
        }
    }

    #[must_use]
    pub fn set(mut self, rule: &str, value: ConfigValue) -> Self {
        self.values.retain(|(held, _)| held != rule);
        self.values.push((rule.to_owned(), value));
        self
    }

    #[must_use]
    pub fn get(&self, rule: &str) -> Option<&ConfigValue> {
        self.values
            .iter()
            .find(|(held, _)| held == rule)
            .map(|(_, value)| value)
    }

    /// Applies every value to the rule set, answering the rules it actually changed.
    #[must_use]
    pub fn apply(&self) -> Vec<RuleChange> {
        self.values
            .iter()
            .filter_map(|(rule, value)| {
                game_rules::set_value(rule, value.clone())
                    .ok()
                    .flatten()
            })
            .collect()
    }
}

/// Registers a profile under its name.
pub fn register_profile(profile: RuleProfile) -> bool {
    with(|held| held.profiles.insert(profile.name.clone(), profile).is_none())
}

/// Forgets a profile.
pub fn unregister_profile(name: &str) -> bool {
    with(|held| held.profiles.remove(name).is_some())
}

/// A profile by name.
#[must_use]
pub fn get_profile(name: &str) -> Option<RuleProfile> {
    with(|held| held.profiles.get(name).cloned())
}

/// Every registered profile, in name order.
#[must_use]
pub fn profiles() -> Vec<RuleProfile> {
    with(|held| held.profiles.values().cloned().collect())
}

/// Applies a registered profile, answering the rules it changed.
#[must_use]
pub fn apply_profile(name: &str) -> Vec<RuleChange> {
    get_profile(name).map_or_else(Vec::new, |profile| profile.apply())
}

/// Why the client's gameplay hooks are out of reach here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client" => {
            "the client runs its own gameplay hooks as its world ticks; nothing here sees \
             them, so a hook a mod registers is run only when the mod dispatches it itself"
                .to_owned()
        }
        "world" => {
            "a hook that reads or changes the world needs the client's world and its \
             scheduler, neither of which is reachable from here"
                .to_owned()
        }
        "tick" => {
            "the client's tick calls its own hooks; the loader's tick reaches a mod, and a \
             mod's hook runs when the mod dispatches it"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about gameplay hooks.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client", "world", "tick"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refuse(what: &str) {
    crate::warn!("a mod asked this host about the client's gameplay hooks: {}", why_unknown(what));
}

/// The client's own gameplay hooks. Empty: [`why_unknown`] says why.
#[must_use]
pub fn client_hooks() -> Vec<Hook> {
    refuse("client");
    Vec::new()
}

/// Asks the client to run one of its own hooks. `false`, always: [`why_unknown`] says why.
pub fn run_client_hook(_family: &str) -> bool {
    refuse("client");
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigType;
    use crate::game_rules::GameRule;
    use std::sync::Arc;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|held| {
            held.hooks.clear();
            held.profiles.clear();
        });
        for rule in game_rules::get_all() {
            game_rules::unregister(&rule.id);
        }
    }

    #[test]
    fn hooks_run_in_phase_then_priority_order_with_a_count() {
        let _serial = serial();
        fresh();
        let seen = Arc::new(Mutex::new(Vec::new()));
        for (name, priority, phase) in [
            ("late", 1, Phase::Before),
            ("early", 10, Phase::Before),
            ("after", 5, Phase::After),
        ] {
            let seen = Arc::clone(&seen);
            register_damage_rule(name, priority, phase, move |_| {
                seen.lock().unwrap().push(name);
            });
        }
        let seen_double = Arc::clone(&seen);
        register_damage_rule("double", 20, Phase::Before, move |payload| {
            if let Payload::Value(value) = payload {
                *value *= 2.0;
            }
            seen_double.lock().unwrap().push("double");
        });

        let mut payload = Payload::Value(4.0);
        assert_eq!(dispatch("damage", &mut payload), 4);
        assert_eq!(payload, Payload::Value(8.0), "the hooks chained on the same payload");
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["double", "early", "late", "after"],
            "phase first, then priority"
        );
    }

    #[test]
    fn a_family_keeps_its_own_hooks_and_an_empty_family_runs_none() {
        let _serial = serial();
        fresh();
        register_damage_rule("a", 0, Phase::Before, |_| {});
        register_loot_rule("b", 0, Phase::Before, |_| {});
        assert_eq!(get_hooks("damage").len(), 1);
        assert_eq!(get_hooks("loot").len(), 1);
        assert_eq!(get_hooks("").len(), 2);
        let mut payload = Payload::None;
        assert_eq!(dispatch("loot", &mut payload), 1);
        assert_eq!(dispatch("damage", &mut payload), 1);
        assert_eq!(dispatch("", &mut payload), 0);
        assert_eq!(dispatch("nothing", &mut payload), 0);
    }

    #[test]
    fn a_hook_can_be_disabled_read_back_and_removed() {
        let _serial = serial();
        fresh();
        let id = register_tick_handler("tick", 0, Phase::Before, |_| {});
        assert_eq!(get_hook(id).map(|hook| hook.name), Some("tick".to_owned()));
        assert_eq!(get_hook(id).map(|hook| hook.phase), Some(Phase::Before));
        assert!(set_enabled(id, false));
        let mut payload = Payload::None;
        assert_eq!(dispatch("tick", &mut payload), 0, "a disabled hook does not run");
        assert!(set_enabled(id, true));
        assert_eq!(dispatch("tick", &mut payload), 1);
        assert!(unregister_rule(id));
        assert_eq!(get_hook(id), None);
        assert!(!unregister_rule(id));
        assert!(!set_enabled(id, true));
    }

    #[test]
    fn every_wishlist_family_registers() {
        let _serial = serial();
        fresh();
        let ids = [
            register_tick_handler("f", 0, Phase::Before, |_| {}),
            register_spawn_rule("f", 0, Phase::Before, |_| {}),
            register_loot_rule("f", 0, Phase::Before, |_| {}),
            register_interaction_rule("f", 0, Phase::Before, |_| {}),
            register_damage_rule("f", 0, Phase::Before, |_| {}),
            register_drop_rule("f", 0, Phase::Before, |_| {}),
            register_experience_rule("f", 0, Phase::Before, |_| {}),
            register_movement_rule("f", 0, Phase::Before, |_| {}),
            register_item_use_rule("f", 0, Phase::Before, |_| {}),
            register_block_break_rule("f", 0, Phase::Before, |_| {}),
            register_block_place_rule("f", 0, Phase::Before, |_| {}),
            register_entity_spawn_rule("f", 0, Phase::Before, |_| {}),
            register_respawn_rule("f", 0, Phase::Before, |_| {}),
            register_sleep_rule("f", 0, Phase::Before, |_| {}),
            register_weather_rule("f", 0, Phase::Before, |_| {}),
            register_time_rule("f", 0, Phase::Before, |_| {}),
            register_explosion_rule("f", 0, Phase::Before, |_| {}),
            register_fire_rule("f", 0, Phase::Before, |_| {}),
        ];
        assert_eq!(ids.len(), FAMILIES.len());
        for family in FAMILIES {
            assert_eq!(get_hooks(family).len(), 1, "{family} has no hook");
        }
        assert_eq!(get_hooks("").len(), 18);
        let generic = register_rule("damage", "generic", 0, Phase::Before, |_| {});
        assert_eq!(get_hooks("damage").len(), 2);
        assert!(unregister_rule(generic));
    }

    #[test]
    fn a_profile_applies_and_reports_exactly_what_it_changed() {
        let _serial = serial();
        fresh();
        game_rules::register(
            GameRule::new("hello:speed", ConfigType::Integer, ConfigValue::Integer(3))
                .bounded(0.0, 10.0),
        );
        game_rules::register(GameRule::new(
            "hello:keep_inventory",
            ConfigType::Boolean,
            ConfigValue::Boolean(false),
        ));
        assert!(register_profile(
            RuleProfile::new("hard")
                .set("hello:speed", ConfigValue::Integer(9))
                .set("hello:keep_inventory", ConfigValue::Boolean(true))
        ));
        let changed = apply_profile("hard");
        assert_eq!(changed.len(), 2);
        assert_eq!(
            changed.iter().map(|change| change.rule.clone()).collect::<Vec<_>>(),
            vec!["hello:speed".to_owned(), "hello:keep_inventory".to_owned()]
        );
        assert_eq!(game_rules::get_value("hello:speed"), Some(ConfigValue::Integer(9)));
        assert!(apply_profile("hard").is_empty(), "already applied");
        assert!(apply_profile("nothing").is_empty());
        assert_eq!(get_profile("hard").and_then(|profile| profile.get("hello:speed").cloned()), Some(ConfigValue::Integer(9)));
        assert_eq!(profiles().len(), 1);
        assert!(unregister_profile("hard"));
        assert!(!unregister_profile("hard"));
    }

    #[test]
    fn the_client_hooks_are_refused_with_a_reason() {
        let _serial = serial();
        assert!(client_hooks().is_empty());
        assert!(!run_client_hook("tick"));
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("client").contains("client"));
        assert!(why_unknown("world").contains("world"));
        assert_eq!(Phase::Before.as_str(), "before");
        assert!(Phase::Before < Phase::After);
    }
}
