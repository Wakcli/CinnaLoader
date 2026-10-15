//! Custom entities: an entity a mod declares in full, a live instance it owns and steps, and
//! the client's world, entity store and entity registry, refused.
//!
//! A declaration is real data — an id, a name, a hitbox, health, a movement speed, a model and
//! texture, attribute grants, components, data, behaviours, a factory, a renderer and a spawn
//! rule — and every mod can read it back. What is refused is everything about a *live* entity:
//! spawning one into the client's world, reading the client's entity store, and the client's
//! own entity registry. Each carries its reason in [`why_unknown`], and the refusing entry
//! point logs it and answers honestly rather than inventing an entity.
//!
//! The instance a mod owns is real: it falls under section 23's gravity through a
//! [`crate::physics::Body`], and section 22's damage arithmetic reduces its health, with death
//! answered rather than guessed.
//!
//! ```
//! use cinnaloader::custom_entities::{self, EntityDefinition};
//! use cinnaloader::effects::damage::DamageInstance;
//!
//! let wisp = EntityDefinition::new("wisp", "Wisp")
//!     .hitbox(0.5, 0.5)
//!     .health(8.0)
//!     .speed(0.3)
//!     .model("hello:wisp", "hello:textures/entity/wisp")
//!     .attribute("minecraft:movement", 0.3);
//! assert!(custom_entities::register_type(wisp));
//! assert_eq!(custom_entities::get_definition("wisp").map(|d| d.health), Some(8.0));
//!
//! let mut mob = custom_entities::spawn("wisp", (0.0, 10.0, 0.0)).expect("declared");
//! mob.step(1.0);
//! assert!(mob.position().y < 10.0, "gravity pulls it down");
//!
//! let landed = mob.hurt(&DamageInstance::new(3.0, "minecraft:generic"));
//! assert_eq!(landed, 3.0, "nothing takes anything off a plain hit");
//! assert!(mob.is_alive());
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::effects::damage::{self, DamageInstance};
use crate::math::Vec3;
use crate::physics::Body;

/// Why a question about the client's entities has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "world" => {
            "the client's world is the only place an entity is simulated, and this host cannot \
             read it"
                .to_owned()
        }
        "store" => {
            "the client's entity store is the only place a live entity exists; reading it needs \
             the hooks section 10 lists as never"
                .to_owned()
        }
        "registry" => {
            "the client's entity registry is built from its own resources at start-up; this host \
             cannot read it and cannot add to it"
                .to_owned()
        }
        "spawn" => {
            "spawning into the client's world is the server's, and this host cannot even ask the \
             client to send it"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about the client's entities.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["world", "store", "registry", "spawn"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about the client's entities: {}", why_unknown(what));
    false
}

/// A hitbox, in blocks.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Hitbox {
    pub width: f32,
    pub height: f32,
}

impl Hitbox {
    #[must_use]
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }

    /// Whether the hitbox is a real box, which an empty id with a zero box is not.
    #[must_use]
    pub fn is_positive(&self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }
}

/// The model and texture an entity draws with.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelRef {
    pub model: String,
    pub texture: String,
}

impl ModelRef {
    #[must_use]
    pub fn new(model: &str, texture: &str) -> Self {
        Self {
            model: model.to_owned(),
            texture: texture.to_owned(),
        }
    }
}

/// An entity type a mod declares in full.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityDefinition {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub hitbox: Hitbox,
    pub health: f32,
    pub speed: f32,
    pub model: Option<ModelRef>,
    pub attributes: BTreeMap<String, f32>,
    pub components: BTreeMap<String, String>,
    pub data: BTreeMap<String, String>,
    pub behaviors: Vec<String>,
    pub factory: Option<String>,
    pub renderer: Option<String>,
    pub spawn_rule: Option<String>,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered_with_client: bool,
}

