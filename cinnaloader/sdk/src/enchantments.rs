//! Enchantments: the ones a mod declares here, the arithmetic it can own, and the client it
//! cannot reach.
//!
//! A mod declares an enchantment — an id, a display name, a maximum level, the item kinds it
//! applies to, a rarity, whether it is a treasure, and the enchantments it will not share an
//! item with — and every mod reads that registry back. An instance carries its level,
//! [`get_compatibility`] answers for a pair the way the game answers, and the cost curve is
//! real arithmetic a mod can quote at any level.
//!
//! An enchantment lands on an [`ItemStack`] this process owns, so one applied reads back at
//! its level and one removed is gone. What is out of reach is the client's enchanting table
//! and whatever sits enchanted on the client's own stacks, both refused with a reason in
//! [`why_unknown`].
//!
//! ```
//! use cinnaloader::enchantments::{self, Enchantment};
//! use cinnaloader::items::ItemStack;
//!
//! assert!(enchantments::register(
//!     Enchantment::new("hello:glow", "Glow").max_level(3).target("minecraft:sword")
//! ));
//! let mut sword = ItemStack::new("minecraft:sword", 1);
//! assert!(enchantments::apply(&mut sword, "hello:glow", 2));
//! assert_eq!(enchantments::get_level(&sword, "hello:glow"), Some(2));
//! assert_eq!(enchantments::get_min_cost("hello:glow", 3), Some(21));
//! assert!(!enchantments::enchant_at_table(&sword, 3), "the table is the client's");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::items::ItemStack;

/// How rare an enchantment is, which sets its cost curve and its weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rarity {
    #[default]
    Common,
    Uncommon,
    Rare,
    VeryRare,
}

impl Rarity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Uncommon => "uncommon",
            Self::Rare => "rare",
            Self::VeryRare => "very rare",
        }
    }

    /// The base, the per-level step and the span of the cost curve, which the game's own
    /// rarity table is modelled on.
    #[must_use]
    pub fn cost_curve(self) -> (u32, u32, u32) {
        match self {
            Self::Common => (1, 10, 5),
            Self::Uncommon => (11, 20, 10),
            Self::Rare => (21, 30, 15),
            Self::VeryRare => (31, 40, 20),
        }
    }
}

/// What an enchantment modifier changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnchantModifierTarget {
    Cost,
    Level,
}

/// One change a mod attaches to an enchantment, which its cost and its ceiling then honour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnchantmentModifier {
    pub name: String,
    pub target: EnchantModifierTarget,
    pub amount: i32,
}

/// An enchantment a mod declares.
#[derive(Debug, Clone, PartialEq)]
pub struct Enchantment {
    pub id: String,
    pub name: String,
    pub owner: String,
    /// The maximum level before any modifier raises it.
    pub base_level: u32,
    /// The item ids and tags it applies to; `*` matches anything.
    pub targets: Vec<String>,
    pub rarity: Rarity,
    pub treasure: bool,
    /// The enchantments it refuses to share an item with.
    pub incompatible: Vec<String>,
    /// The effect names the client would draw for it, which this host only records.
    pub effects: Vec<String>,
    pub modifiers: Vec<EnchantmentModifier>,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered: bool,
}

