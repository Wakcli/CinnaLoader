//! Blocks: the types a mod declares, and the world this host cannot reach.
//!
//! The same split as section 15, and for the same reason. A **block type** is data a mod
//! ships: its id, properties, hardness, resistance, friction, sound, shapes, tool
//! requirement, drops, flags and behaviour callbacks. Declaring one is real, and every mod
//! can read the registry. Putting a block *in the world* — `get_at`, `set`, `place`,
//! `break_block`, light levels, block entities, scheduled ticks — is the client's world
//! state, which section 10 lists as `never`; every one of those answers `None` or `false`
//! with the reason from [`why_unknown`].
//!
//! The behaviour callbacks are stored on the declaration and are **never called by this
//! host**: nothing here sees a block event from the client. They are recorded so a mod's
//! implementation is complete and can be called by the mod itself, and so that the day the
//! hooks land, the callbacks are already where they belong.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! blocks::register(BlockDescription::new("lantern", "Lantern")
//!     .hardness(1.5)
//!     .light_level(12)
//!     .property(BlockProperty::facing(&["north", "south"]))
//!     .drops(&["lantern"]));
//! assert_eq!(blocks::get_hardness("lantern"), Some(1.5));
//! assert!(blocks::get_at(0, 64, 0).is_none(), "the world is the client's");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events::{self, kind as event_kind};

/// Why a block question about the world has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "position" => {
            "the block at a position is the client's world state; reading it needs the hooks \
             section 10 lists as never"
                .to_owned()
        }
        "change" => "changing a block is the client's world, and needs those same hooks".to_owned(),
        "light" => "light levels are computed by the client".to_owned(),
        "entity" => "block entities belong to the client's world".to_owned(),
        "tick" => "scheduled block ticks are the client's scheduler".to_owned(),
        "shape" => "collision and render shapes are the client's, from its own block definitions".to_owned(),
        "fluid" => "fluid state is part of the client's world".to_owned(),
        "event" => {
            "block events come from the client as it runs; nothing here sees them, so the \
             callbacks on a declaration are never called by this host"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// A block event this host would have to receive from the client to fire.
pub const BLOCK_EVENTS: [&str; 18] = [
    "block_break",
    "block_place",
    "block_interact",
    "block_attack",
    "block_update",
    "block_neighbor_update",
    "block_state_change",
    "block_entity_load",
    "block_entity_save",
    "block_tick",
    "block_drop",
    "block_explosion",
    "block_burn",
    "block_fade",
    "block_grow",
    "fluid_flow",
    "piston_move",
    "redstone_update",
];

/// Teaches the event bus each block event name, so a mod can subscribe to one before the
/// hooks exist to fire it.
pub fn register_event_kinds() -> usize {
    BLOCK_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// What a block's property may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockProperty {
    pub name: String,
    /// The values it can take, in the order a panel would show them.
    pub values: Vec<String>,
    pub default: String,
}

impl BlockProperty {
    /// A property with a default of its first value.
    #[must_use]
    pub fn new(name: &str, values: &[&str]) -> Self {
        let values: Vec<String> = values.iter().map(|value| (*value).to_owned()).collect();
        let default = values.first().cloned().unwrap_or_default();
        Self {
            name: name.to_owned(),
            values,
            default,
        }
    }

    /// The same, with another default.
    #[must_use]
    pub fn defaulting(mut self, value: &str) -> Self {
        if self.values.iter().any(|held| held == value) {
            self.default = value.to_owned();
        }
        self
    }

    /// `facing`, which every block with a direction has.
    #[must_use]
    pub fn facing(values: &[&str]) -> Self {
        Self::new("facing", values)
    }

    /// `waterlogged`.
    #[must_use]
    pub fn waterlogged() -> Self {
        Self::new("waterlogged", &["false", "true"])
    }

    /// `powered`.
    #[must_use]
    pub fn powered() -> Self {
        Self::new("powered", &["false", "true"])
    }

    /// One value, in the order the property lists them, or `None` when it does not take it.
    #[must_use]
    pub fn index_of(&self, value: &str) -> Option<usize> {
        self.values.iter().position(|held| held == value)
    }
}

/// What a block is made of, for the sound it makes and what tool it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlockMaterial {
    #[default]
    Stone,
    Wood,
    Dirt,
    Grass,
    Metal,
    Glass,
    Wool,
    Plant,
    Sand,
    Liquid,
    Ice,
    Custom,
}