impl EntityDefinition {
    /// An entity whose id and name are given and whose every other field is a sane default.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            hitbox: Hitbox::new(0.6, 1.8),
            health: 20.0,
            speed: 0.25,
            model: None,
            attributes: BTreeMap::new(),
            components: BTreeMap::new(),
            data: BTreeMap::new(),
            behaviors: Vec::new(),
            factory: None,
            renderer: None,
            spawn_rule: None,
            registered_with_client: false,
        }
    }

    #[must_use]
    pub fn hitbox(mut self, width: f32, height: f32) -> Self {
        self.hitbox = Hitbox::new(width, height);
        self
    }

    #[must_use]
    pub fn health(mut self, health: f32) -> Self {
        self.health = health;
        self
    }

    #[must_use]
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    #[must_use]
    pub fn model(mut self, model: &str, texture: &str) -> Self {
        self.model = Some(ModelRef::new(model, texture));
        self
    }

    #[must_use]
    pub fn attribute(mut self, name: &str, value: f32) -> Self {
        self.attributes.insert(name.to_owned(), value);
        self
    }

    #[must_use]
    pub fn component(mut self, name: &str, value: &str) -> Self {
        self.components.insert(name.to_owned(), value.to_owned());
        self
    }

    #[must_use]
    pub fn data(mut self, name: &str, value: &str) -> Self {
        self.data.insert(name.to_owned(), value.to_owned());
        self
    }

    #[must_use]
    pub fn behavior(mut self, name: &str) -> Self {
        self.behaviors.push(name.to_owned());
        self
    }

    #[must_use]
    pub fn factory(mut self, name: &str) -> Self {
        self.factory = Some(name.to_owned());
        self
    }

    #[must_use]
    pub fn renderer(mut self, name: &str) -> Self {
        self.renderer = Some(name.to_owned());
        self
    }

    #[must_use]
    pub fn spawn_rule(mut self, name: &str) -> Self {
        self.spawn_rule = Some(name.to_owned());
        self
    }

    /// The registry id, with the owning mod in front.
    #[must_use]
    pub fn registry_id(&self) -> String {
        format!("{}:{}", self.owner, self.id)
    }
}

impl fmt::Display for EntityDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}x{}, {} health, {} speed)",
            self.name, self.id, self.hitbox.width, self.hitbox.height, self.health, self.speed
        )
    }
}

/// Every reason a declaration is not one this host would accept. Empty means valid.
#[must_use]
pub fn validate_definition(definition: &EntityDefinition) -> Vec<String> {
    let mut problems = Vec::new();
    if definition.id.trim().is_empty() {
        problems.push("the id is empty".to_owned());
    }
    if !definition.hitbox.is_positive() {
        problems.push(format!(
            "the hitbox is not positive ({}x{})",
            definition.hitbox.width, definition.hitbox.height
        ));
    }
    if definition.health < 0.0 {
        problems.push("the health is negative".to_owned());
    }
    if let Some(model) = &definition.model {
        if model.model.trim().is_empty() {
            problems.push("the model id is empty".to_owned());
        }
    }
    problems
}

fn registry() -> &'static Mutex<BTreeMap<String, EntityDefinition>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, EntityDefinition>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, EntityDefinition>) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares an entity type, refusing a declaration with an empty id or a non-positive hitbox.
pub fn register_type(definition: EntityDefinition) -> bool {
    let problems = validate_definition(&definition);
    if !problems.is_empty() {
        crate::warn!(
            "{} refused the entity {}: {}",
            crate::metadata::id(),
            definition.id,
            problems.join("; ")
        );
        return false;
    }
    with(|registry| registry.insert(definition.id.clone(), definition).is_none())
}

/// Forgets a declared entity type.
pub fn unregister_type(id: &str) -> bool {
    with(|registry| registry.remove(id).is_some())
}

/// Re-validates and replaces a declaration, leaving the old one in place when it is refused.
pub fn reload_definition(definition: EntityDefinition) -> Result<EntityDefinition, Vec<String>> {
    let problems = validate_definition(&definition);
    if !problems.is_empty() {
        return Err(problems);
    }
    with(|registry| {
        registry.insert(definition.id.clone(), definition.clone());
    });
    Ok(definition)
}

/// A declared entity type, by id.
#[must_use]
pub fn get_definition(id: &str) -> Option<EntityDefinition> {
    with(|registry| registry.get(id).cloned())
}

/// Every declared entity type, in id order.
#[must_use]
pub fn get_definitions() -> Vec<EntityDefinition> {
    with(|registry| registry.values().cloned().collect())
}

/// Every declared entity type, grouped by the mod that declared it.
#[must_use]
pub fn registry_by_mod() -> Vec<(String, Vec<String>)> {
    with(|registry| {
        let mut by_mod: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for definition in registry.values() {
            by_mod
                .entry(definition.owner.clone())
                .or_default()
                .push(definition.id.clone());
        }
        by_mod.into_iter().collect()
    })
}

fn mutate(id: &str, change: impl FnOnce(&mut EntityDefinition)) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(definition) => {
            change(definition);
            true
        }
        None => false,
    })
}

