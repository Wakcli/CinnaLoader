//! Status effects and damage: what a mod registers, and the arithmetic it can own.
//!
//! Both halves here are mostly *data a mod owns plus a calculation*, which is why this
//! section is more real than the entity ones around it.
//!
//! **Effects.** A mod registers an effect type — its id, name, whether it is good or bad,
//! its colour, its icon and particle names, its default duration and amplifier, and whether
//! it has a behaviour — and every mod can read that registry. Applying, removing, clearing
//! and reading an effect work on the [`PlayerState`] a mod owns, which is the same list
//! section 21 keeps, so an effect a mod applies to its own state is really there.
//!
//! **Damage.** [`DamageInstance`] is a value: an amount, a type, a source, the armour
//! toughness, resistance, enchantment protection and cooldown that apply, and the knockback.
//! [`calculate`] is the real arithmetic — armour first, then resistance, then enchantment
//! protection, then the cooldown multiplier — and it is a pure function, so a mod can call it
//! as often as it likes. Damage *types*, *sources* and *modifiers* are registries a mod owns.
//! What is out of reach is the client's own damage path: `apply` on a live entity and
//! `modify_calculation` both need a hook the client does not offer here.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! effects::register(EffectType::new("hello:haste", "Haste").beneficial().icon("hello:haste"));
//! let mut who = PlayerState::new("Tester");
//! effects::apply(&mut who, "hello:haste", 200, 1);
//! assert!(effects::has(&who, "hello:haste"));
//!
//! let hit = DamageInstance::new(10.0, "minecraft:generic").with_armor(4.0);
//! assert!(damage::calculate(&hit) < 10.0, "armour takes some of it");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events;
use crate::player::{Effect, PlayerState};

/// The event kinds this section lists, which are section 19's and 21's under one heading
/// here. Registering them teaches the bus each name.
pub const EFFECT_EVENTS: [&str; 33] = [
    "entity_spawn",
    "entity_despawn",
    "entity_death",
    "entity_damage",
    "entity_heal",
    "entity_attack",
    "entity_knockback",
    "entity_teleport",
    "entity_move",
    "entity_collision",
    "entity_target",
    "entity_mount",
    "entity_dismount",
    "entity_effect_add",
    "entity_effect_remove",
    "entity_attribute_change",
    "entity_equipment_change",
    "entity_name_change",
    "entity_transform",
    "player_join",
    "player_leave",
    "player_respawn",
    "player_death",
    "player_game_mode_change",
    "player_experience_change",
    "player_food_change",
    "player_jump",
    "player_land",
    "player_sneak",
    "player_sprint",
    "player_swim",
    "player_fly",
    "player_interact",
];

/// Teaches the event bus each name this section lists.
pub fn register_event_kinds() -> usize {
    EFFECT_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// The event kinds this host would need from the client to fire one.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    EFFECT_EVENTS.to_vec()
}

/// Whether one of them is a kind the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    EFFECT_EVENTS.contains(&kind) && events::is_registered_kind(kind)
}

/// Whether an effect does a player good or harm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EffectCategory {
    #[default]
    Beneficial,
    Harmful,
    Neutral,
}

impl EffectCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Beneficial => "beneficial",
            Self::Harmful => "harmful",
            Self::Neutral => "neutral",
        }
    }
}

/// An effect type a mod registers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectType {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub category: EffectCategory,
    /// The colour it is drawn in, `0xRRGGBB`, when the client would draw it.
    pub color: u32,
    /// The icon name a mod ships; the client's own icon is not reachable.
    pub icon: String,
    /// The particle name a mod ships.
    pub particles: String,
    pub default_duration: u64,
    pub default_amplifier: u8,
    /// Whether the mod has a behaviour for it, which this host records and never calls.
    pub has_behavior: bool,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered: bool,
}

impl EffectType {
    /// An effect with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            category: EffectCategory::Neutral,
            color: 0x00_ffff,
            icon: String::new(),
            particles: String::new(),
            default_duration: 0,
            default_amplifier: 0,
            has_behavior: false,
            registered: false,
        }
    }

    #[must_use]
    pub fn beneficial(mut self) -> Self {
        self.category = EffectCategory::Beneficial;
        self
    }

    #[must_use]
    pub fn harmful(mut self) -> Self {
        self.category = EffectCategory::Harmful;
        self
    }

    #[must_use]
    pub fn neutral(mut self) -> Self {
        self.category = EffectCategory::Neutral;
        self
    }

    #[must_use]
    pub fn color(mut self, color: u32) -> Self {
        self.color = color & 0x00ff_ffff;
        self
    }

    #[must_use]
    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = icon.to_owned();
        self
    }

    #[must_use]
    pub fn particles(mut self, particles: &str) -> Self {
        self.particles = particles.to_owned();
        self
    }

    #[must_use]
    pub fn duration(mut self, duration_millis: u64) -> Self {
        self.default_duration = duration_millis;
        self
    }

    #[must_use]
    pub fn amplifier(mut self, amplifier: u8) -> Self {
        self.default_amplifier = amplifier;
        self
    }

    #[must_use]
    pub fn with_behavior(mut self) -> Self {
        self.has_behavior = true;
        self
    }

    /// The effect value this type describes, ready to apply.
    #[must_use]
    pub fn effect(&self) -> Effect {
        Effect::new(&self.id, self.default_amplifier, self.default_duration)
    }
}

