//! Entities: the types a mod declares, and the client's entity world, which is out of reach.
//!
//! Every question about a *live* entity — where it is, how much health it has, whether it is
//! sneaking, whether it is a player — is the client's entity store, and section 10 lists the
//! capability that would read it as `never`. Spawning, damaging, teleporting, mounting and
//! the rest are server-authoritative besides: the client can only ask, and this host has no
//! way to ask, so they are refused with the reason instead of being faked.
//!
//! What a mod can do here is **declare an entity type**: its identifier, family, size,
//! health, movement, attributes, components and tags. That declaration is real data every mod
//! can read, and [`is_registered_with_the_client`] is honest that the client has not been
//! told. It is the same shape as the blocks and items sections, and it is what a mod needs
//! before there is a hook to hand it to the client.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! entities::register_type(EntityType::new("wisp", "Wisp")
//!     .family("mob")
//!     .health(8.0)
//!     .size(0.5, 0.5)
//!     .attribute("minecraft:movement", 0.3));
//! assert_eq!(entities::get_max_health("wisp"), Some(8.0));
//! assert!(entities::live_entities::get().is_none(), "a live entity is the client's");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events;

/// The entity events this host would have to receive from the client to fire.
pub const ENTITY_EVENTS: [&str; 18] = [
    "entity_spawn",
    "entity_despawn",
    "entity_death",
    "entity_damage",
    "entity_heal",
    "entity_move",
    "entity_teleport",
    "entity_interact",
    "entity_attack",
    "entity_mount",
    "entity_dismount",
    "entity_target",
    "entity_equip",
    "entity_tick",
    "entity_load",
    "entity_save",
    "entity_component_change",
    "entity_attribute_change",
];

/// Teaches the event bus each entity event name, so a subscription is in place before the
/// hooks land.
pub fn register_event_kinds() -> usize {
    ENTITY_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// The event kinds this host would need from the client to fire an entity callback.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    ENTITY_EVENTS.to_vec()
}

/// Whether an entity event kind is one the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    ENTITY_EVENTS.contains(&kind) && events::is_registered_kind(kind)
}

/// Why an entity question about the client's world has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "lookup" => {
            "the client's entity store is the only place an entity exists; reading it needs \
             the hooks section 10 lists as never"
                .to_owned()
        }
        "position" => "an entity's position comes from the client's own interpolation".to_owned(),
        "movement" => "velocity and rotation are the client's; setting them is a packet to the server".to_owned(),
        "state" => "sneaking, sprinting, burning and the rest are flags the client applies".to_owned(),
        "health" => "health arrives from the server and is kept by the client".to_owned(),
        "attribute" => "attributes are synced to the client, which this host cannot read".to_owned(),
        "component" => {
            "the client exposes metadata and flags rather than arbitrary components; reading \
             them needs a hook"
                .to_owned()
        }
        "tag" => "entity families are server data, not something a client is told".to_owned(),
        "ownership" => {
            "owners, riders and targets are not synced to a Bedrock client in general; the \
             local mount is, through the client's own seat handling"
                .to_owned()
        }
        "change" => {
            "damage, healing, spawning and removal are the server's, and this host cannot \
             even ask the client to send them"
                .to_owned()
        }
        "presentation" => "a name tag or glowing outline is the client's presentation layer".to_owned(),
        "events" => {
            "entity events come from the client as it runs; nothing here sees them, so they \
             are registered for the day the hooks land"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What an entity looks like, so a mod can reason about one it declared.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntitySize {
    pub width: f32,
    pub height: f32,
}

/// A component a declared entity carries, as Bedrock spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityComponent {
    pub name: String,
    pub value: String,
}

impl EntityComponent {
    #[must_use]
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }
}

/// An entity type a mod declares.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityType {
    pub id: String,
    pub name: String,
    pub owner: String,
    /// The family or families it belongs to, which is what Bedrock calls tags.
    pub families: Vec<String>,
    pub size: EntitySize,
    pub health: f32,
    pub movement: f32,
    pub attributes: BTreeMap<String, f32>,
    pub components: Vec<EntityComponent>,
    pub tags: Vec<String>,
    /// Whether it can be ridden, and whether it can be a passenger.
    pub rideable: bool,
    /// Whether it is hostile, which a mod says about its own entity.
    pub hostile: bool,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered: bool,
}