/// Records the factory an entity is built with.
pub fn register_factory(id: &str, name: &str) -> bool {
    mutate(id, |definition| definition.factory = Some(name.to_owned()))
}

/// Records the renderer an entity draws with.
pub fn register_renderer(id: &str, name: &str) -> bool {
    mutate(id, |definition| definition.renderer = Some(name.to_owned()))
}

/// Records an entity's model and texture.
pub fn register_model(id: &str, model: ModelRef) -> bool {
    mutate(id, |definition| definition.model = Some(model))
}

/// Records the rule that decides where an entity may spawn.
pub fn register_spawn_rule(id: &str, name: &str) -> bool {
    mutate(id, |definition| definition.spawn_rule = Some(name.to_owned()))
}

/// Grants an entity attributes.
pub fn register_attributes(id: &str, attributes: &[(&str, f32)]) -> bool {
    mutate(id, |definition| {
        for (name, value) in attributes {
            definition.attributes.insert((*name).to_owned(), *value);
        }
    })
}

/// Adds components to an entity.
pub fn register_components(id: &str, components: &[(&str, &str)]) -> bool {
    mutate(id, |definition| {
        for (name, value) in components {
            definition.components.insert((*name).to_owned(), (*value).to_owned());
        }
    })
}

/// Adds one entry to an entity's data.
pub fn register_data(id: &str, name: &str, value: &str) -> bool {
    mutate(id, |definition| {
        definition.data.insert(name.to_owned(), value.to_owned());
    })
}

/// Adds behaviours to an entity.
pub fn register_behaviors(id: &str, behaviors: &[&str]) -> bool {
    mutate(id, |definition| {
        for behavior in behaviors {
            definition.behaviors.push((*behavior).to_owned());
        }
    })
}

/// A live entity a mod owns: a physics body, health and the state a caller reads back.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityInstance {
    pub definition: String,
    pub body: Body,
    pub health: f32,
    pub max_health: f32,
    /// Armour points the damage arithmetic applies.
    pub armor: f64,
    pub dead: bool,
    pub ticks: u64,
}

impl EntityInstance {
    #[must_use]
    pub fn position(&self) -> Vec3 {
        let (x, y, z) = self.body.position;
        Vec3::new(x, y, z)
    }

    #[must_use]
    pub fn velocity(&self) -> Vec3 {
        let (x, y, z) = self.body.velocity;
        Vec3::new(x, y, z)
    }

    pub fn set_velocity(&mut self, velocity: Vec3) {
        self.body.set_velocity(velocity.x, velocity.y, velocity.z);
    }

    /// One tick of movement through section 23's body: gravity, drag, then the position.
    pub fn step(&mut self, dt: f64) {
        self.body.step(dt);
        self.ticks += 1;
    }

    /// Applies section 22's damage arithmetic and takes the result off the health. Answers what
    /// actually landed; death follows from the health reaching zero.
    pub fn hurt(&mut self, hit: &DamageInstance) -> f64 {
        if self.dead {
            return 0.0;
        }
        let landed = damage::calculate(hit);
        self.health = (self.health - landed as f32).max(0.0);
        if self.health <= 0.0 {
            self.dead = true;
        }
        landed
    }

    /// The same, built with this instance's own armour.
    pub fn take_damage(&mut self, amount: f64, damage_type: &str) -> f64 {
        self.hurt(&DamageInstance::new(amount, damage_type).with_armor(self.armor))
    }

    pub fn kill(&mut self) {
        self.health = 0.0;
        self.dead = true;
    }

    #[must_use]
    pub fn health(&self) -> f32 {
        self.health
    }

    #[must_use]
    pub fn max_health(&self) -> f32 {
        self.max_health
    }

    #[must_use]
    pub fn is_alive(&self) -> bool {
        !self.dead && self.health > 0.0
    }

    #[must_use]
    pub fn is_dead(&self) -> bool {
        !self.is_alive()
    }
}

/// Builds a fresh instance of a declared entity, at rest and in the air with full health.
#[must_use]
pub fn get_defaults(id: &str) -> Option<EntityInstance> {
    let definition = get_definition(id)?;
    let mut body = Body::new((0.0, 0.0, 0.0)).with_size(
        f64::from(definition.hitbox.width),
        f64::from(definition.hitbox.height),
    );
    body.on_ground = false;
    Some(EntityInstance {
        definition: definition.id,
        body,
        health: definition.health,
        max_health: definition.health,
        armor: f64::from(definition.attributes.get("minecraft:armor").copied().unwrap_or(0.0)),
        dead: false,
        ticks: 0,
    })
}