impl fmt::Display for EffectType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}, colour #{:06x})",
            self.name,
            self.id,
            self.category.as_str(),
            self.color
        )
    }
}

fn effects() -> &'static Mutex<BTreeMap<String, EffectType>> {
    static EFFECTS: OnceLock<Mutex<BTreeMap<String, EffectType>>> = OnceLock::new();
    EFFECTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_effects<T>(body: impl FnOnce(&mut BTreeMap<String, EffectType>) -> T) -> T {
    let mut guard = effects().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers an effect type, or replaces the one with that id.
pub fn register(effect: EffectType) -> bool {
    with_effects(|effects| effects.insert(effect.id.clone(), effect).is_none())
}

/// Forgets an effect type.
pub fn unregister(id: &str) -> bool {
    with_effects(|effects| effects.remove(id).is_some())
}

/// A registered effect type, by id.
#[must_use]
pub fn get_by_id(id: &str) -> Option<EffectType> {
    with_effects(|effects| effects.get(id).cloned())
}

/// A registered effect type, by name.
#[must_use]
pub fn get_by_name(name: &str) -> Option<EffectType> {
    with_effects(|effects| {
        effects
            .values()
            .find(|effect| effect.name == name)
            .cloned()
    })
}

/// Every registered effect type.
#[must_use]
pub fn get_all() -> Vec<EffectType> {
    with_effects(|effects| effects.values().cloned().collect())
}

/// The one effect of that id on a player, if it is there.
#[must_use]
pub fn get(player: &PlayerState, id: &str) -> Option<Effect> {
    player.effects.iter().find(|effect| effect.name == id).cloned()
}

/// Applies an effect to a player a mod owns, replacing one of the same id.
pub fn apply(player: &mut PlayerState, id: &str, duration_millis: u64, amplifier: u8) -> bool {
    let effect = match get_by_id(id) {
        Some(kind) if !duration_millis_is_overridden(duration_millis) => Effect::new(
            &kind.id,
            amplifier,
            if duration_millis == 0 {
                kind.default_duration
            } else {
                duration_millis
            },
        ),
        Some(kind) => Effect::new(&kind.id, amplifier, duration_millis),
        None => Effect::new(id, amplifier, duration_millis),
    };
    player.apply_effect(effect);
    true
}

fn duration_millis_is_overridden(duration_millis: u64) -> bool {
    duration_millis != 0
}

/// Takes an effect off a player.
pub fn remove(player: &mut PlayerState, id: &str) -> bool {
    player.remove_effect(id)
}

/// Takes every effect off a player, and answers how many went.
pub fn clear(player: &mut PlayerState) -> usize {
    let count = player.effects.len();
    player.effects.clear();
    count
}

/// Whether a player has an effect.
#[must_use]
pub fn has(player: &PlayerState, id: &str) -> bool {
    player.effects.iter().any(|effect| effect.name == id)
}

/// How long an effect has left.
#[must_use]
pub fn get_duration(player: &PlayerState, id: &str) -> Option<u64> {
    get(player, id).map(|effect| effect.duration)
}

/// Sets how long an effect has left.
pub fn set_duration(player: &mut PlayerState, id: &str, duration_millis: u64) -> bool {
    match player.effects.iter_mut().find(|effect| effect.name == id) {
        Some(effect) => {
            effect.duration = duration_millis;
            true
        }
        None => false,
    }
}

/// How strong an effect is, counting from zero the way Bedrock does.
#[must_use]
pub fn get_amplifier(player: &PlayerState, id: &str) -> Option<u8> {
    get(player, id).map(|effect| effect.amplifier)
}

/// Sets how strong an effect is.
pub fn set_amplifier(player: &mut PlayerState, id: &str, amplifier: u8) -> bool {
    match player.effects.iter_mut().find(|effect| effect.name == id) {
        Some(effect) => {
            effect.amplifier = amplifier;
            true
        }
        None => false,
    }
}

/// Records that a mod has a behaviour for an effect, which this host never calls.
pub fn register_behavior(id: &str) -> bool {
    with_effects(|effects| match effects.get_mut(id) {
        Some(effect) => {
            effect.has_behavior = true;
            true
        }
        None => false,
    })
}

/// Records an effect's icon name. The client's own icon is not reachable.
pub fn register_icon(id: &str, icon: &str) -> bool {
    with_effects(|effects| match effects.get_mut(id) {
        Some(effect) => {
            effect.icon = icon.to_owned();
            true
        }
        None => false,
    })
}

/// Records an effect's particle name.
pub fn register_particles(id: &str, particles: &str) -> bool {
    with_effects(|effects| match effects.get_mut(id) {
        Some(effect) => {
            effect.particles = particles.to_owned();
            true
        }
        None => false,
    })
}

/// Applies an effect to the live player. `false`: the server decides that.
pub fn apply_live(_id: &str, _duration_millis: u64, _amplifier: u8) -> bool {
    crate::warn!(
        "a mod asked this host to apply an effect to the player: {}",
        why_unknown("authority")
    );
    false
}

/// The damage half of the section: a hit a mod owns from its amount to its knockback, the
/// damage types and sources it registers, and the arithmetic — armour, then resistance, then
/// enchantment protection, then the cooldown multiplier, then the mod's own final modifiers.
pub mod damage {
    use super::*;

    /// Why applying damage or changing the calculation has no answer here.
    #[must_use]
    pub fn why_unknown(what: &str) -> String {
        match what {
            "authority" => {
                "applying damage to a live entity is the server's, and this host cannot even \
                 ask the client to send it"
                    .to_owned()
            }
            "calculation" => {
                "the client's own damage calculation is inside the client, and changing it \
                 needs a hook that does not exist here"
                    .to_owned()
            }
            other => super::why_unknown(other),
        }
    }

    /// A damage type a mod registers: what kind of harm it is and how it behaves.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct DamageType {
        pub id: String,
        pub owner: String,
        /// Whether armour takes any of it off.
        pub bypasses_armor: bool,
        /// Whether resistance takes any of it off.
        pub bypasses_resistance: bool,
        /// Whether it counts as fire, which is what a player's fire resistance answers.
        pub is_fire: bool,
        /// Whether it is the kind that knocks a player back.
        pub knocks_back: bool,
        pub death_message: String,
    }

    impl DamageType {
        #[must_use]
        pub fn new(id: &str) -> Self {
            Self {
                id: id.to_owned(),
                owner: crate::metadata::id(),
                bypasses_armor: false,
                bypasses_resistance: false,
                is_fire: false,
                knocks_back: true,
                death_message: format!("was slain by {id}"),
            }
        }

        #[must_use]
        pub fn bypasses_armor(mut self) -> Self {
            self.bypasses_armor = true;
            self
        }

        #[must_use]
        pub fn bypasses_resistance(mut self) -> Self {
            self.bypasses_resistance = true;
            self
        }

        #[must_use]
        pub fn fire(mut self) -> Self {
            self.is_fire = true;
            self
        }

        #[must_use]
        pub fn no_knockback(mut self) -> Self {
            self.knocks_back = false;
            self
        }

        #[must_use]
        pub fn death_message(mut self, message: &str) -> Self {
            self.death_message = message.to_owned();
            self
        }
    }

    /// Where the damage came from.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct DamageSource {
        pub id: String,
        /// Who or what caused it.
        pub cause: String,
        /// Who is credited with it, when a player is.
        pub attacker: Option<String>,
        /// The weapon, when there was one.
        pub weapon: Option<String>,
        pub indirect: bool,
    }

    impl DamageSource {
        /// A source with just an id.
        #[must_use]
        pub fn new(id: &str) -> Self {
            Self {
                id: id.to_owned(),
                cause: id.to_owned(),
                attacker: None,
                weapon: None,
                indirect: false,
            }
        }

        #[must_use]
        pub fn by(mut self, attacker: &str) -> Self {
            self.attacker = Some(attacker.to_owned());
            self
        }

        #[must_use]
        pub fn with_weapon(mut self, weapon: &str) -> Self {
            self.weapon = Some(weapon.to_owned());
            self
        }

        #[must_use]
        pub fn cause(mut self, cause: &str) -> Self {
            self.cause = cause.to_owned();
            self
        }

        #[must_use]
        pub fn indirect(mut self) -> Self {
            self.indirect = true;
            self
        }
    }

    /// How a modifier changes a damage calculation.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum DamageModifierKind {
        /// Adds to the amount before reductions.
        Add,
        /// Multiplies the amount before reductions.
        Multiply,
        /// Applies after all reductions.
        Final,
    }

    /// One change to a damage calculation, which a mod registers and applies itself.
    #[derive(Debug, Clone, PartialEq)]
    pub struct DamageModifier {
        pub name: String,
        pub kind: DamageModifierKind,
        pub amount: f64,
    }

    /// One instance of damage, which a mod owns from the amount to the knockback.
    #[derive(Debug, Clone, PartialEq)]
    pub struct DamageInstance {
        pub amount: f64,
        pub damage_type: String,
        pub source: DamageSource,
        /// Armour points that apply.
        pub armor: f64,
        /// How much of the damage the armour lets through anyway, 0 to 1.
        pub armor_toughness: f64,
        /// Resistance level, which takes 20% off per level.
        pub resistance: u8,
        /// Enchantment protection points that apply.
        pub enchantment_protection: u8,
        /// Milliseconds since the target was last hurt.
        pub since_last_hit: u64,
        pub knockback: f64,
        pub cancelled: bool,
        pub modifiers: Vec<DamageModifier>,
    }

    impl DamageInstance {
        /// A hit of `amount` of a damage type, with nothing taking anything off it.
        #[must_use]
        pub fn new(amount: f64, damage_type: &str) -> Self {
            Self {
                amount: amount.max(0.0),
                damage_type: damage_type.to_owned(),
                source: DamageSource::new(damage_type),
                armor: 0.0,
                armor_toughness: 0.0,
                resistance: 0,
                enchantment_protection: 0,
                since_last_hit: u64::MAX,
                knockback: 0.4,
                cancelled: false,
                modifiers: Vec::new(),
            }
        }

        #[must_use]
        pub fn with_armor(mut self, armor: f64) -> Self {
            self.armor = armor.max(0.0);
            self
        }

        #[must_use]
        pub fn with_toughness(mut self, toughness: f64) -> Self {
            self.armor_toughness = toughness.max(0.0);
            self
        }

        #[must_use]
        pub fn with_resistance(mut self, resistance: u8) -> Self {
            self.resistance = resistance;
            self
        }

        #[must_use]
        pub fn with_protection(mut self, enchantment_protection: u8) -> Self {
            self.enchantment_protection = enchantment_protection;
            self
        }

        #[must_use]
        pub fn with_source(mut self, source: DamageSource) -> Self {
            self.source = source;
            self
        }

        #[must_use]
        pub fn with_knockback(mut self, knockback: f64) -> Self {
            self.knockback = knockback;
            self
        }

        /// How long since the target was last hurt, which decides the cooldown multiplier.
        #[must_use]
        pub fn with_cooldown(mut self, since_last_hit: u64) -> Self {
            self.since_last_hit = since_last_hit;
            self
        }

        #[must_use]
        pub fn with_modifier(mut self, modifier: DamageModifier) -> Self {
            self.modifiers.retain(|held| held.name != modifier.name);
            self.modifiers.push(modifier);
            self
        }

        /// The amount before any reduction.
        #[must_use]
        pub fn base(&self) -> f64 {
            let mut amount = self.amount
                + self
                    .modifiers
                    .iter()
                    .filter(|modifier| modifier.kind == DamageModifierKind::Add)
                    .map(|modifier| modifier.amount)
                    .sum::<f64>();
            for modifier in self
                .modifiers
                .iter()
                .filter(|modifier| modifier.kind == DamageModifierKind::Multiply)
            {
                amount *= 1.0 + modifier.amount;
            }
            amount.max(0.0)
        }

        /// What the armour takes off, as a fraction between 0 and 1. Bedrock's own rule: armour
        /// and toughness together, capped at 80%.
        #[must_use]
        pub fn armor_reduction(&self) -> f64 {
            if armor_is_bypassed(&self.damage_type) {
                return 0.0;
            }
            let effective = self.armor / (self.armor + 8.0 + self.armor_toughness * 2.0) * 20.0;
            (effective / 25.0).clamp(0.0, 0.8)
        }

        /// What resistance takes off: 20% per level, capped at 80%.
        #[must_use]
        pub fn resistance_reduction(&self) -> f64 {
            if resistance_is_bypassed(&self.damage_type) {
                return 0.0;
            }
            (f64::from(self.resistance) * 0.2).clamp(0.0, 0.8)
        }

        /// What enchantment protection takes off: 4% per point, capped at 80%.
        #[must_use]
        pub fn enchantment_reduction(&self) -> f64 {
            (f64::from(self.enchantment_protection) * 0.04).clamp(0.0, 0.8)
        }

        /// The multiplier the cooldown applies: a hit straight after another counts less.
        #[must_use]
        pub fn cooldown_multiplier(&self) -> f64 {
            let cooldown = 500;
            if self.since_last_hit >= cooldown {
                return 1.0;
            }
            (0.2 + 0.8 * (self.since_last_hit as f64 / cooldown as f64)).clamp(0.2, 1.0)
        }

        /// The damage that actually lands, with everything applied in the order the game does.
        #[must_use]
        pub fn resolve(&self) -> f64 {
            if self.cancelled {
                return 0.0;
            }
            let mut amount = self.base();
            amount *= 1.0 - self.armor_reduction();
            amount *= 1.0 - self.resistance_reduction();
            amount *= 1.0 - self.enchantment_reduction();
            amount *= self.cooldown_multiplier();
            for modifier in self
                .modifiers
                .iter()
                .filter(|modifier| modifier.kind == DamageModifierKind::Final)
            {
                amount *= 1.0 + modifier.amount;
            }
            amount.max(0.0)
        }
    }

    impl fmt::Display for DamageInstance {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "{} of {} landing {:.2} (armour -{:.0}%, resistance -{:.0}%, protection -{:.0}%)",
                self.amount,
                self.damage_type,
                self.resolve(),
                self.armor_reduction() * 100.0,
                self.resistance_reduction() * 100.0,
                self.enchantment_reduction() * 100.0
            )
        }
    }

    fn damage_types() -> &'static Mutex<BTreeMap<String, DamageType>> {
        static TYPES: OnceLock<Mutex<BTreeMap<String, DamageType>>> = OnceLock::new();
        TYPES.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    pub(crate) fn with_types<T>(body: impl FnOnce(&mut BTreeMap<String, DamageType>) -> T) -> T {
        let mut guard = damage_types().lock().unwrap_or_else(|error| error.into_inner());
        body(&mut guard)
    }

    /// Registers a damage type.
    pub fn register_type(damage_type: DamageType) -> bool {
        with_types(|types| types.insert(damage_type.id.clone(), damage_type).is_none())
    }

    /// Forgets a damage type.
    pub fn unregister_type(id: &str) -> bool {
        with_types(|types| types.remove(id).is_some())
    }

    /// A registered damage type.
    #[must_use]
    pub fn get_type(id: &str) -> Option<DamageType> {
        with_types(|types| types.get(id).cloned())
    }

    /// Every registered damage type.
    #[must_use]
    pub fn get_types() -> Vec<DamageType> {
        with_types(|types| types.values().cloned().collect())
    }

    fn armor_is_bypassed(damage_type: &str) -> bool {
        get_type(damage_type).is_some_and(|kind| kind.bypasses_armor)
    }

    fn resistance_is_bypassed(damage_type: &str) -> bool {
        get_type(damage_type).is_some_and(|kind| kind.bypasses_resistance)
    }

    /// What a hit lands for, with everything applied.
    #[must_use]
    pub fn calculate(instance: &DamageInstance) -> f64 {
        instance.resolve()
    }

    /// The source of a hit.
    #[must_use]
    pub fn get_source(instance: &DamageInstance) -> DamageSource {
        instance.source.clone()
    }

    /// The amount before any reduction.
    #[must_use]
    pub fn get_amount(instance: &DamageInstance) -> f64 {
        instance.base()
    }

    /// What resistance takes off, as a fraction.
    #[must_use]
    pub fn get_resistance(instance: &DamageInstance) -> f64 {
        instance.resistance_reduction()
    }

    /// What armour takes off, as a fraction.
    #[must_use]
    pub fn get_armor_reduction(instance: &DamageInstance) -> f64 {
        instance.armor_reduction()
    }

    /// What enchantment protection takes off, as a fraction.
    #[must_use]
    pub fn get_enchantment_reduction(instance: &DamageInstance) -> f64 {
        instance.enchantment_reduction()
    }

    /// The cooldown multiplier.
    #[must_use]
    pub fn get_cooldown(instance: &DamageInstance) -> f64 {
        instance.cooldown_multiplier()
    }

    /// How much knockback the hit carries.
    #[must_use]
    pub fn get_knockback(instance: &DamageInstance) -> f64 {
        instance.knockback
    }

    /// Sets how much knockback the hit carries.
    pub fn set_knockback(instance: &mut DamageInstance, knockback: f64) {
        instance.knockback = knockback.max(0.0);
    }

    /// Cancels a hit, so it lands for nothing.
    pub fn cancel(instance: &mut DamageInstance) {
        instance.cancelled = true;
    }

    /// Reads a hit as landed, without changing it.
    #[must_use]
    pub fn resolve(instance: &DamageInstance) -> f64 {
        instance.resolve()
    }

    /// Registers a damage source, which is a name a mod can then use in its own books.
    pub fn register_source(source: DamageSource) -> bool {
        with_sources(|sources| sources.insert(source.id.clone(), source).is_none())
    }

    /// Forgets a damage source.
    pub fn unregister_source(id: &str) -> bool {
        with_sources(|sources| sources.remove(id).is_some())
    }

    /// A registered damage source.
    #[must_use]
    pub fn get_source_by_id(id: &str) -> Option<DamageSource> {
        with_sources(|sources| sources.get(id).cloned())
    }

    fn sources() -> &'static Mutex<BTreeMap<String, DamageSource>> {
        static SOURCES: OnceLock<Mutex<BTreeMap<String, DamageSource>>> = OnceLock::new();
        SOURCES.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    pub(crate) fn with_sources<T>(body: impl FnOnce(&mut BTreeMap<String, DamageSource>) -> T) -> T {
        let mut guard = sources().lock().unwrap_or_else(|error| error.into_inner());
        body(&mut guard)
    }

    /// Registers a damage modifier a mod applies itself.
    pub fn register_modifier(modifier: DamageModifier) -> bool {
        with_modifiers(|modifiers| {
            modifiers
                .insert(modifier.name.clone(), modifier)
                .is_none()
        })
    }

    /// Forgets a damage modifier.
    pub fn unregister_modifier(name: &str) -> bool {
        with_modifiers(|modifiers| modifiers.remove(name).is_some())
    }

    /// A registered damage modifier.
    #[must_use]
    pub fn get_modifier(name: &str) -> Option<DamageModifier> {
        with_modifiers(|modifiers| modifiers.get(name).cloned())
    }

    /// Every registered damage modifier.
    #[must_use]
    pub fn get_modifiers() -> Vec<DamageModifier> {
        with_modifiers(|modifiers| modifiers.values().cloned().collect())
    }

    fn modifiers() -> &'static Mutex<BTreeMap<String, DamageModifier>> {
        static MODIFIERS: OnceLock<Mutex<BTreeMap<String, DamageModifier>>> = OnceLock::new();
        MODIFIERS.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    pub(crate) fn with_modifiers<T>(body: impl FnOnce(&mut BTreeMap<String, DamageModifier>) -> T) -> T {
        let mut guard = modifiers().lock().unwrap_or_else(|error| error.into_inner());
        body(&mut guard)
    }

    /// Applies a hit to a player a mod owns.
    pub fn apply_to(player: &mut PlayerState, instance: &DamageInstance) -> f32 {
        let landing = instance.resolve() as f32;
        if landing > 0.0 {
            player.set_health(player.health - landing);
        }
        player.health
    }

    /// Applies a hit to a live entity. `false`: the server decides that.
    pub fn apply(_instance: &DamageInstance) -> bool {
        crate::warn!(
            "a mod asked this host to apply damage: {}",
            why_unknown("authority")
        );
        false
    }

    /// Changes the client's own damage calculation. `false`: that needs a hook on the client's
    /// damage path, which does not exist.
    pub fn modify_calculation(_name: &str) -> bool {
        crate::warn!(
            "a mod asked this host to change the client's damage calculation: {}",
            why_unknown("calculation")
        );
        false
    }
}