impl EntityType {
    /// An entity with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            families: Vec::new(),
            size: EntitySize {
                width: 0.6,
                height: 1.8,
            },
            health: 20.0,
            movement: 0.25,
            attributes: BTreeMap::new(),
            components: Vec::new(),
            tags: Vec::new(),
            rideable: false,
            hostile: false,
            registered: false,
        }
    }

    #[must_use]
    pub fn family(mut self, family: &str) -> Self {
        self.families.push(family.to_owned());
        self
    }

    #[must_use]
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = EntitySize { width, height };
        self
    }

    #[must_use]
    pub fn health(mut self, health: f32) -> Self {
        self.health = health.max(0.0);
        self
    }

    #[must_use]
    pub fn movement(mut self, movement: f32) -> Self {
        self.movement = movement.max(0.0);
        self
    }

    #[must_use]
    pub fn attribute(mut self, name: &str, value: f32) -> Self {
        self.attributes.insert(name.to_owned(), value);
        self
    }

    #[must_use]
    pub fn component(mut self, component: EntityComponent) -> Self {
        self.components.retain(|held| held.name != component.name);
        self.components.push(component);
        self
    }

    #[must_use]
    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_owned());
        self
    }

    #[must_use]
    pub fn rideable(mut self, rideable: bool) -> Self {
        self.rideable = rideable;
        self
    }

    #[must_use]
    pub fn hostile(mut self, hostile: bool) -> Self {
        self.hostile = hostile;
        self
    }

    /// The bounding box it would have at a position, which is what a mod asks for before a
    /// hook exists.
    #[must_use]
    pub fn bounding_box_at(&self, x: f64, y: f64, z: f64) -> (f64, f64, f64, f64, f64, f64) {
        let half = f64::from(self.size.width) / 2.0;
        (
            x - half,
            y,
            z - half,
            x + half,
            y + f64::from(self.size.height),
            z + half,
        )
    }
}

impl fmt::Display for EntityType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}x{}, {} health{})",
            self.name,
            self.id,
            self.size.width,
            self.size.height,
            self.health,
            if self.hostile { ", hostile" } else { "" }
        )
    }
}

fn registry() -> &'static Mutex<BTreeMap<String, EntityType>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, EntityType>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, EntityType>) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares an entity type. The declaration is real; the client is not told, and
/// [`is_registered_with_the_client`] says so.
pub fn register_type(entity: EntityType) -> bool {
    crate::warn!(
        "{} declared the entity {}: this host records it, but {}",
        crate::metadata::id(),
        entity.id,
        why_unknown("lookup")
    );
    with(|registry| registry.insert(entity.id.clone(), entity).is_none())
}

/// Forgets a declared entity type.
pub fn unregister_type(id: &str) -> bool {
    with(|registry| registry.remove(id).is_some())
}

/// Declares a component on an entity that is already declared.
pub fn register_component(id: &str, component: EntityComponent) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(entity) => {
            entity.components.retain(|held| held.name != component.name);
            entity.components.push(component);
            true
        }
        None => false,
    })
}

/// A declared entity type, by id.
#[must_use]
pub fn get_by_id(id: &str) -> Option<EntityType> {
    with(|registry| registry.get(id).cloned())
}

/// A declared entity type, by name.
#[must_use]
pub fn get_by_name(name: &str) -> Option<EntityType> {
    with(|registry| {
        registry
            .values()
            .find(|entity| entity.name == name)
            .cloned()
    })
}

/// Every declared entity type, in id order.
#[must_use]
pub fn get_all() -> Vec<EntityType> {
    with(|registry| registry.values().cloned().collect())
}

/// A declared entity's registry id, with the owning mod in front.
#[must_use]
pub fn get_registry_id(id: &str) -> Option<String> {
    get_by_id(id).map(|entity| format!("{}:{}", entity.owner, entity.id))
}

/// A declared entity's maximum health.
#[must_use]
pub fn get_max_health(id: &str) -> Option<f32> {
    get_by_id(id).map(|entity| entity.health)
}