impl Enchantment {
    /// An enchantment with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            base_level: 1,
            targets: Vec::new(),
            rarity: Rarity::Common,
            treasure: false,
            incompatible: Vec::new(),
            effects: Vec::new(),
            modifiers: Vec::new(),
            registered: false,
        }
    }

    #[must_use]
    pub fn max_level(mut self, level: u32) -> Self {
        self.base_level = level.max(1);
        self
    }

    #[must_use]
    pub fn target(mut self, item: &str) -> Self {
        self.targets.push(item.to_owned());
        self
    }

    #[must_use]
    pub fn rarity(mut self, rarity: Rarity) -> Self {
        self.rarity = rarity;
        self
    }

    #[must_use]
    pub fn treasure(mut self) -> Self {
        self.treasure = true;
        self
    }

    #[must_use]
    pub fn incompatible(mut self, other: &str) -> Self {
        self.incompatible.push(other.to_owned());
        self
    }

    #[must_use]
    pub fn effect(mut self, effect: &str) -> Self {
        self.effects.push(effect.to_owned());
        self
    }

    #[must_use]
    pub fn modifier(mut self, modifier: EnchantmentModifier) -> Self {
        self.modifiers.retain(|held| held.name != modifier.name);
        self.modifiers.push(modifier);
        self
    }

    fn offset(&self, target: EnchantModifierTarget) -> i32 {
        self.modifiers
            .iter()
            .filter(|modifier| modifier.target == target)
            .map(|modifier| modifier.amount)
            .sum()
    }

    /// The maximum level, with the level modifiers applied.
    #[must_use]
    pub fn ceiling(&self) -> u32 {
        (self.base_level as i32 + self.offset(EnchantModifierTarget::Level)).max(1) as u32
    }

    /// The lowest enchanting cost at a level, on the rarity's curve.
    #[must_use]
    pub fn min_cost(&self, level: u32) -> u32 {
        let (base, per_level, _) = self.rarity.cost_curve();
        let curve = base + per_level * level.saturating_sub(1);
        (curve as i32 + self.offset(EnchantModifierTarget::Cost)).max(0) as u32
    }

    /// The highest enchanting cost at a level.
    #[must_use]
    pub fn max_cost(&self, level: u32) -> u32 {
        let (_, _, span) = self.rarity.cost_curve();
        self.min_cost(level) + span
    }

    /// Whether it applies to a stack: a matching item id, a matching tag, or the `*` target.
    #[must_use]
    pub fn can_apply_to(&self, stack: &ItemStack) -> bool {
        self.targets.iter().any(|target| {
            target == "*"
                || target == stack.item()
                || crate::items::has_tag(stack.item(), target)
        })
    }

    /// Whether it refuses to share an item with another.
    #[must_use]
    pub fn refuses(&self, other: &str) -> bool {
        self.incompatible.iter().any(|held| held == other)
    }
}

impl fmt::Display for Enchantment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, max {}, {})",
            self.name,
            self.id,
            self.ceiling(),
            self.rarity.as_str()
        )
    }
}

/// One enchantment at a level, which is what compatibility and combining speak about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnchantmentInstance {
    pub id: String,
    pub level: u32,
}

impl EnchantmentInstance {
    #[must_use]
    pub fn new(id: &str, level: u32) -> Self {
        Self {
            id: id.to_owned(),
            level: level.max(1),
        }
    }
}

/// How two enchantment instances sit on one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compatibility {
    /// The same enchantment twice, which an anvil merges.
    Same,
    /// Different enchantments that may share an item.
    Compatible,
    /// A pair one of them, or the registry, refuses.
    Incompatible,
}

fn registry() -> &'static Mutex<BTreeMap<String, Enchantment>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, Enchantment>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, Enchantment>) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares an enchantment, or replaces the one with that id.
pub fn register(enchantment: Enchantment) -> bool {
    with(|registry| registry.insert(enchantment.id.clone(), enchantment).is_none())
}

/// Forgets a declared enchantment.
pub fn unregister(id: &str) -> bool {
    with(|registry| registry.remove(id).is_some())
}

/// A declared enchantment, by id.
#[must_use]
pub fn get(id: &str) -> Option<Enchantment> {
    with(|registry| registry.get(id).cloned())
}

/// Every declared enchantment, in id order.
#[must_use]
pub fn get_all() -> Vec<Enchantment> {
    with(|registry| registry.values().cloned().collect())
}

/// A declared enchantment's maximum level.
#[must_use]
pub fn get_max_level(id: &str) -> Option<u32> {
    get(id).map(|enchantment| enchantment.ceiling())
}

/// The lowest enchanting cost of a declared enchantment at a level.
#[must_use]
pub fn get_min_cost(id: &str, level: u32) -> Option<u32> {
    get(id).map(|enchantment| enchantment.min_cost(level))
}

/// The highest enchanting cost of a declared enchantment at a level.
#[must_use]
pub fn get_max_cost(id: &str, level: u32) -> Option<u32> {
    get(id).map(|enchantment| enchantment.max_cost(level))
}

/// Whether a declared enchantment applies to a stack.
#[must_use]
pub fn can_apply(id: &str, stack: &ItemStack) -> bool {
    get(id).is_some_and(|enchantment| enchantment.can_apply_to(stack))
}

fn refuses(left: &str, right: &str) -> bool {
    get(left).is_some_and(|enchantment| enchantment.refuses(right))
}