impl BlockMaterial {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stone => "stone",
            Self::Wood => "wood",
            Self::Dirt => "dirt",
            Self::Grass => "grass",
            Self::Metal => "metal",
            Self::Glass => "glass",
            Self::Wool => "wool",
            Self::Plant => "plant",
            Self::Sand => "sand",
            Self::Liquid => "liquid",
            Self::Ice => "ice",
            Self::Custom => "custom",
        }
    }

    /// The sound this material makes when it is stepped on or broken.
    #[must_use]
    pub fn sound(self) -> &'static str {
        match self {
            Self::Stone => "stone",
            Self::Wood => "wood",
            Self::Dirt | Self::Grass | Self::Sand => "grass",
            Self::Metal => "metal",
            Self::Glass => "glass",
            Self::Wool => "cloth",
            Self::Plant => "grass",
            Self::Liquid => "liquid",
            Self::Ice => "glass",
            Self::Custom => "stone",
        }
    }
}

/// The shapes a block has. This host stores what a mod says; the client computes its own.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BlockShapes {
    pub collision: Option<(f32, f32, f32, f32, f32, f32)>,
    pub outline: Option<(f32, f32, f32, f32, f32, f32)>,
    pub render: Option<(f32, f32, f32, f32, f32, f32)>,
    pub selection: Option<(f32, f32, f32, f32, f32, f32)>,
}

/// What tool a block wants, and what it drops.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolRequirement {
    /// The tool class, such as `pickaxe`; empty means anything works.
    pub tool: String,
    /// The lowest tier that works, such as `iron`.
    pub tier: String,
    /// Whether it needs the right tool at all to drop anything.
    pub required: bool,
}

/// What a block does when the client would tell it something happened. Recorded, not called
/// here — see the module docs.
#[derive(Clone, Default)]
pub struct BlockBehaviour {
    pub on_neighbor_update: Option<fn()>,
    pub on_random_tick: Option<fn()>,
    pub on_scheduled_tick: Option<fn()>,
    pub on_use: Option<fn()>,
    pub on_attack: Option<fn()>,
    pub on_break: Option<fn()>,
    pub on_placed: Option<fn()>,
}

impl BlockBehaviour {
    /// The callbacks that are set, by name, for a report.
    #[must_use]
    pub fn set_callbacks(&self) -> Vec<&'static str> {
        [
            ("on_neighbor_update", self.on_neighbor_update.is_some()),
            ("on_random_tick", self.on_random_tick.is_some()),
            ("on_scheduled_tick", self.on_scheduled_tick.is_some()),
            ("on_use", self.on_use.is_some()),
            ("on_attack", self.on_attack.is_some()),
            ("on_break", self.on_break.is_some()),
            ("on_placed", self.on_placed.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect()
    }
}

impl fmt::Debug for BlockBehaviour {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "BlockBehaviour({:?})", self.set_callbacks())
    }
}

impl PartialEq for BlockBehaviour {
    /// Two behaviours are the same when they carry the same callbacks: function pointers are
    /// compared by what is set, not by where the code happens to sit.
    fn eq(&self, other: &Self) -> bool {
        self.set_callbacks() == other.set_callbacks()
    }
}

impl Eq for BlockBehaviour {}

/// A block type a mod declares.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDescription {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub material: BlockMaterial,
    pub hardness: f32,
    pub resistance: f32,
    pub friction: f32,
    pub light_level: u8,
    /// What it emits, when it is a light source.
    pub light_emission: u8,
    pub solid: bool,
    pub replaceable: bool,
    pub transparent: bool,
    pub fluid: bool,
    pub air: bool,
    pub flammable: bool,
    pub gravity: bool,
    pub properties: Vec<BlockProperty>,
    pub shapes: BlockShapes,
    pub tool: ToolRequirement,
    pub drops: Vec<String>,
    /// The fluid this block holds, when it is one.
    pub fluid_state: Option<String>,
    /// Whether it can have a block entity.
    pub block_entity: bool,
    pub behaviour: BlockBehaviour,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered: bool,
}

