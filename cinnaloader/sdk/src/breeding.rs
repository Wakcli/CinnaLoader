//! Breeding: the pairs a mod starts, and the traits a child really inherits.
//!
//! A mod declares what a kind of animal passes on ([`Animal`]) and which two kinds make which
//! child ([`BreedingRule`]), then starts a [`BreedingPair`] here. What is *not* just data is
//! the inheritance: [`breed`] builds a child whose every trait is taken from one parent or the
//! other by a draw seeded with the caller's number, so the same seed always answers the same
//! child and a different one really does answer differently. The pair carries the love flag,
//! an age and the restock cooldown, and [`advance`] ticks them all down — the cooldown really
//! reaches zero and the child's growth timer really matures it.
//!
//! Refused: the client's live animals, their love and age, and making two of them breed — those
//! are the client's entities and the server's spawns, which this host does not reach.
//!
//! ```
//! use cinnaloader::breeding;
//!
//! breeding::register_parent(breeding::Animal::new("cow-a", "cow").trait_of("speed", 0.25));
//! breeding::register_parent(breeding::Animal::new("cow-b", "cow").trait_of("speed", 0.30));
//! breeding::register_rule(breeding::BreedingRule::new("cow", "cow", "calf"));
//! let pair = breeding::start("cow-a", "cow-b").expect("two registered cows");
//! let calf = breeding::breed(&pair, 7).expect("a rule and both parents");
//! assert_eq!(calf.kind, "calf");
//! assert!(calf.traits.contains_key("speed"));
//! assert!(!breeding::get_live_animal().is_some(), "a live animal is the client's");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events;
use crate::worldgen::Random;

/// The breeding events this host would have to receive from the client to fire.
pub const BREED_EVENTS: [&str; 6] = [
    "breed_start",
    "breed_cancel",
    "breed_ready",
    "offspring_born",
    "offspring_matured",
    "breeding_rule_added",
];

/// Teaches the event bus each breeding event name, so a subscription is in place before the
/// hooks land.
pub fn register_event_kinds() -> usize {
    BREED_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// The event kinds this host would need from the client to fire a breeding callback.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    BREED_EVENTS.to_vec()
}

/// Whether a breeding event kind is one the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    BREED_EVENTS.contains(&kind) && events::is_registered_kind(kind)
}

/// Why a breeding question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "live" => {
            "the client's animals are entities it keeps to itself; a mod's own parents and \
             pairs live in this registry"
                .to_owned()
        }
        "breed" => "making two live animals breed is the server's to accept, not a client's".to_owned(),
        "state" => "an animal's love and age are the client's synced state, which this host cannot read".to_owned(),
        "offspring" => {
            "a real offspring is spawned by the server; this host models the traits a rule \
             would inherit and leaves the spawn refused"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in breeding, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["live", "breed", "state", "offspring"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about live breeding: {}", why_unknown(what));
    false
}

/// An animal a mod declares, with the traits it may pass on.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Animal {
    pub id: String,
    pub kind: String,
    pub traits: BTreeMap<String, f32>,
}

impl Animal {
    #[must_use]
    pub fn new(id: &str, kind: &str) -> Self {
        Self {
            id: id.to_owned(),
            kind: kind.to_owned(),
            traits: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn trait_of(mut self, name: &str, value: f32) -> Self {
        self.traits.insert(name.to_owned(), value);
        self
    }
}

/// Which two kinds make which child, and how long the pair must rest afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreedingRule {
    pub parent: String,
    pub partner: String,
    pub child: String,
    pub cooldown: u32,
    pub mature_ticks: u32,
}

impl BreedingRule {
    #[must_use]
    pub fn new(parent: &str, partner: &str, child: &str) -> Self {
        Self {
            parent: parent.to_owned(),
            partner: partner.to_owned(),
            child: child.to_owned(),
            cooldown: 6000,
            mature_ticks: 100,
        }
    }

    #[must_use]
    pub fn cooldown(mut self, ticks: u32) -> Self {
        self.cooldown = ticks;
        self
    }

    #[must_use]
    pub fn mature_ticks(mut self, ticks: u32) -> Self {
        self.mature_ticks = ticks;
        self
    }

    /// Whether this rule covers two kinds, in either order.
    #[must_use]
    pub fn covers(&self, left: &str, right: &str) -> bool {
        (self.parent == left && self.partner == right) || (self.parent == right && self.partner == left)
    }
}

/// A child: its kind, the traits it inherited, and how far it is from grown.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Offspring {
    pub kind: String,
    pub traits: BTreeMap<String, f32>,
    /// Ticks of growth left, and how many it has already had.
    pub growth: u32,
    pub age: u32,
    pub mature: bool,
}