/// How two instances sit on one item: the same enchantment, compatible, or refused.
#[must_use]
pub fn get_compatibility(a: &EnchantmentInstance, b: &EnchantmentInstance) -> Compatibility {
    if get(&a.id).is_none() || get(&b.id).is_none() {
        return Compatibility::Incompatible;
    }
    if a.id == b.id {
        return Compatibility::Same;
    }
    if refuses(&a.id, &b.id) || refuses(&b.id, &a.id) {
        Compatibility::Incompatible
    } else {
        Compatibility::Compatible
    }
}

/// Whether a pair may share an item.
#[must_use]
pub fn can_combine(a: &EnchantmentInstance, b: &EnchantmentInstance) -> bool {
    get_compatibility(a, b) != Compatibility::Incompatible
}

/// Combines two of the same enchantment the way an anvil does: the higher level, or one more
/// when they match, never past the maximum.
#[must_use]
pub fn resolve(a: &EnchantmentInstance, b: &EnchantmentInstance) -> Option<EnchantmentInstance> {
    if get_compatibility(a, b) != Compatibility::Same {
        return None;
    }
    let ceiling = get_max_level(&a.id).unwrap_or(u32::MAX);
    let level = if a.level == b.level {
        a.level.saturating_add(1)
    } else {
        a.level.max(b.level)
    };
    Some(EnchantmentInstance::new(&a.id, level.min(ceiling)))
}

/// The level an enchantment sits at on a stack.
#[must_use]
pub fn get_level(stack: &ItemStack, id: &str) -> Option<u32> {
    stack.enchantments().get(id).map(|level| (*level).max(0) as u32)
}

/// Puts a declared enchantment on a stack, at a level within its maximum, when it applies.
pub fn apply(stack: &mut ItemStack, id: &str, level: u32) -> bool {
    let Some(enchantment) = get(id) else {
        return false;
    };
    if !enchantment.can_apply_to(stack) {
        return false;
    }
    stack.add_enchantment(id, clamp_level(level, enchantment.ceiling()) as i32);
    true
}

/// Changes the level of an enchantment already on a stack.
pub fn modify_level(stack: &mut ItemStack, id: &str, level: u32) -> bool {
    let Some(enchantment) = get(id) else {
        return false;
    };
    if !stack.enchantments().contains_key(id) {
        return false;
    }
    stack.add_enchantment(id, clamp_level(level, enchantment.ceiling()) as i32);
    true
}

/// Takes an enchantment off a stack.
pub fn remove(stack: &mut ItemStack, id: &str) -> bool {
    stack.remove_enchantment(id)
}

fn clamp_level(level: u32, ceiling: u32) -> u32 {
    level.clamp(1, ceiling.max(1))
}

/// Records an effect name on a declared enchantment, which the client would draw.
pub fn register_effect(id: &str, effect: &str) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(enchantment) => {
            enchantment.effects.push(effect.to_owned());
            true
        }
        None => false,
    })
}

/// Records one more item kind a declared enchantment applies to.
pub fn register_target(id: &str, target: &str) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(enchantment) => {
            enchantment.targets.push(target.to_owned());
            true
        }
        None => false,
    })
}

/// Attaches a modifier to a declared enchantment, which its cost and ceiling then honour.
pub fn register_modifier(id: &str, modifier: EnchantmentModifier) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(enchantment) => {
            enchantment.modifiers.retain(|held| held.name != modifier.name);
            enchantment.modifiers.push(modifier);
            true
        }
        None => false,
    })
}