/// A declared entity's attributes.
#[must_use]
pub fn get_attributes(id: &str) -> BTreeMap<String, f32> {
    get_by_id(id)
        .map(|entity| entity.attributes)
        .unwrap_or_default()
}

/// One declared attribute.
#[must_use]
pub fn get_attribute(id: &str, name: &str) -> Option<f32> {
    get_by_id(id).and_then(|entity| entity.attributes.get(name).copied())
}

/// A declared entity's families.
#[must_use]
pub fn get_tags(id: &str) -> Vec<String> {
    get_by_id(id).map(|entity| entity.families).unwrap_or_default()
}

/// A declared entity's components.
#[must_use]
pub fn get_components(id: &str) -> Vec<EntityComponent> {
    get_by_id(id).map(|entity| entity.components).unwrap_or_default()
}

/// One declared component.
#[must_use]
pub fn get_component(id: &str, name: &str) -> Option<EntityComponent> {
    get_by_id(id).and_then(|entity| {
        entity
            .components
            .into_iter()
            .find(|component| component.name == name)
    })
}

/// Whether a declared entity has a component.
#[must_use]
pub fn has_component(id: &str, name: &str) -> bool {
    get_component(id, name).is_some()
}

/// Whether a declared entity is a player's, which only a player entity would be.
#[must_use]
pub fn is_player(id: &str) -> Option<bool> {
    get_by_id(id).map(|entity| entity.families.iter().any(|family| family == "player"))
}

/// Whether a declared entity is living.
#[must_use]
pub fn is_living(id: &str) -> Option<bool> {
    get_by_id(id).map(|entity| {
        entity.health > 0.0
            && !entity
                .families
                .iter()
                .any(|family| family == "inanimate" || family == "item")
    })
}

/// Whether a declared entity is hostile, which is what the mod said about it.
#[must_use]
pub fn is_hostile(id: &str) -> Option<bool> {
    get_by_id(id).map(|entity| entity.hostile)
}

/// Whether a declared entity is passive.
#[must_use]
pub fn is_passive(id: &str) -> Option<bool> {
    get_by_id(id).map(|entity| !entity.hostile)
}

/// Whether a declared entity can be ridden.
#[must_use]
pub fn is_rideable(id: &str) -> Option<bool> {
    get_by_id(id).map(|entity| entity.rideable)
}

/// A declared entity's size.
#[must_use]
pub fn get_dimensions(id: &str) -> Option<EntitySize> {
    get_by_id(id).map(|entity| entity.size)
}

/// A declared entity's eye height, which is its own height less the default eye offset.
#[must_use]
pub fn get_eye_height(id: &str) -> Option<f32> {
    get_by_id(id).map(|entity| (entity.size.height - 0.2).max(0.0))
}

/// The bounding box a declared entity would have at a position.
#[must_use]
pub fn get_bounding_box(id: &str, x: f64, y: f64, z: f64) -> Option<(f64, f64, f64, f64, f64, f64)> {
    get_by_id(id).map(|entity| entity.bounding_box_at(x, y, z))
}

/// Whether the client has been told about a declared entity, which it never has.
#[must_use]
pub fn is_registered_with_the_client(id: &str) -> bool {
    get_by_id(id).is_some_and(|entity| entity.registered)
}

/// How far apart two positions are, which is the one piece of entity arithmetic that is
/// pure.
#[must_use]
pub fn distance(from: (f64, f64, f64), to: (f64, f64, f64)) -> f64 {
    distance_squared(from, to).sqrt()
}

/// The same without the square root, for a caller comparing distances.
#[must_use]
pub fn distance_squared(from: (f64, f64, f64), to: (f64, f64, f64)) -> f64 {
    let (dx, dy, dz) = (to.0 - from.0, to.1 - from.1, to.2 - from.2);
    dx * dx + dy * dy + dz * dz
}

/// What this host cannot reach about entities, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "lookup",
        "position",
        "movement",
        "state",
        "health",
        "attribute",
        "component",
        "tag",
        "ownership",
        "change",
        "presentation",
        "events",
    ]
    .iter()
    .map(|what| ((*what).to_owned(), why_unknown(what)))
    .collect()
}