/// Spawns an instance of a declared entity where it is put. `None` when nothing is declared
/// under that id.
#[must_use]
pub fn spawn(id: &str, at: (f64, f64, f64)) -> Option<EntityInstance> {
    let mut instance = get_defaults(id)?;
    instance.body.position = at;
    Some(instance)
}

/// Whether the client has been told about a declared entity, which it never has.
#[must_use]
pub fn is_registered_with_the_client(id: &str) -> bool {
    get_definition(id).is_some_and(|definition| definition.registered_with_client)
}

/// The live half: the client's world, entity store and entity registry, each refused with its
/// reason rather than faked.
pub mod live {
    use super::Vec3;

    /// Spawns into the client's world. `false`.
    pub fn spawn(_id: &str, _at: Vec3) -> bool {
        super::refused("spawn")
    }

    /// Removes from the client's entity store. `false`.
    pub fn despawn(_id: &str) -> bool {
        super::refused("store")
    }

    /// A live entity by id. `None`.
    #[must_use]
    pub fn get(_id: &str) -> Option<()> {
        None
    }

    /// A live entity's position. `None`.
    #[must_use]
    pub fn get_position(_id: &str) -> Option<Vec3> {
        None
    }

    /// A live entity's health. `None`.
    #[must_use]
    pub fn get_health(_id: &str) -> Option<f32> {
        None
    }

    /// Every live entity. Empty.
    #[must_use]
    pub fn all() -> Vec<String> {
        Vec::new()
    }

    /// The client's own entity registry. Empty.
    #[must_use]
    pub fn registry() -> Vec<String> {
        Vec::new()
    }

    /// The client's registry id for a kind. `None`.
    #[must_use]
    pub fn get_registry_id(_id: &str) -> Option<String> {
        None
    }
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

    fn wisp() -> EntityDefinition {
        EntityDefinition::new("wisp", "Wisp")
            .hitbox(0.5, 0.5)
            .health(8.0)
            .speed(0.3)
            .model("hello:wisp", "hello:textures/entity/wisp")
            .attribute("minecraft:movement", 0.3)
            .attribute("minecraft:armor", 2.0)
            .component("minecraft:health", "8")
            .data("hello:mood", "calm")
            .behavior("hello:drift")
            .factory("hello:wisp_factory")
            .renderer("hello:wisp_renderer")
            .spawn_rule("hello:caves")
    }

    #[test]
    fn a_declared_entity_can_be_read_back() {
        let _serial = serial();
        fresh();
        assert!(register_type(wisp()));
        let declared = get_definition("wisp").expect("declared");
        assert_eq!(declared.name, "Wisp");
        assert_eq!(declared.hitbox, Hitbox::new(0.5, 0.5));
        assert_eq!(declared.health, 8.0);
        assert_eq!(declared.speed, 0.3);
        assert_eq!(
            declared.model,
            Some(ModelRef::new("hello:wisp", "hello:textures/entity/wisp"))
        );
        assert_eq!(declared.attributes.get("minecraft:movement"), Some(&0.3));
        assert!(declared.registry_id().ends_with(":wisp"));
        assert_eq!(get_definitions().len(), 1);
        assert!(!is_registered_with_the_client("wisp"));

        let by_mod = registry_by_mod();
        assert_eq!(by_mod.len(), 1);
        assert!(by_mod[0].1.contains(&"wisp".to_owned()));

        assert!(register_data("wisp", "hello:mood", "wary"));
        assert!(register_behaviors("wisp", &["hello:glow"]));
        assert!(!register_factory("nothing", "hello:factory"));
        let changed = get_definition("wisp").expect("declared");
        assert_eq!(changed.data.get("hello:mood"), Some(&"wary".to_owned()));
        assert_eq!(changed.behaviors.len(), 2);
        assert!(unregister_type("wisp"));
        assert!(!unregister_type("wisp"));
        assert!(get_definition("wisp").is_none());
    }

    #[test]
    fn an_invalid_definition_is_refused() {
        let _serial = serial();
        fresh();
        assert!(validate_definition(&wisp()).is_empty());
        assert!(register_type(wisp()));

        let nameless = EntityDefinition::new("", "Nameless").hitbox(1.0, 1.0);
        let problems = validate_definition(&nameless);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("id is empty"), "{}", problems[0]);
        assert!(!register_type(nameless));
        assert_eq!(get_definitions().len(), 1, "the good one is untouched");