/// A pair of animals that is ready to breed, and what came of it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BreedingPair {
    pub id: String,
    pub a: String,
    pub b: String,
    pub love: bool,
    pub age: u32,
    pub cooldown: u32,
    pub child: Option<Offspring>,
}

fn parents() -> &'static Mutex<BTreeMap<String, Animal>> {
    static PARENTS: OnceLock<Mutex<BTreeMap<String, Animal>>> = OnceLock::new();
    PARENTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_parents<T>(body: impl FnOnce(&mut BTreeMap<String, Animal>) -> T) -> T {
    let mut guard = parents().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn rules() -> &'static Mutex<Vec<BreedingRule>> {
    static RULES: OnceLock<Mutex<Vec<BreedingRule>>> = OnceLock::new();
    RULES.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_rules<T>(body: impl FnOnce(&mut Vec<BreedingRule>) -> T) -> T {
    let mut guard = rules().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn pairs() -> &'static Mutex<BTreeMap<String, BreedingPair>> {
    static PAIRS: OnceLock<Mutex<BTreeMap<String, BreedingPair>>> = OnceLock::new();
    PAIRS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_pairs<T>(body: impl FnOnce(&mut BTreeMap<String, BreedingPair>) -> T) -> T {
    let mut guard = pairs().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn factories() -> &'static Mutex<Vec<String>> {
    static FACTORIES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    FACTORIES.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_factories<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = factories().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares an animal, or replaces the one with that id.
pub fn register_parent(animal: Animal) -> bool {
    with_parents(|parents| parents.insert(animal.id.clone(), animal).is_none())
}

/// Forgets a declared animal.
pub fn unregister_parent(id: &str) -> bool {
    with_parents(|parents| parents.remove(id).is_some())
}

/// A declared animal by id.
#[must_use]
pub fn get_parent(id: &str) -> Option<Animal> {
    with_parents(|parents| parents.get(id).cloned())
}

/// Every declared animal, in id order.
#[must_use]
pub fn get_parents() -> Vec<Animal> {
    with_parents(|parents| parents.values().cloned().collect())
}

/// Declares which two kinds make which child, once.
pub fn register_rule(rule: BreedingRule) -> bool {
    with_rules(|rules| {
        if rules.iter().any(|held| held.covers(&rule.parent, &rule.partner)) {
            false
        } else {
            rules.push(rule);
            true
        }
    })
}

/// Every breeding rule a mod registered.
#[must_use]
pub fn get_rules() -> Vec<BreedingRule> {
    with_rules(|rules| rules.clone())
}

fn rule_for(a: &str, b: &str) -> Option<BreedingRule> {
    let (left, right) = (get_parent(a)?, get_parent(b)?);
    with_rules(|rules| {
        rules
            .iter()
            .find(|rule| rule.covers(&left.kind, &right.kind))
            .cloned()
    })
}

/// The kind of child two declared animals would make.
#[must_use]
pub fn get_offspring_type(a: &str, b: &str) -> Option<String> {
    rule_for(a, b).map(|rule| rule.child)
}

/// The id a pair of parents is filed under.
#[must_use]
pub fn pair_id(a: &str, b: &str) -> String {
    format!("{a}+{b}")
}

/// Whether two declared animals could breed right now: a rule for their kinds, and no pair
/// already started.
#[must_use]
pub fn can_breed(a: &str, b: &str) -> bool {
    rule_for(a, b).is_some() && !with_pairs(|pairs| pairs.contains_key(&pair_id(a, b)))
}

/// Starts a pair breeding, and answers the id it was filed under. `None` when [`can_breed`]
/// would answer no.
pub fn start(a: &str, b: &str) -> Option<String> {
    if !can_breed(a, b) {
        return None;
    }
    let id = pair_id(a, b);
    with_pairs(|pairs| {
        pairs.insert(
            id.clone(),
            BreedingPair {
                id: id.clone(),
                a: a.to_owned(),
                b: b.to_owned(),
                love: true,
                age: 0,
                cooldown: 0,
                child: None,
            },
        );
    });
    Some(id)
}

/// Stops a pair, and answers whether there was one.
pub fn cancel(id: &str) -> bool {
    with_pairs(|pairs| pairs.remove(id).is_some())
}

/// A pair, by id.
#[must_use]
pub fn get_pair(id: &str) -> Option<BreedingPair> {
    with_pairs(|pairs| pairs.get(id).cloned())
}

/// A pair's restock cooldown.
#[must_use]
pub fn get_cooldown(id: &str) -> Option<u32> {
    get_pair(id).map(|pair| pair.cooldown)
}

/// Sets a pair's restock cooldown, and answers whether there was one.
pub fn set_cooldown(id: &str, ticks: u32) -> bool {
    with_pairs(|pairs| match pairs.get_mut(id) {
        Some(pair) => {
            pair.cooldown = ticks;
            true
        }
        None => false,
    })
}

/// Breeds a started pair: a child whose every shared trait comes from one parent, chosen by a
/// draw seeded with `seed`. `None` when there is no such pair or no rule for its kinds.
pub fn breed(id: &str, seed: u64) -> Option<Offspring> {
    let pair = get_pair(id)?;
    if !pair.love {
        return None;
    }
    let rule = rule_for(&pair.a, &pair.b)?;
    let (left, right) = (get_parent(&pair.a)?, get_parent(&pair.b)?);
    let mut rng = Random::new(seed);
    let mut traits = BTreeMap::new();
    let names: Vec<&String> = left.traits.keys().chain(right.traits.keys()).collect();
    let mut seen: Vec<String> = Vec::new();
    for name in names {
        if seen.contains(name) {
            continue;
        }
        seen.push(name.clone());
        let pick_left = rng.next_u32() & 1 == 0;
        let value = match (left.traits.get(name), right.traits.get(name)) {
            (Some(left_value), Some(right_value)) => Some(if pick_left { *left_value } else { *right_value }),
            (Some(left_value), None) => Some(*left_value),
            (None, Some(right_value)) => Some(*right_value),
            (None, None) => None,
        };
        if let Some(value) = value {
            traits.insert(name.clone(), value);
        }
    }
    let child = Offspring {
        kind: rule.child,
        traits,
        growth: rule.mature_ticks,
        age: 0,
        mature: rule.mature_ticks == 0,
    };
    with_pairs(|pairs| {
        if let Some(pair) = pairs.get_mut(id) {
            pair.love = false;
            pair.cooldown = rule.cooldown;
            pair.child = Some(child.clone());
        }
    });
    Some(child)
}

/// A pair's child, once it has one.
#[must_use]
pub fn get_child(id: &str) -> Option<Offspring> {
    get_pair(id).and_then(|pair| pair.child)
}

/// Advances every pair: its age, its cooldown and its child's growth. Answers how many
/// children matured on this call.
pub fn advance(ticks: u32) -> usize {
    let mut matured = 0;
    with_pairs(|pairs| {
        for pair in pairs.values_mut() {
            pair.age += ticks;
            pair.cooldown = pair.cooldown.saturating_sub(ticks);
            if let Some(child) = pair.child.as_mut() {
                if !child.mature {
                    child.age += ticks;
                    child.growth = child.growth.saturating_sub(ticks);
                    if child.growth == 0 {
                        child.mature = true;
                        matured += 1;
                    }
                }
            }
        }
    });
    matured
}

/// Records an offspring factory a mod ships, so a caller can see what can be built later.
pub fn register_offspring_factory(name: &str) -> bool {
    with_factories(|factories| {
        if factories.iter().any(|held| held == name) {
            false
        } else {
            factories.push(name.to_owned());
            true
        }
    })
}

/// The offspring factories a mod registered.
#[must_use]
pub fn get_offspring_factories() -> Vec<String> {
    with_factories(|factories| factories.clone())
}

/// A live animal. `None`, always: [`why_unknown`] says why.
#[must_use]
pub fn get_live_animal() -> Option<Animal> {
    None
}

/// Makes two live animals breed, which is the server's. `false`.
pub fn breed_live(_a: &str, _b: &str) -> bool {
    refused("breed")
}

impl fmt::Display for BreedingPair {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({} x {}, love={}, cooldown={})",
            self.id, self.a, self.b, self.love, self.cooldown
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_parents(|parents| parents.clear());
        with_rules(|rules| rules.clear());
        with_pairs(|pairs| pairs.clear());
        with_factories(|factories| factories.clear());
    }

    fn cows() {
        register_parent(Animal::new("a", "cow").trait_of("speed", 0.25).trait_of("health", 10.0));
        register_parent(Animal::new("b", "cow").trait_of("speed", 0.30));
        register_rule(BreedingRule::new("cow", "cow", "calf").cooldown(100).mature_ticks(60));
    }

    #[test]
    fn a_rule_answers_the_offspring_type() {
        let _serial = serial();
        fresh();
        cows();
        assert_eq!(get_offspring_type("a", "b"), Some("calf".to_owned()));
        assert_eq!(get_offspring_type("b", "a"), Some("calf".to_owned()), "order does not matter");
        assert_eq!(get_offspring_type("a", "nothing"), None);
        assert!(!register_rule(BreedingRule::new("cow", "cow", "other")), "one rule per pair of kinds");
        assert_eq!(get_rules().len(), 1);
        assert_eq!(get_parent("a").map(|animal| animal.kind), Some("cow".to_owned()));
        assert_eq!(get_parents().len(), 2);
    }

    #[test]
    fn a_pair_can_be_started_and_cancelled() {
        let _serial = serial();
        fresh();
        cows();
        assert!(can_breed("a", "b"));
        let id = start("a", "b").expect("a rule for two cows");
        assert_eq!(id, "a+b");
        assert!(!can_breed("a", "b"), "only one pair at a time");
        let pair = get_pair(&id).expect("started");
        assert!(pair.love);
        assert_eq!(pair.age, 0);
        assert_eq!(pair.cooldown, 0);
        let text = pair.to_string();
        assert!(text.contains("a+b"), "{text}");
        assert!(text.contains("love=true"), "{text}");
        assert!(cancel(&id));
        assert!(!cancel(&id));
        assert!(can_breed("a", "b"), "cancelling frees the pair");
        assert_eq!(start("a", "nothing"), None);
    }

    #[test]
    fn a_child_inherits_each_trait_from_one_parent_by_seed() {
        let _serial = serial();
        fresh();
        cows();
        let id = start("a", "b").expect("started");
        let first = breed(&id, 7).expect("a rule and parents");
        assert_eq!(first.kind, "calf");
        assert_eq!(first.traits.len(), 2, "speed and health");
        assert_eq!(first.traits.get("health"), Some(&10.0), "only a has health");
        let speed = first.traits.get("speed").copied().expect("a speed");
        assert!(speed == 0.25 || speed == 0.30, "speed came from a parent: {speed}");
        assert_eq!(first.growth, 60);
        assert!(!first.mature);
        // The same seed answers the same child; the pair is spent, so cancel and start again.
        cancel(&id);
        let id = start("a", "b").expect("started again");
        let again = breed(&id, 7).expect("again");
        assert_eq!(again.traits, first.traits, "one seed, one child");
        cancel(&id);
        let id = start("a", "b").expect("started a third time");
        let other = breed(&id, 8).expect("a different seed");
        assert_eq!(other.traits.len(), 2, "still every trait, whichever parent it came from");
        assert_eq!(get_pair(&id).map(|pair| pair.love), Some(false), "love is spent");
        assert_eq!(get_cooldown(&id), Some(100), "the rule's cooldown");
    }

    #[test]
    fn the_growth_timer_advances_and_matures() {
        let _serial = serial();
        fresh();
        cows();
        let id = start("a", "b").expect("started");
        breed(&id, 1).expect("a child");
        assert_eq!(get_child(&id).map(|child| child.growth), Some(60));
        assert_eq!(advance(20), 0, "not grown yet");
        assert_eq!(get_child(&id).map(|child| child.growth), Some(40));
        assert_eq!(get_child(&id).map(|child| child.age), Some(20));
        assert_eq!(get_child(&id).map(|child| child.mature), Some(false));
        assert_eq!(advance(40), 1, "the growth reached zero and it matured");
        let child = get_child(&id).expect("a child");
        assert!(child.mature);
        assert_eq!(child.growth, 0);
        assert_eq!(child.age, 60);
        assert_eq!(advance(5), 0, "a grown child does not mature twice");
    }

    #[test]
    fn the_cooldown_ticks_down_to_nothing() {
        let _serial = serial();
        fresh();
        cows();
        let id = start("a", "b").expect("started");
        assert!(set_cooldown(&id, 100));
        assert_eq!(get_cooldown(&id), Some(100));
        assert_eq!(advance(60), 0);
        assert_eq!(get_cooldown(&id), Some(40));
        assert_eq!(get_pair(&id).map(|pair| pair.age), Some(60), "the pair aged too");
        advance(100);
        assert_eq!(get_cooldown(&id), Some(0), "and never goes below zero");
        assert!(!set_cooldown("nothing", 5));
        assert_eq!(get_cooldown("nothing"), None);
    }

    #[test]
    fn the_offspring_factories_a_mod_registers_are_recorded() {
        let _serial = serial();
        fresh();
        assert!(register_offspring_factory("hello:twins"));
        assert!(!register_offspring_factory("hello:twins"), "once");
        assert_eq!(get_offspring_factories(), vec!["hello:twins".to_owned()]);
    }

    #[test]
    fn the_clients_animals_and_a_live_breeding_are_refused() {
        let _serial = serial();
        assert!(get_live_animal().is_none());
        assert!(!breed_live("a", "b"));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_breeding_event_kinds_can_be_subscribed_to_before_they_fire() {
        let _serial = serial();
        assert!(register_event_kinds() > 0);
        assert!(has_event_kind("offspring_born"));
        assert!(!has_event_kind("nothing_at_all"));
        assert_eq!(event_kinds().len(), 6);
    }
}