fn live(what: &str) -> bool {
    crate::warn!("a mod asked this host about a live entity: {}", why_unknown(what));
    false
}

/// A handle to a live entity. `None`, always: [`why_unknown`] says why.
pub type EntityHandle = ();

/// An entity, which only the client has. `None`, under the name the wishlist uses.
#[must_use]
pub fn get() -> Option<EntityHandle> {
    None
}

/// The live-entity half: every one of these is the client's entity store or the server's
/// authority, and answers honestly rather than inventing an entity.
pub mod live_entities {
    /// An entity by handle. `None`.
    #[must_use]
    pub fn get() -> Option<super::EntityHandle> {
        None
    }

    /// An entity by its unique id. `None`.
    #[must_use]
    pub fn get_by_uuid(_uuid: &str) -> Option<super::EntityHandle> {
        None
    }

    /// An entity's type. `None`.
    #[must_use]
    pub fn get_type() -> Option<String> {
        None
    }

    /// An entity's registry id. `None`.
    #[must_use]
    pub fn get_registry_id() -> Option<String> {
        None
    }

    /// An entity's unique id. `None`.
    #[must_use]
    pub fn get_uuid() -> Option<String> {
        None
    }

    /// An entity's runtime id, which only the client knows. `None`.
    #[must_use]
    pub fn get_network_id() -> Option<u64> {
        None
    }

    /// Where an entity is. `None`.
    #[must_use]
    pub fn get_position() -> Option<(f64, f64, f64)> {
        None
    }

    /// Where it was a tick ago. `None`.
    #[must_use]
    pub fn get_previous_position() -> Option<(f64, f64, f64)> {
        None
    }

    /// Where it is looking. `None`.
    #[must_use]
    pub fn get_rotation() -> Option<(f32, f32)> {
        None
    }

    /// What it is standing on. `None`: the world is the client's.
    #[must_use]
    pub fn get_world() -> Option<String> {
        None
    }

    /// Which dimension it is in. `None`.
    #[must_use]
    pub fn get_dimension() -> Option<String> {
        None
    }

    /// How it is moving. `None`.
    #[must_use]
    pub fn get_velocity() -> Option<(f64, f64, f64)> {
        None
    }

    /// How much health it has. `None`.
    #[must_use]
    pub fn get_health() -> Option<f32> {
        None
    }

    /// Whether it is alive. `None`.
    #[must_use]
    pub fn is_alive() -> Option<bool> {
        None
    }

    /// Whether it is dead. `None`.
    #[must_use]
    pub fn is_dead() -> Option<bool> {
        None
    }

    /// Whether it is on the ground. `None`.
    #[must_use]
    pub fn is_on_ground() -> Option<bool> {
        None
    }

    /// Whether it is in water. `None`.
    #[must_use]
    pub fn is_in_water() -> Option<bool> {
        None
    }

    /// Whether it is burning. `None`.
    #[must_use]
    pub fn is_burning() -> Option<bool> {
        None
    }

    /// Whether it is invisible. `None`.
    #[must_use]
    pub fn is_invisible() -> Option<bool> {
        None
    }

    /// Whether it is sneaking. `None`.
    #[must_use]
    pub fn is_sneaking() -> Option<bool> {
        None
    }

    /// Whether it is sprinting. `None`.
    #[must_use]
    pub fn is_sprinting() -> Option<bool> {
        None
    }

    /// Whether it is crouching, which for a Bedrock client is sneak. `None`.
    #[must_use]
    pub fn is_crouching() -> Option<bool> {
        None
    }

    /// Whether it is swimming. `None`.
    #[must_use]
    pub fn is_swimming() -> Option<bool> {
        None
    }

    /// Whether it is flying. `None`.
    #[must_use]
    pub fn is_flying() -> Option<bool> {
        None
    }

    /// Whether it is gliding. `None`.
    #[must_use]
    pub fn is_gliding() -> Option<bool> {
        None
    }

    /// Whether it is riding something. `None`.
    #[must_use]
    pub fn is_riding() -> Option<bool> {
        None
    }