impl BlockDescription {
    /// A solid stone-like block with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            material: BlockMaterial::Stone,
            hardness: 1.0,
            resistance: 1.0,
            friction: 0.6,
            light_level: 0,
            light_emission: 0,
            solid: true,
            replaceable: false,
            transparent: false,
            fluid: false,
            air: false,
            flammable: false,
            gravity: false,
            properties: Vec::new(),
            shapes: BlockShapes::default(),
            tool: ToolRequirement::default(),
            drops: Vec::new(),
            fluid_state: None,
            block_entity: false,
            behaviour: BlockBehaviour::default(),
            registered: false,
        }
    }

    #[must_use]
    pub fn material(mut self, material: BlockMaterial) -> Self {
        self.material = material;
        self
    }

    #[must_use]
    pub fn hardness(mut self, hardness: f32) -> Self {
        self.hardness = hardness.max(0.0);
        self
    }

    #[must_use]
    pub fn resistance(mut self, resistance: f32) -> Self {
        self.resistance = resistance.max(0.0);
        self
    }

    #[must_use]
    pub fn friction(mut self, friction: f32) -> Self {
        self.friction = friction.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn light_level(mut self, level: u8) -> Self {
        self.light_emission = level.min(15);
        self.light_level = self.light_emission;
        self
    }

    #[must_use]
    pub fn transparent(mut self, transparent: bool) -> Self {
        self.transparent = transparent;
        self
    }

    #[must_use]
    pub fn replaceable(mut self, replaceable: bool) -> Self {
        self.replaceable = replaceable;
        self
    }

    #[must_use]
    pub fn fluid(mut self, fluid_state: &str) -> Self {
        self.fluid = true;
        self.fluid_state = Some(fluid_state.to_owned());
        self
    }

    #[must_use]
    pub fn air(mut self) -> Self {
        self.air = true;
        self.solid = false;
        self.transparent = true;
        self.replaceable = true;
        self
    }

    #[must_use]
    pub fn flammable(mut self, flammable: bool) -> Self {
        self.flammable = flammable;
        self
    }

    #[must_use]
    pub fn gravity(mut self, gravity: bool) -> Self {
        self.gravity = gravity;
        self
    }

    #[must_use]
    pub fn property(mut self, property: BlockProperty) -> Self {
        self.properties.retain(|held| held.name != property.name);
        self.properties.push(property);
        self
    }

    #[must_use]
    pub fn shapes(mut self, shapes: BlockShapes) -> Self {
        self.shapes = shapes;
        self
    }

    #[must_use]
    pub fn tool(mut self, tool: &str, tier: &str, required: bool) -> Self {
        self.tool = ToolRequirement {
            tool: tool.to_owned(),
            tier: tier.to_owned(),
            required,
        };
        self
    }

    #[must_use]
    pub fn drops(mut self, drops: &[&str]) -> Self {
        self.drops = drops.iter().map(|drop| (*drop).to_owned()).collect();
        self
    }

    #[must_use]
    pub fn block_entity(mut self, block_entity: bool) -> Self {
        self.block_entity = block_entity;
        self
    }

    #[must_use]
    pub fn behaviour(mut self, behaviour: BlockBehaviour) -> Self {
        self.behaviour = behaviour;
        self
    }

    /// The sound this block makes, from its material.
    #[must_use]
    pub fn sound(&self) -> &'static str {
        self.material.sound()
    }

    /// A property by name.
    #[must_use]
    pub fn property_of(&self, name: &str) -> Option<&BlockProperty> {
        self.properties.iter().find(|held| held.name == name)
    }

    /// The default state, as `property=value` pairs.
    #[must_use]
    pub fn default_state(&self) -> Vec<(String, String)> {
        self.properties
            .iter()
            .map(|property| (property.name.clone(), property.default.clone()))
            .collect()
    }
}

impl fmt::Display for BlockDescription {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}, hardness {})",
            self.name,
            self.id,
            self.material.as_str(),
            self.hardness
        )
    }
}

fn registry() -> &'static Mutex<BTreeMap<String, BlockDescription>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, BlockDescription>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, BlockDescription>) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a block type, or replaces the one with that id.
pub fn register(description: BlockDescription) -> bool {
    with(|registry| registry.insert(description.id.clone(), description).is_none())
}

/// Forgets a declared block type.
pub fn unregister(id: &str) -> bool {
    with(|registry| registry.remove(id).is_some())
}

/// Declares a property on a block that is already declared, and answers whether it was
/// there.
pub fn register_property(id: &str, property: BlockProperty) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(description) => {
            description.properties.retain(|held| held.name != property.name);
            description.properties.push(property);
            true
        }
        None => false,
    })
}