        let flat = EntityDefinition::new("flat", "Flat").hitbox(0.0, 1.0);
        let problems = validate_definition(&flat);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("hitbox is not positive"), "{}", problems[0]);
        assert!(!register_type(flat));

        let hurting = EntityDefinition::new("hurt", "Hurt").hitbox(0.5, 0.5).health(-1.0);
        assert!(!register_type(hurting));
        assert!(get_definition("hurt").is_none());
    }

    #[test]
    fn an_instance_falls_under_section_23_gravity() {
        let _serial = serial();
        fresh();
        register_type(wisp());
        let mut mob = spawn("wisp", (0.0, 10.0, 0.0)).expect("declared");
        assert_eq!(mob.position(), Vec3::new(0.0, 10.0, 0.0));
        assert_eq!(mob.velocity(), Vec3::ZERO);
        assert!(mob.is_alive());
        assert_eq!(mob.health(), 8.0);
        assert_eq!(mob.max_health(), 8.0);
        mob.step(1.0);
        assert!(mob.velocity().y < 0.0, "gravity pulls it down");
        assert_eq!(
            mob.velocity().y,
            -crate::physics::GRAVITY * crate::physics::VERTICAL_DRAG,
            "one tick of gravity, then section 23's drag"
        );
        assert!(mob.position().y < 10.0);
        assert_eq!(mob.ticks, 1);
        assert!(spawn("nothing", (0.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn an_instance_takes_exact_damage_and_dies_at_zero() {
        let _serial = serial();
        fresh();
        register_type(wisp());
        let mut mob = spawn("wisp", (0.0, 64.0, 0.0)).expect("declared");
        assert_eq!(mob.health(), 8.0);
        let landed = mob.hurt(&DamageInstance::new(3.0, "minecraft:generic"));
        assert_eq!(landed, 3.0, "nothing takes anything off a plain hit");
        assert_eq!(mob.health(), 5.0);
        assert!(mob.is_alive());

        let landed = mob.take_damage(5.0, "minecraft:generic");
        assert!(landed < 5.0, "armour takes some off: {landed}");
        assert!(landed > 4.0, "but not all of it: {landed}");
        assert!(mob.is_alive(), "{}", mob.health());

        let big = mob.take_damage(100.0, "minecraft:generic");
        assert!((big - 84.0).abs() < 1e-9, "armour 2 takes 16% off: {big}");
        assert!(mob.is_dead());
        assert_eq!(mob.health(), 0.0);
        assert_eq!(mob.hurt(&DamageInstance::new(1.0, "minecraft:generic")), 0.0, "already dead");
        mob.kill();
        assert!(mob.is_dead());
    }

    #[test]
    fn reloading_a_definition_validates_and_replaces() {
        let _serial = serial();
        fresh();
        register_type(wisp());
        let bigger = EntityDefinition::new("wisp", "Wisp").hitbox(0.5, 0.5).health(20.0);
        let reloaded = reload_definition(bigger).expect("valid");
        assert_eq!(reloaded.health, 20.0);
        assert_eq!(get_definition("wisp").map(|d| d.health), Some(20.0));

        let bad = EntityDefinition::new("wisp", "Wisp").hitbox(-1.0, 0.5);
        let problems = reload_definition(bad).expect_err("refused");
        assert!(!problems.is_empty());
        assert_eq!(get_definition("wisp").map(|d| d.health), Some(20.0), "old one kept");
    }

    #[test]
    fn the_clients_world_and_registry_are_refused_with_a_reason() {
        let _serial = serial();
        fresh();
        register_type(wisp());
        assert!(!live::spawn("wisp", Vec3::new(0.0, 64.0, 0.0)));
        assert!(!live::despawn("wisp"));
        assert!(live::get("wisp").is_none());
        assert!(live::get_position("wisp").is_none());
        assert!(live::get_health("wisp").is_none());
        assert!(live::get_registry_id("wisp").is_none());
        assert!(live::all().is_empty());
        assert!(live::registry().is_empty());
        assert!(!is_registered_with_the_client("wisp"));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn a_declaration_renders_for_a_report() {
        let text = wisp().to_string();
        assert!(text.contains("Wisp"), "{text}");
        assert!(text.contains("wisp"), "{text}");
        assert!(text.contains("8 health"), "{text}");
    }
}