    /// Whether the server drives it. `Some(true)` for every entity but the local player is a
    /// guess, so this is `None`.
    #[must_use]
    pub fn is_ai_controlled() -> Option<bool> {
        None
    }

    /// Who owns it. `None`: ownership is not synced to a client in general.
    #[must_use]
    pub fn get_owner() -> Option<String> {
        None
    }

    /// What it is riding. `None`.
    #[must_use]
    pub fn get_vehicle() -> Option<String> {
        None
    }

    /// Who is riding it. Empty.
    #[must_use]
    pub fn get_passengers() -> Vec<String> {
        Vec::new()
    }

    /// What it is attacking. `None`: mob targets are not synced to a client.
    #[must_use]
    pub fn get_target() -> Option<String> {
        None
    }

    /// Where its eyes are. `None`.
    #[must_use]
    pub fn get_eye_position() -> Option<(f64, f64, f64)> {
        None
    }

    /// An entity's tags. Empty.
    #[must_use]
    pub fn get_tags() -> Vec<String> {
        Vec::new()
    }

    /// Adds a tag, which is server data. `false`.
    pub fn add_tag(_tag: &str) -> bool {
        super::live("tag")
    }

    /// Removes a tag, which is server data. `false`.
    pub fn remove_tag(_tag: &str) -> bool {
        super::live("tag")
    }

    /// One attribute. `None`.
    #[must_use]
    pub fn get_attribute(_name: &str) -> Option<f32> {
        None
    }

    /// Every attribute. Empty.
    #[must_use]
    pub fn get_attributes() -> Vec<(String, f32)> {
        Vec::new()
    }

    /// Sets an attribute, which is server data. `false`.
    pub fn set_attribute(_name: &str, _value: f32) -> bool {
        super::live("attribute")
    }

    /// One of an entity's components. `None`.
    #[must_use]
    pub fn get_component(_name: &str) -> Option<String> {
        None
    }

    /// Every component. Empty.
    #[must_use]
    pub fn get_components() -> Vec<String> {
        Vec::new()
    }

    /// Adds a component. `false`.
    pub fn add_component(_name: &str, _value: &str) -> bool {
        super::live("component")
    }

    /// Removes a component. `false`.
    pub fn remove_component(_name: &str) -> bool {
        super::live("component")
    }

    /// Whether an entity has a component. `None`.
    #[must_use]
    pub fn has_component(_name: &str) -> Option<bool> {
        None
    }

    /// How far one entity is from a position. `None`.
    #[must_use]
    pub fn get_distance(_to: (f64, f64, f64)) -> Option<f64> {
        None
    }

    /// The same, squared. `None`.
    #[must_use]
    pub fn get_distance_squared(_to: (f64, f64, f64)) -> Option<f64> {
        None
    }

    /// Where an entity is by its id. `None`.
    #[must_use]
    pub fn position_of(_id: &str) -> Option<(f64, f64, f64)> {
        None
    }

    /// Moves an entity. `false`: the client can only ask the server.
    pub fn set_position(_x: f64, _y: f64, _z: f64) -> bool {
        super::live("movement")
    }

    /// Sets how it moves. `false`.
    pub fn set_velocity(_x: f64, _y: f64, _z: f64) -> bool {
        super::live("movement")
    }

    /// Adds to how it moves. `false`.
    pub fn add_velocity(_x: f64, _y: f64, _z: f64) -> bool {
        super::live("movement")
    }

    /// Sets where it looks. `false`.
    pub fn set_rotation(_yaw: f32, _pitch: f32) -> bool {
        super::live("movement")
    }

    /// Sets how big it is. `false`.
    pub fn set_bounding_box(_width: f32, _height: f32) -> bool {
        super::live("movement")
    }

    /// Puts an entity somewhere else. `false`.
    pub fn teleport(_x: f64, _y: f64, _z: f64) -> bool {
        super::live("movement")
    }

    /// Makes an entity look at a point. `false`.
    pub fn look_at(_x: f64, _y: f64, _z: f64) -> bool {
        super::live("movement")
    }

    /// Makes a new entity. `false`: spawning is the server's.
    pub fn spawn(_kind: &str, _x: f64, _y: f64, _z: f64) -> bool {
        super::live("change")
    }