/// A property on a declared block.
#[must_use]
pub fn get_property(id: &str, name: &str) -> Option<BlockProperty> {
    with(|registry| {
        registry
            .get(id)
            .and_then(|description| description.property_of(name))
            .cloned()
    })
}

/// Every property of a declared block.
#[must_use]
pub fn get_properties(id: &str) -> Vec<BlockProperty> {
    with(|registry| {
        registry
            .get(id)
            .map(|description| description.properties.clone())
            .unwrap_or_default()
    })
}

/// Says that a declared block can have a block entity.
pub fn register_block_entity(id: &str) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(description) => {
            description.block_entity = true;
            true
        }
        None => false,
    })
}

/// A declared block type, by id.
#[must_use]
pub fn get_type(id: &str) -> Option<BlockDescription> {
    with(|registry| registry.get(id).cloned())
}

/// The same, by the name a mod gave it.
#[must_use]
pub fn get_by_name(name: &str) -> Option<BlockDescription> {
    with(|registry| {
        registry
            .values()
            .find(|description| description.name == name)
            .cloned()
    })
}

/// Every declared block type, in id order.
#[must_use]
pub fn get_all() -> Vec<BlockDescription> {
    with(|registry| registry.values().cloned().collect())
}

/// A declared block's id, which is the same as the one it was registered with.
#[must_use]
pub fn get_id(id: &str) -> Option<String> {
    get_type(id).map(|description| description.id)
}

/// A declared block's registry id, which is its id with the owning mod in front.
#[must_use]
pub fn get_registry_id(id: &str) -> Option<String> {
    get_type(id).map(|description| format!("{}:{}", description.owner, description.id))
}

/// A declared block's hardness.
#[must_use]
pub fn get_hardness(id: &str) -> Option<f32> {
    get_type(id).map(|description| description.hardness)
}

/// A declared block's blast resistance.
#[must_use]
pub fn get_resistance(id: &str) -> Option<f32> {
    get_type(id).map(|description| description.resistance)
}

/// A declared block's friction.
#[must_use]
pub fn get_friction(id: &str) -> Option<f32> {
    get_type(id).map(|description| description.friction)
}

/// A declared block's sound, from its material.
#[must_use]
pub fn get_sound(id: &str) -> Option<&'static str> {
    get_type(id).map(|description| description.material.sound())
}

/// A declared block's collision shape.
#[must_use]
pub fn get_collision_shape(id: &str) -> Option<(f32, f32, f32, f32, f32, f32)> {
    get_type(id).and_then(|description| description.shapes.collision)
}

/// A declared block's outline shape.
#[must_use]
pub fn get_outline_shape(id: &str) -> Option<(f32, f32, f32, f32, f32, f32)> {
    get_type(id).and_then(|description| description.shapes.outline)
}

/// A declared block's selection shape.
#[must_use]
pub fn get_selection_shape(id: &str) -> Option<(f32, f32, f32, f32, f32, f32)> {
    get_type(id).and_then(|description| description.shapes.selection)
}

/// A declared block's render shape.
#[must_use]
pub fn get_render_shape(id: &str) -> Option<(f32, f32, f32, f32, f32, f32)> {
    get_type(id).and_then(|description| description.shapes.render)
}

/// A declared block's fluid state.
#[must_use]
pub fn get_fluid_state(id: &str) -> Option<String> {
    get_type(id).and_then(|description| description.fluid_state)
}

/// A declared block's drops.
#[must_use]
pub fn get_drops(id: &str) -> Vec<String> {
    get_type(id)
        .map(|description| description.drops)
        .unwrap_or_default()
}

/// What tool a declared block wants.
#[must_use]
pub fn get_tool_requirement(id: &str) -> Option<ToolRequirement> {
    get_type(id).map(|description| description.tool)
}

/// What a declared block emits.
#[must_use]
pub fn get_light_level(id: &str) -> Option<u8> {
    get_type(id).map(|description| description.light_level)
}

/// Whether a declared block is solid.
#[must_use]
pub fn is_solid(id: &str) -> Option<bool> {
    get_type(id).map(|description| description.solid)
}

/// Whether a declared block can be replaced by placing into it.
#[must_use]
pub fn is_replaceable(id: &str) -> Option<bool> {
    get_type(id).map(|description| description.replaceable)
}