/// Why an enchanting question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "table" => {
            "the enchanting table is the client's screen and it rolls the offers itself, \
             which this host cannot ask it to do"
                .to_owned()
        }
        "enchantments" => {
            "the enchantments on the client's own stacks are client state, which this host \
             cannot read"
                .to_owned()
        }
        "stack" => {
            "an item stack the server or the client owns is not this host's to enchant"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// Asks the client's enchanting table to enchant a stack. This host cannot.
pub fn enchant_at_table(_stack: &ItemStack, _slot: u32) -> bool {
    crate::warn!(
        "a mod asked this host to enchant at the table: {}",
        why_unknown("table")
    );
    false
}

/// The enchantments on the client's own stacks. Empty: they are the client's.
#[must_use]
pub fn read_client_enchantments() -> Vec<EnchantmentInstance> {
    crate::warn!(
        "a mod asked this host for the client's item enchantments: {}",
        why_unknown("enchantments")
    );
    Vec::new()
}

/// Puts an enchantment on the server's or the client's own stack. This host cannot.
pub fn write_client_stack(_id: &str, _level: u32) -> bool {
    crate::warn!(
        "a mod asked this host to enchant the client's own stack: {}",
        why_unknown("stack")
    );
    false
}

/// What this host cannot reach about enchantments.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["table", "enchantments", "stack"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|registry| registry.clear());
    }

    #[test]
    fn an_enchantment_is_declared_and_read_back() {
        let _serial = serial();
        fresh();
        assert!(register(
            Enchantment::new("hello:glow", "Glow")
                .max_level(3)
                .target("minecraft:sword")
                .target("minecraft:axe")
                .rarity(Rarity::Rare)
                .treasure()
                .incompatible("minecraft:sharpness")
                .effect("hello:shine")
        ));
        let back = get("hello:glow").expect("declared");
        assert_eq!(back.name, "Glow");
        assert_eq!(back.ceiling(), 3);
        assert_eq!(get_max_level("hello:glow"), Some(3));
        assert_eq!(back.rarity, Rarity::Rare);
        assert!(back.treasure);
        assert!(back.refuses("minecraft:sharpness"));
        assert_eq!(back.targets.len(), 2);
        assert_eq!(back.effects, vec!["hello:shine".to_owned()]);
        assert!(!back.registered, "the client was never told");
        assert_eq!(get_all().len(), 1);
        assert!(back.to_string().contains("Glow"), "{back}");
        assert!(!register(Enchantment::new("hello:glow", "Glow again")), "it replaces");
        assert_eq!(get("hello:glow").map(|enchantment| enchantment.name), Some("Glow again".to_owned()));
        assert!(unregister("hello:glow"));
        assert!(!unregister("hello:glow"));
    }

    #[test]
    fn the_cost_curve_scales_with_the_level() {
        let _serial = serial();
        fresh();
        register(
            Enchantment::new("minecraft:sharpness", "Sharpness")
                .max_level(5)
                .target("minecraft:sword"),
        );
        assert_eq!(get_min_cost("minecraft:sharpness", 1), Some(1));
        assert_eq!(get_max_cost("minecraft:sharpness", 1), Some(6));
        assert_eq!(get_min_cost("minecraft:sharpness", 3), Some(21));
        assert_eq!(get_max_cost("minecraft:sharpness", 3), Some(26));
        assert_eq!(get_min_cost("minecraft:sharpness", 5), Some(41));
        assert_eq!(get_max_cost("minecraft:sharpness", 5), Some(46));
        assert_eq!(get_min_cost("nothing", 3), None);

        assert!(register_modifier(
            "minecraft:sharpness",
            EnchantmentModifier {
                name: "hello:discount".to_owned(),
                target: EnchantModifierTarget::Cost,
                amount: -1,
            },
        ));
        assert_eq!(get_min_cost("minecraft:sharpness", 3), Some(20));
        assert!(!register_modifier(
            "nothing",
            EnchantmentModifier {
                name: "x".to_owned(),
                target: EnchantModifierTarget::Cost,
                amount: 1,
            },
        ));
    }

    #[test]
    fn an_incompatible_pair_is_refused_and_a_pair_resolves() {
        let _serial = serial();
        fresh();
        register(
            Enchantment::new("minecraft:sharpness", "Sharpness")
                .max_level(5)
                .target("minecraft:sword")
                .incompatible("minecraft:smite"),
        );
        register(
            Enchantment::new("minecraft:smite", "Smite")
                .max_level(5)
                .target("minecraft:sword")
                .incompatible("minecraft:sharpness"),
        );
        register(
            Enchantment::new("minecraft:looting", "Looting")
                .max_level(3)
                .target("minecraft:sword"),
        );

        let sharp = EnchantmentInstance::new("minecraft:sharpness", 2);
        let smite = EnchantmentInstance::new("minecraft:smite", 1);
        let loot = EnchantmentInstance::new("minecraft:looting", 1);
        assert_eq!(get_compatibility(&sharp, &smite), Compatibility::Incompatible);
        assert!(!can_combine(&sharp, &smite));
        assert!(resolve(&sharp, &smite).is_none());
        assert_eq!(get_compatibility(&sharp, &loot), Compatibility::Compatible);
        assert!(can_combine(&sharp, &loot));
        assert!(resolve(&sharp, &loot).is_none(), "different enchantments do not merge");
        assert_eq!(
            get_compatibility(&sharp, &EnchantmentInstance::new("hello:unknown", 1)),
            Compatibility::Incompatible,
            "an unregistered one cannot be confirmed"
        );

        let two = EnchantmentInstance::new("minecraft:sharpness", 2);
        let four = EnchantmentInstance::new("minecraft:sharpness", 4);
        assert_eq!(get_compatibility(&two, &four), Compatibility::Same);
        assert_eq!(resolve(&two, &four).map(|held| held.level), Some(4), "the higher level wins");
        assert_eq!(resolve(&two, &two).map(|held| held.level), Some(3), "equal levels go up one");
        let five = EnchantmentInstance::new("minecraft:sharpness", 5);
        assert_eq!(resolve(&five, &five).map(|held| held.level), Some(5), "capped at the maximum");
    }

    #[test]
    fn an_enchantment_applies_to_a_stack_and_reads_back() {
        let _serial = serial();
        fresh();
        register(
            Enchantment::new("minecraft:sharpness", "Sharpness")
                .max_level(5)
                .target("minecraft:sword"),
        );
        let mut sword = ItemStack::new("minecraft:sword", 1);
        assert!(can_apply("minecraft:sharpness", &sword));
        assert!(apply(&mut sword, "minecraft:sharpness", 3));
        assert_eq!(get_level(&sword, "minecraft:sharpness"), Some(3));
        assert!(sword.is_enchanted());
        assert!(modify_level(&mut sword, "minecraft:sharpness", 9));
        assert_eq!(get_level(&sword, "minecraft:sharpness"), Some(5), "clamped to the maximum");
        assert!(remove(&mut sword, "minecraft:sharpness"));
        assert_eq!(get_level(&sword, "minecraft:sharpness"), None);
        assert!(!remove(&mut sword, "minecraft:sharpness"));
        assert!(!modify_level(&mut sword, "minecraft:sharpness", 2), "it is gone");

        let mut stone = ItemStack::new("minecraft:stone", 1);
        assert!(!can_apply("minecraft:sharpness", &stone));
        assert!(!apply(&mut stone, "minecraft:sharpness", 1), "the wrong kind of item");
        assert!(!apply(&mut stone, "nothing", 1), "an unregistered one");
    }

    #[test]
    fn registration_helpers_add_targets_effects_and_modifiers() {
        let _serial = serial();
        fresh();
        register(Enchantment::new("hello:glow", "Glow").max_level(2));
        assert!(register_target("hello:glow", "minecraft:sword"));
        assert!(register_effect("hello:glow", "hello:shine"));
        assert!(register_modifier(
            "hello:glow",
            EnchantmentModifier {
                name: "hello:more".to_owned(),
                target: EnchantModifierTarget::Level,
                amount: 1,
            },
        ));
        let back = get("hello:glow").expect("declared");
        assert_eq!(back.targets, vec!["minecraft:sword".to_owned()]);
        assert_eq!(back.effects, vec!["hello:shine".to_owned()]);
        assert_eq!(back.ceiling(), 3, "the level modifier raises the ceiling");
        assert_eq!(get_max_level("hello:glow"), Some(3));
        assert!(!register_target("nothing", "minecraft:sword"));
        assert!(!register_effect("nothing", "x"));
        assert!(!register_modifier(
            "nothing",
            EnchantmentModifier {
                name: "x".to_owned(),
                target: EnchantModifierTarget::Cost,
                amount: 1,
            },
        ));
    }

    #[test]
    fn a_wildcard_target_applies_to_any_item() {
        let _serial = serial();
        fresh();
        register(Enchantment::new("minecraft:mending", "Mending").max_level(1).target("*"));
        let mut anything = ItemStack::new("minecraft:diamond_pickaxe", 1);
        assert!(can_apply("minecraft:mending", &anything));
        assert!(apply(&mut anything, "minecraft:mending", 1));
        assert_eq!(get_level(&anything, "minecraft:mending"), Some(1));
    }

    #[test]
    fn what_the_client_owns_is_refused_with_a_reason() {
        let _serial = serial();
        let sword = ItemStack::new("minecraft:sword", 1);
        assert!(!enchant_at_table(&sword, 3));
        assert!(read_client_enchantments().is_empty());
        assert!(!write_client_stack("minecraft:sharpness", 1));
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