    /// Removes an entity. `false`.
    pub fn despawn(_id: &str) -> bool {
        super::live("change")
    }

    /// Removes an entity. `false`.
    pub fn remove(_id: &str) -> bool {
        super::live("change")
    }

    /// Kills an entity. `false`.
    pub fn kill(_id: &str) -> bool {
        super::live("change")
    }

    /// Discards an entity. `false`.
    pub fn discard(_id: &str) -> bool {
        super::live("change")
    }

    /// Hurts an entity. `false`.
    pub fn damage(_amount: f32) -> bool {
        super::live("change")
    }

    /// Heals an entity. `false`.
    pub fn heal(_amount: f32) -> bool {
        super::live("change")
    }

    /// Attacks something. `false`.
    pub fn attack(_id: &str) -> bool {
        super::live("change")
    }

    /// Sets an entity's health. `false`.
    pub fn set_health(_health: f32) -> bool {
        super::live("change")
    }

    /// Names an entity. `false`.
    pub fn set_name(_name: &str) -> bool {
        super::live("change")
    }

    /// Gives an entity a custom name. `false`.
    pub fn set_custom_name(_name: &str) -> bool {
        super::live("presentation")
    }

    /// Hides an entity. `false`.
    pub fn set_invisible(_invisible: bool) -> bool {
        super::live("presentation")
    }

    /// Makes an entity glow. `false`.
    pub fn set_glowing(_glowing: bool) -> bool {
        super::live("presentation")
    }

    /// Silences an entity. `false`.
    pub fn set_silent(_silent: bool) -> bool {
        super::live("presentation")
    }

    /// Makes an entity invulnerable. `false`.
    pub fn set_invulnerable(_invulnerable: bool) -> bool {
        super::live("change")
    }

    /// Puts a rider on something. `false`.
    pub fn mount(_vehicle: &str) -> bool {
        super::live("ownership")
    }

    /// Takes a rider off. `false`.
    pub fn dismount() -> bool {
        super::live("ownership")
    }

    /// Adds a passenger. `false`.
    pub fn add_passenger(_passenger: &str) -> bool {
        super::live("ownership")
    }

    /// Removes a passenger. `false`.
    pub fn remove_passenger(_passenger: &str) -> bool {
        super::live("ownership")
    }
}

/// The wishlist's spelling, which is the same functions at the top level where no declared
/// accessor holds the name, and `<name>_live` where one does.
pub use live_entities::{
    attack, damage, despawn, discard, dismount, heal, kill, mount, spawn, teleport,
};