/// Whether a declared block lets light through.
#[must_use]
pub fn is_transparent(id: &str) -> Option<bool> {
    get_type(id).map(|description| description.transparent)
}

/// Whether a declared block is a fluid.
#[must_use]
pub fn is_fluid(id: &str) -> Option<bool> {
    get_type(id).map(|description| description.fluid)
}

/// Whether a declared block is air.
#[must_use]
pub fn is_air(id: &str) -> Option<bool> {
    get_type(id).map(|description| description.air)
}

/// Whether a declared block burns.
#[must_use]
pub fn is_flammable(id: &str) -> Option<bool> {
    get_type(id).map(|description| description.flammable)
}

/// The behaviour a mod put on a declared block, which this host never calls.
#[must_use]
pub fn get_behaviour(id: &str) -> Option<BlockBehaviour> {
    get_type(id).map(|description| description.behaviour)
}

/// Whether the client has been told about a declared block, which it never has.
#[must_use]
pub fn is_registered_with_the_client(id: &str) -> bool {
    get_type(id).is_some_and(|description| description.registered)
}

/// What this host cannot do about blocks in the world, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "position",
        "change",
        "light",
        "entity",
        "tick",
        "shape",
        "fluid",
        "event",
    ]
    .iter()
    .map(|what| ((*what).to_owned(), why_unknown(what)))
    .collect()
}

fn no_hook(what: &str) -> bool {
    crate::warn!(
        "a mod asked this host to {what}: {}",
        why_unknown("change")
    );
    false
}

/// The world half: every one of these is the client's, and answers honestly rather than
/// inventing a block.
pub mod known_only_to_the_client {
    /// The block at a position.
    #[must_use]
    pub fn get_at(_x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }

    /// The block a mod would put at a position, which is the same question.
    #[must_use]
    pub fn get() -> Option<String> {
        None
    }

    /// The state at a position, as `property=value` pairs.
    #[must_use]
    pub fn get_state(_x: i32, _y: i32, _z: i32) -> Vec<(String, String)> {
        Vec::new()
    }

    /// One property at a position.
    #[must_use]
    pub fn get_property(_x: i32, _y: i32, _z: i32, _name: &str) -> Option<String> {
        None
    }

    /// The block type at a position.
    #[must_use]
    pub fn get_type(_x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }

    /// The block's id at a position.
    #[must_use]
    pub fn get_id(_x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }

    /// The registry id of the block at a position.
    #[must_use]
    pub fn get_registry_id(_x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }

    /// Puts a block at a position.
    pub fn set(_x: i32, _y: i32, _z: i32, _block: &str) -> bool {
        super::no_hook("change a block")
    }

    /// Puts a block with a state at a position.
    pub fn set_state(_x: i32, _y: i32, _z: i32, _block: &str, _state: &[(&str, &str)]) -> bool {
        super::no_hook("change a block state")
    }

    /// Changes one property of the block at a position.
    pub fn set_property(_x: i32, _y: i32, _z: i32, _name: &str, _value: &str) -> bool {
        super::no_hook("change a block property")
    }

    /// Places a block, as a player would.
    pub fn place(_x: i32, _y: i32, _z: i32, _block: &str) -> bool {
        super::no_hook("place a block")
    }

    /// Breaks the block at a position.
    pub fn break_block(_x: i32, _y: i32, _z: i32) -> bool {
        super::no_hook("break a block")
    }

    /// Removes the block at a position without drops.
    pub fn remove(_x: i32, _y: i32, _z: i32) -> bool {
        super::no_hook("remove a block")
    }

    /// Replaces the block at a position when it is `from`.
    pub fn replace(_x: i32, _y: i32, _z: i32, _from: &str, _to: &str) -> bool {
        super::no_hook("replace a block")
    }

    /// Asks the client to update the block at a position.
    pub fn update(_x: i32, _y: i32, _z: i32) -> bool {
        super::no_hook("update a block")
    }

    /// Asks the client to tell the neighbours.
    pub fn notify_neighbors(_x: i32, _y: i32, _z: i32) -> bool {
        super::no_hook("notify a block's neighbours")
    }

    /// Asks the client to tick a block later.
    pub fn schedule_tick(_x: i32, _y: i32, _z: i32, _delay: u32) -> bool {
        super::no_hook("schedule a block tick")
    }

    /// Asks the client to drop a scheduled tick.
    pub fn cancel_scheduled_tick(_x: i32, _y: i32, _z: i32) -> bool {
        super::no_hook("cancel a block tick")
    }