/// Why something in the effects half has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "authority" => {
            "applying an effect to a live entity is the server's, and this host cannot even \
             ask the client to send it"
                .to_owned()
        }
        "icon" => "an effect's icon is the client's asset, from its own effect definitions".to_owned(),
        "particles" => "effect particles are drawn by the client's renderer".to_owned(),
        "behavior" => {
            "an effect's behaviour would be called by the client as it ticks; nothing here \
             sees that"
                .to_owned()
        }
        "events" => {
            "these event kinds come from the client as it runs; they are registered for the \
             day the hooks land"
                .to_owned()
        }
        other => damage::why_unknown(other),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["authority", "calculation", "icon", "particles", "behavior", "events"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

pub use damage::{
    DamageInstance, DamageModifier, DamageModifierKind, DamageSource, DamageType,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_effects(|effects| effects.clear());
        damage::with_types(|types| types.clear());
        damage::with_sources(|sources| sources.clear());
        damage::with_modifiers(|modifiers| modifiers.clear());
    }

    #[test]
    fn an_effect_type_is_registered_and_read_back() {
        let _serial = serial();
        fresh();
        assert!(register(
            EffectType::new("hello:haste", "Haste")
                .beneficial()
                .color(0x11_ff88)
                .icon("hello:haste")
                .particles("minecraft:spark")
                .duration(3_000)
                .amplifier(1)
                .with_behavior()
        ));
        let registered = get_by_id("hello:haste").expect("registered");
        assert_eq!(registered.name, "Haste");
        assert_eq!(registered.category, EffectCategory::Beneficial);
        assert_eq!(registered.color, 0x11_ff88);
        assert_eq!(registered.icon, "hello:haste");
        assert_eq!(registered.particles, "minecraft:spark");
        assert!(registered.has_behavior);
        assert!(!registered.registered, "the client was never told");
        assert_eq!(
            get_by_name("Haste").map(|effect| effect.id),
            Some("hello:haste".to_owned())
        );
        assert_eq!(get_all().len(), 1);
        assert_eq!(registered.effect().level(), 2);
        assert!(
            !register(EffectType::new("hello:haste", "Haste again")),
            "it replaces"
        );
        assert!(get_by_name("Haste again").is_some());
        assert!(unregister("hello:haste"));
        assert!(!unregister("hello:haste"));
    }

    #[test]
    fn an_effect_is_applied_to_the_player_a_mod_owns() {
        let _serial = serial();
        fresh();
        register(EffectType::new("hello:haste", "Haste").duration(2_000).amplifier(1));
        let mut who = PlayerState::new("Tester");
        assert!(apply(&mut who, "hello:haste", 0, 0));
        assert!(has(&who, "hello:haste"));
        assert_eq!(get_duration(&who, "hello:haste"), Some(2_000), "the type's default");
        assert_eq!(get_amplifier(&who, "hello:haste"), Some(0));
        assert_eq!(get(&who, "hello:haste").map(|effect| effect.level()), Some(1));
        assert!(apply(&mut who, "hello:haste", 500, 2));
        assert_eq!(who.effects.len(), 1, "the second replaces the first");
        assert_eq!(get_duration(&who, "hello:haste"), Some(500));
        assert_eq!(get_amplifier(&who, "hello:haste"), Some(2));
        assert!(set_duration(&mut who, "hello:haste", 9_000));
        assert!(set_amplifier(&mut who, "hello:haste", 3));
        assert_eq!(get_duration(&who, "hello:haste"), Some(9_000));
        assert!(!set_duration(&mut who, "nothing", 1));
        assert!(apply(&mut who, "an-unregistered-one", 100, 0));
        assert!(has(&who, "an-unregistered-one"));
        assert!(remove(&mut who, "an-unregistered-one"));
        assert_eq!(clear(&mut who), 1);
        assert!(!has(&who, "hello:haste"));
    }

    #[test]
    fn the_registration_helpers_are_the_mods_own_data() {
        let _serial = serial();
        fresh();
        register(EffectType::new("hello:haste", "Haste"));
        assert!(register_behavior("hello:haste"));
        assert!(register_icon("hello:haste", "hello:haste_icon"));
        assert!(register_particles("hello:haste", "minecraft:spark"));
        let registered = get_by_id("hello:haste").expect("registered");
        assert!(registered.has_behavior);
        assert_eq!(registered.icon, "hello:haste_icon");
        assert_eq!(registered.particles, "minecraft:spark");
        assert!(!register_behavior("nothing"));
        assert!(!register_icon("nothing", "x"));
        assert!(!register_particles("nothing", "x"));
    }

    #[test]
    fn damage_is_calculated_the_way_the_game_does() {
        let _serial = serial();
        fresh();
        let plain = damage::DamageInstance::new(10.0, "minecraft:generic");
        assert_eq!(damage::calculate(&plain), 10.0, "nothing takes anything off");
        assert_eq!(damage::get_amount(&plain), 10.0);
        assert_eq!(damage::get_resistance(&plain), 0.0);
        assert_eq!(damage::get_cooldown(&plain), 1.0);
        assert_eq!(damage::get_knockback(&plain), 0.4);

        let armored = damage::DamageInstance::new(10.0, "minecraft:generic").with_armor(20.0);
        let reduction = damage::get_armor_reduction(&armored);
        assert!(reduction > 0.0 && reduction <= 0.8, "{reduction}");
        assert!(damage::calculate(&armored) < 10.0);
        assert_eq!(damage::calculate(&armored), 10.0 * (1.0 - reduction));

        let resistant = damage::DamageInstance::new(10.0, "minecraft:generic").with_resistance(2);
        assert_eq!(damage::get_resistance(&resistant), 0.4);
        assert_eq!(damage::calculate(&resistant), 6.0);
        let protected = damage::DamageInstance::new(10.0, "minecraft:generic").with_protection(4);
        assert_eq!(damage::get_enchantment_reduction(&protected), 0.16);
        assert_eq!(damage::calculate(&protected), 8.4);

        let quick =
            damage::DamageInstance::new(10.0, "minecraft:generic").with_cooldown(100);
        assert!(damage::get_cooldown(&quick) < 1.0);
        assert!(damage::calculate(&quick) < 10.0);
    }

    #[test]
    fn a_damage_type_decides_what_takes_anything_off() {
        let _serial = serial();
        fresh();
        damage::register_type(
            damage::DamageType::new("hello:void")
                .bypasses_armor()
                .bypasses_resistance()
                .fire(),
        );
        let kind = damage::get_type("hello:void").expect("registered");
        assert!(kind.bypasses_armor);
        assert!(kind.bypasses_resistance);
        assert!(kind.is_fire);
        assert_eq!(damage::get_types().len(), 1);

        let armored = damage::DamageInstance::new(10.0, "hello:void")
            .with_armor(20.0)
            .with_resistance(4);
        assert_eq!(damage::get_armor_reduction(&armored), 0.0, "it goes through armour");
        assert_eq!(damage::get_resistance(&armored), 0.0);
        assert_eq!(damage::calculate(&armored), 10.0);
        assert!(damage::unregister_type("hello:void"));
        assert!(!damage::unregister_type("hello:void"));
    }

    #[test]
    fn modifiers_and_sources_are_registries_a_mod_owns() {
        let _serial = serial();
        fresh();
        assert!(damage::register_modifier(damage::DamageModifier {
            name: "hello:sharp".to_owned(),
            kind: damage::DamageModifierKind::Multiply,
            amount: 0.5,
        }));
        assert_eq!(damage::get_modifiers().len(), 1);
        assert_eq!(
            damage::get_modifier("hello:sharp").map(|modifier| modifier.amount),
            Some(0.5)
        );
        assert!(damage::unregister_modifier("hello:sharp"));

        let instance = damage::DamageInstance::new(10.0, "minecraft:generic").with_modifier(
            damage::DamageModifier {
                name: "hello:sharp".to_owned(),
                kind: damage::DamageModifierKind::Multiply,
                amount: 0.5,
            },
        );
        assert_eq!(damage::get_amount(&instance), 15.0);
        assert_eq!(damage::calculate(&instance), 15.0);
        let added = instance.clone().with_modifier(damage::DamageModifier {
            name: "hello:band".to_owned(),
            kind: damage::DamageModifierKind::Add,
            amount: 5.0,
        });
        assert_eq!(damage::get_amount(&added), 22.5, "additive first, then the multiplier");

        let source = damage::DamageSource::new("hello:arrow")
            .by("Tester")
            .with_weapon("minecraft:bow");
        assert!(damage::register_source(source));
        assert_eq!(
            damage::get_source_by_id("hello:arrow").and_then(|source| source.attacker),
            Some("Tester".to_owned())
        );
        assert_eq!(
            damage::get_source(&instance),
            damage::DamageSource::new("minecraft:generic")
        );
        assert!(damage::unregister_source("hello:arrow"));
    }

    #[test]
    fn a_hit_can_be_cancelled_or_applied_to_a_player_a_mod_owns() {
        let _serial = serial();
        fresh();
        let mut who = PlayerState::new("Tester");
        let hit = damage::DamageInstance::new(7.0, "minecraft:generic");
        assert_eq!(damage::apply_to(&mut who, &hit), 13.0);
        let mut cancelled = hit.clone();
        damage::cancel(&mut cancelled);
        assert_eq!(damage::resolve(&cancelled), 0.0);
        assert_eq!(
            damage::apply_to(&mut who, &cancelled),
            13.0,
            "a cancelled hit does nothing"
        );
        let mut knock = hit.clone();
        damage::set_knockback(&mut knock, 1.5);
        assert_eq!(damage::get_knockback(&knock), 1.5);
        assert!(!damage::apply(&hit), "a live entity is the server's");
        assert!(!apply_live("hello:haste", 100, 0));
        assert!(!damage::modify_calculation("hello:damage"));
    }

    #[test]
    fn a_damage_instance_renders_for_a_report() {
        let hit = damage::DamageInstance::new(10.0, "minecraft:generic").with_armor(20.0);
        let text = hit.to_string();
        assert!(text.contains("minecraft:generic"), "{text}");
        assert!(text.contains("armour -"), "{text}");
        assert_eq!(EffectCategory::Harmful.as_str(), "harmful");
    }

    #[test]
    fn the_event_kinds_this_section_lists_are_registered() {
        let _serial = serial();
        assert!(register_event_kinds() > 0);
        assert!(has_event_kind("entity_damage"));
        assert!(has_event_kind("player_respawn"));
        assert!(!has_event_kind("nothing_at_all"));
        assert_eq!(event_kinds().len(), 33);
    }

    #[test]
    fn what_this_host_cannot_reach_says_so() {
        let _serial = serial();
        assert_eq!(refusals().len(), 6);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(!damage::why_unknown("authority").is_empty());
        assert!(!damage::why_unknown("calculation").is_empty());
    }
}