pub use live_entities::{
    add_component as add_component_live, add_passenger, add_tag, add_velocity, discard as discard_live,
    get_attribute as get_live_attribute, get_attributes as get_live_attributes,
    get_component as get_live_component, get_components as get_live_components,
    get_distance as get_live_distance, get_distance_squared as get_live_distance_squared,
    get_eye_position, get_health as get_live_health, get_network_id, get_owner,
    get_passengers, get_position, get_previous_position, get_rotation, get_target, get_type,
    get_velocity,
    get_uuid, get_vehicle, get_world, is_alive, is_burning, is_crouching, is_dead, is_flying,
    is_gliding, is_in_water, is_invisible, is_on_ground, is_riding, is_sneaking, is_sprinting,
    is_swimming, look_at, remove as remove_live, remove_component as remove_component_live,
    remove_passenger, remove_tag, set_attribute, set_bounding_box, set_custom_name, set_glowing,
    set_health as set_live_health, set_invisible, set_name, set_position, set_rotation,
    set_silent, set_invulnerable, set_velocity,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|registry| registry.clear());
    }

    fn wisp() -> EntityType {
        EntityType::new("wisp", "Wisp")
            .family("mob")
            .family("undead")
            .size(0.5, 0.5)
            .health(8.0)
            .movement(0.3)
            .attribute("minecraft:movement", 0.3)
            .attribute("minecraft:follow_range", 16.0)
            .component(EntityComponent::new("minecraft:health", "8"))
            .tag("cinnaloader:spirits")
            .rideable(false)
            .hostile(false)
    }

    #[test]
    fn a_declared_entity_can_be_read_back() {
        let _serial = serial();
        fresh();
        assert!(register_type(wisp()));
        let declared = get_by_id("wisp").expect("declared");
        assert_eq!(declared.name, "Wisp");
        assert_eq!(declared.families, vec!["mob".to_owned(), "undead".to_owned()]);
        assert_eq!(declared.size, EntitySize { width: 0.5, height: 0.5 });
        assert_eq!(get_by_name("Wisp").map(|entity| entity.id), Some("wisp".to_owned()));
        assert_eq!(get_all().len(), 1);
        assert!(get_registry_id("wisp").is_some_and(|id| id.ends_with(":wisp")));
        assert_eq!(get_max_health("wisp"), Some(8.0));
        assert_eq!(get_attributes("wisp").len(), 2);
        assert_eq!(get_attribute("wisp", "minecraft:movement"), Some(0.3));
        assert_eq!(get_tags("wisp"), vec!["mob".to_owned(), "undead".to_owned()]);
        assert_eq!(get_components("wisp").len(), 1);
        assert!(has_component("wisp", "minecraft:health"));
        assert_eq!(is_player("wisp"), Some(false));
        assert_eq!(is_living("wisp"), Some(true));
        assert_eq!(is_hostile("wisp"), Some(false));
        assert_eq!(is_passive("wisp"), Some(true));
        assert_eq!(is_rideable("wisp"), Some(false));
        assert_eq!(get_dimensions("wisp").map(|size| size.height), Some(0.5));
        assert_eq!(get_eye_height("wisp"), Some(0.3));
        assert_eq!(
            get_bounding_box("wisp", 0.0, 64.0, 0.0),
            Some((-0.25, 64.0, -0.25, 0.25, 64.5, 0.25))
        );
        assert!(!is_registered_with_the_client("wisp"));
        assert!(register_component("wisp", EntityComponent::new("minecraft:loot", "none")));
        assert!(!register_component("nothing", EntityComponent::new("a", "b")));
        assert!(unregister_type("wisp"));
        assert!(!unregister_type("wisp"));
    }

    #[test]
    fn a_live_entity_is_refused_with_a_reason() {
        let _serial = serial();
        assert!(live_entities::get().is_none());
        assert!(live_entities::get_by_uuid("abc").is_none());
        assert!(live_entities::get_type().is_none());
        assert!(live_entities::get_position().is_none());
        assert!(live_entities::get_velocity().is_none());
        assert_eq!(live_entities::is_alive(), None);
        assert_eq!(live_entities::is_sneaking(), None);
        assert_eq!(live_entities::get_health(), None);
        assert!(live_entities::get_owner().is_none());
        assert!(live_entities::get_passengers().is_empty());
        assert!(!live_entities::spawn("wisp", 0.0, 64.0, 0.0));
        assert!(!live_entities::damage(1.0));
        assert!(!live_entities::teleport(0.0, 64.0, 0.0));
        assert!(!live_entities::mount("horse"));
        assert!(!live_entities::set_glowing(true));
        assert_eq!(refusals().len(), 12);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn distance_is_the_one_pure_piece_of_entity_arithmetic() {
        assert_eq!(distance((0.0, 0.0, 0.0), (3.0, 4.0, 0.0)), 5.0);
        assert_eq!(distance_squared((0.0, 0.0, 0.0), (3.0, 4.0, 0.0)), 25.0);
        assert_eq!(distance((1.0, 2.0, 3.0), (1.0, 2.0, 3.0)), 0.0);
    }

    #[test]
    fn the_entity_event_kinds_can_be_subscribed_to_before_they_fire() {
        let _serial = serial();
        assert!(register_event_kinds() > 0);
        assert!(has_event_kind("entity_spawn"));
        assert!(!has_event_kind("nothing_at_all"));
        assert_eq!(event_kinds().len(), 18);
    }

    #[test]
    fn an_entity_declaration_renders_for_a_report() {
        let text = wisp().to_string();
        assert!(text.contains("Wisp"), "{text}");
        assert!(text.contains("wisp"), "{text}");
        assert!(text.contains("8 health"), "{text}");
    }
}