    /// The light level at a position.
    #[must_use]
    pub fn get_light_level(_x: i32, _y: i32, _z: i32) -> Option<u8> {
        None
    }

    /// How much light the block at a position blocks.
    #[must_use]
    pub fn get_light_block(_x: i32, _y: i32, _z: i32) -> Option<u8> {
        None
    }

    /// The fluid state at a position.
    #[must_use]
    pub fn get_fluid_state(_x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }

    /// The block entity at a position.
    #[must_use]
    pub fn get_block_entity(_x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }

    /// Asks the client to make a block entity.
    pub fn create_block_entity(_x: i32, _y: i32, _z: i32, _kind: &str) -> bool {
        super::no_hook("create a block entity")
    }

    /// Whether a block may be placed at a position.
    #[must_use]
    pub fn can_place_at(_x: i32, _y: i32, _z: i32, _block: &str) -> Option<bool> {
        None
    }

    /// Whether the block at a position would survive where it is.
    #[must_use]
    pub fn can_survive(_x: i32, _y: i32, _z: i32) -> Option<bool> {
        None
    }
}

/// The world accessors, at the top level, under the names the wishlist uses. Where the
/// wishlist spells a world question and a declared-type question the same way — `get_type`,
/// `get_id`, `get_property`, `get_light_level`, `get_fluid_state`, `get_block_entity` — the
/// declared-type one keeps the name, because it is the one that can answer, and the world
/// question is `<name>_at`. The module path
/// [`known_only_to_the_client`](self::known_only_to_the_client) has every one of them under
/// the wishlist's own spelling.
pub use known_only_to_the_client::{
    break_block, can_place_at, can_survive, cancel_scheduled_tick, create_block_entity, get,
    get_at, get_light_block, get_state, notify_neighbors, place, remove, replace, schedule_tick,
    set, set_property, set_state, update,
};

pub use known_only_to_the_client::{
    get_block_entity as get_block_entity_at, get_fluid_state as get_fluid_state_at,
    get_id as get_id_at, get_light_level as get_light_level_at, get_property as get_property_at,
    get_registry_id as get_registry_id_at, get_type as get_type_at,
};

/// The behaviour half. These are the callbacks a declaration holds, and this host calls
/// none of them: the client's block events are what would, and they are out of reach.
pub mod behaviour {
    /// Which callbacks are set on a declared block, for a report.
    #[must_use]
    pub fn callbacks(id: &str) -> Vec<&'static str> {
        super::get_behaviour(id)
            .map(|behaviour| behaviour.set_callbacks())
            .unwrap_or_default()
    }

    /// Why nothing calls them here.
    #[must_use]
    pub fn why_never_called() -> String {
        super::why_unknown("event")
    }
}

/// The event kinds this host would need from the client to fire a block callback.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    BLOCK_EVENTS.to_vec()
}

/// Whether a block event kind is one the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    BLOCK_EVENTS.contains(&kind) && events::is_registered_kind(kind)
}

/// The name of the event kind a mod would subscribe to for a block breaking, spelled the way
/// the bus spells it.
#[must_use]
pub fn break_event_kind() -> &'static str {
    event_kind::REGISTRY_CHANGED
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

    fn lantern() -> BlockDescription {
        BlockDescription::new("lantern", "Lantern")
            .material(BlockMaterial::Metal)
            .hardness(1.5)
            .resistance(2.0)
            .light_level(12)
            .transparent(true)
            .property(BlockProperty::facing(&["north", "south", "east", "west"]))
            .property(BlockProperty::waterlogged())
            .tool("pickaxe", "iron", true)
            .drops(&["lantern"])
            .block_entity(true)
            .behaviour(BlockBehaviour {
                on_break: Some(|| {}),
                ..BlockBehaviour::default()
            })
    }

    #[test]
    fn a_declared_block_can_be_read_back() {
        let _serial = serial();
        fresh();
        assert!(register(lantern()));
        assert!(!register(lantern()), "the second registration replaces the first");
        assert_eq!(get_type("lantern").map(|block| block.name), Some("Lantern".to_owned()));
        assert_eq!(get_by_name("Lantern").map(|block| block.id), Some("lantern".to_owned()));
        assert_eq!(get_id("lantern"), Some("lantern".to_owned()));
        assert!(get_registry_id("lantern").is_some_and(|id| id.ends_with(":lantern")));
        assert_eq!(get_hardness("lantern"), Some(1.5));
        assert_eq!(get_resistance("lantern"), Some(2.0));
        assert_eq!(get_friction("lantern"), Some(0.6));
        assert_eq!(get_sound("lantern"), Some("metal"));
        assert_eq!(get_light_level("lantern"), Some(12));
        assert_eq!(is_transparent("lantern"), Some(true));
        assert_eq!(is_solid("lantern"), Some(true));
        assert_eq!(is_fluid("lantern"), Some(false));
        assert_eq!(get_drops("lantern"), vec!["lantern".to_owned()]);
        assert_eq!(
            get_tool_requirement("lantern").map(|tool| tool.tool),
            Some("pickaxe".to_owned())
        );
        assert_eq!(get_all().len(), 1);
    }

    #[test]
    fn properties_and_block_entities_are_declared_on_a_block() {
        let _serial = serial();
        fresh();
        register(lantern());
        assert_eq!(get_properties("lantern").len(), 2);
        assert_eq!(
            get_property("lantern", "waterlogged").map(|property| property.default),
            Some("false".to_owned())
        );
        assert!(get_property("lantern", "nowhere").is_none());
        assert!(register_property("lantern", BlockProperty::powered()));
        assert_eq!(get_properties("lantern").len(), 3);
        assert!(!register_property("nothing", BlockProperty::powered()));
        assert!(get_type("lantern").is_some_and(|block| block.block_entity));
        assert!(register_block_entity("lantern"));
        assert!(!register_block_entity("nothing"));
        assert!(!is_registered_with_the_client("lantern"), "the client was never told");
    }

    #[test]
    fn a_property_offers_its_values_in_order() {
        let property = BlockProperty::facing(&["north", "south"]).defaulting("south");
        assert_eq!(property.default, "south");
        assert_eq!(property.index_of("south"), Some(1));
        assert_eq!(property.index_of("up"), None);
        assert_eq!(BlockProperty::powered().default, "false");
        assert_eq!(
            BlockProperty::new("none", &[]).default,
            String::new(),
            "no values means no default"
        );
        // A material picks the sound the game would play.
        assert_eq!(BlockMaterial::Wood.sound(), "wood");
        assert_eq!(BlockMaterial::Grass.sound(), "grass");
        assert_eq!(BlockMaterial::Liquid.as_str(), "liquid");
    }

    #[test]
    fn every_block_question_about_the_world_is_refused_with_a_reason() {
        let _serial = serial();
        assert!(known_only_to_the_client::get_at(0, 64, 0).is_none());
        assert!(known_only_to_the_client::get().is_none());
        assert!(known_only_to_the_client::get_state(0, 64, 0).is_empty());
        assert!(known_only_to_the_client::get_type(0, 64, 0).is_none());
        assert!(known_only_to_the_client::get_light_level(0, 64, 0).is_none());
        assert!(known_only_to_the_client::get_block_entity(0, 64, 0).is_none());
        assert!(known_only_to_the_client::can_place_at(0, 64, 0, "lantern").is_none());
        assert!(!known_only_to_the_client::set(0, 64, 0, "lantern"));
        assert!(!known_only_to_the_client::place(0, 64, 0, "lantern"));
        assert!(!known_only_to_the_client::break_block(0, 64, 0));
        assert!(!known_only_to_the_client::schedule_tick(0, 64, 0, 20));
        assert!(!known_only_to_the_client::create_block_entity(0, 64, 0, "chest"));
        assert_eq!(refusals().len(), 8);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_behaviour_callbacks_are_recorded_and_never_called_here() {
        let _serial = serial();
        fresh();
        register(lantern());
        assert_eq!(behaviour::callbacks("lantern"), vec!["on_break"]);
        assert!(behaviour::callbacks("nothing").is_empty());
        assert!(behaviour::why_never_called().contains("never called"));
        assert!(get_behaviour("lantern")
            .expect("the declaration carries it")
            .on_break
            .is_some());
    }

    #[test]
    fn the_block_event_kinds_can_be_subscribed_to_before_they_fire() {
        let _serial = serial();
        assert!(register_event_kinds() > 0);
        assert!(has_event_kind("block_break"));
        assert!(!has_event_kind("nothing_at_all"));
        assert_eq!(event_kinds().len(), 18);
    }
}
