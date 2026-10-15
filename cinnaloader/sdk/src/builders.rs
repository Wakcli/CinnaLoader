//! Builders: fluent chains that end in a real block, item or entity.
//!
//! Section 78's `BlockBuilder`, `ItemBuilder` and `EntityBuilder` are one chain each. Every
//! setter returns the builder and only collects; [`BlockBuilder::register`] validates the
//! whole chain once and lands the result in the module that owns that kind — a block in
//! [`crate::blocks`], an item in [`crate::items`], an entity in [`crate::entities`] — so
//! reading it back is the same call a mod would make without a builder. An empty id, a
//! duplicate id, or a value out of range answers a [`BuildError`] naming the field and the
//! value, and nothing is registered.
//!
//! What a builder cannot do is hand a block, item or entity to the *client's* own registries,
//! the ones it froze at startup. [`register_with_the_client`] warns and answers `false`, with
//! the sentence from [`why_unknown`].
//!
//! ```
//! use cinnaloader::builders::{BlockBuilder, ItemBuilder};
//! use cinnaloader::{blocks, items};
//!
//! BlockBuilder::new()
//!     .id("demo:glowstone")
//!     .name("Glowstone")
//!     .hardness(1.5)
//!     .light_level(15)
//!     .register()
//!     .expect("in range and free");
//! assert_eq!(blocks::get_hardness("demo:glowstone"), Some(1.5));
//! assert_eq!(blocks::get_light_level("demo:glowstone"), Some(15));
//!
//! ItemBuilder::new().id("demo:gem").max_stack_size(16).register().expect("in range");
//! assert_eq!(items::get_max_stack_size("demo:gem"), Some(16));
//! assert!(BlockBuilder::new().register().is_err(), "an empty id is refused");
//! ```

use std::fmt;

use crate::blocks::{
    BlockBehaviour, BlockDescription, BlockMaterial, BlockProperty, BlockShapes, ToolRequirement,
};
use crate::entities::{EntityComponent, EntityType};
use crate::items::{Component, ItemType};

/// What went wrong before anything was registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildErrorKind {
    /// A field that has to be set was left empty.
    Missing,
    /// The id is already taken in the owning module.
    Duplicate,
    /// A value lies outside the range the game allows.
    OutOfRange,
}

/// A refused build: the field, the value that was refused, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError {
    kind: BuildErrorKind,
    field: &'static str,
    value: String,
}

impl BuildError {
    fn missing(field: &'static str) -> Self {
        Self {
            kind: BuildErrorKind::Missing,
            field,
            value: String::new(),
        }
    }

    fn duplicate(id: &str) -> Self {
        Self {
            kind: BuildErrorKind::Duplicate,
            field: "id",
            value: id.to_owned(),
        }
    }

    fn out_of_range(field: &'static str, value: &str) -> Self {
        Self {
            kind: BuildErrorKind::OutOfRange,
            field,
            value: value.to_owned(),
        }
    }

    #[must_use]
    pub fn kind(&self) -> BuildErrorKind {
        self.kind
    }

    #[must_use]
    pub fn field(&self) -> &'static str {
        self.field
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            BuildErrorKind::Missing => write!(formatter, "the {} is missing", self.field),
            BuildErrorKind::Duplicate => {
                write!(formatter, "the id {} is already registered", self.value)
            }
            BuildErrorKind::OutOfRange => {
                write!(formatter, "the {} {} is out of range", self.field, self.value)
            }
        }
    }
}

impl std::error::Error for BuildError {}

/// The material that makes a sound, so `BlockBuilder::sound` lands somewhere real.
fn sound_material(sound: &str) -> BlockMaterial {
    match sound {
        "wood" => BlockMaterial::Wood,
        "grass" => BlockMaterial::Grass,
        "sand" => BlockMaterial::Sand,
        "metal" => BlockMaterial::Metal,
        "glass" => BlockMaterial::Glass,
        "cloth" => BlockMaterial::Wool,
        "plant" => BlockMaterial::Plant,
        "liquid" => BlockMaterial::Liquid,
        "ice" => BlockMaterial::Ice,
        _ => BlockMaterial::Stone,
    }
}

/// A fluent chain for a block, ending in [`BlockBuilder::register`].
#[derive(Debug, Clone, PartialEq)]
pub struct BlockBuilder {
    id: String,
    name: String,
    hardness: f32,
    resistance: f32,
    sound: String,
    light_level: u8,
    collision: Option<(f32, f32, f32, f32, f32, f32)>,
    outline: Option<(f32, f32, f32, f32, f32, f32)>,
    texture: String,
    model: String,
    drops: Vec<String>,
    tool: ToolRequirement,
    properties: Vec<BlockProperty>,
    behavior: BlockBehaviour,
}

impl Default for BlockBuilder {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            hardness: 1.0,
            resistance: 1.0,
            sound: "stone".to_owned(),
            light_level: 0,
            collision: None,
            outline: None,
            texture: String::new(),
            model: String::new(),
            drops: Vec::new(),
            tool: ToolRequirement::default(),
            properties: Vec::new(),
            behavior: BlockBehaviour::default(),
        }
    }
}

impl BlockBuilder {
    /// A solid stone-like block with the defaults a caller would expect.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }

    #[must_use]
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    #[must_use]
    pub fn hardness(mut self, hardness: f32) -> Self {
        self.hardness = hardness;
        self
    }

    #[must_use]
    pub fn resistance(mut self, resistance: f32) -> Self {
        self.resistance = resistance;
        self
    }

    #[must_use]
    pub fn sound(mut self, sound: &str) -> Self {
        self.sound = sound.to_owned();
        self
    }

    #[must_use]
    pub fn light_level(mut self, level: u8) -> Self {
        self.light_level = level;
        self
    }

    #[must_use]
    pub fn collision(mut self, shape: (f32, f32, f32, f32, f32, f32)) -> Self {
        self.collision = Some(shape);
        self
    }

    #[must_use]
    pub fn outline(mut self, shape: (f32, f32, f32, f32, f32, f32)) -> Self {
        self.outline = Some(shape);
        self
    }

    #[must_use]
    pub fn texture(mut self, texture: &str) -> Self {
        self.texture = texture.to_owned();
        self
    }

    #[must_use]
    pub fn model(mut self, model: &str) -> Self {
        self.model = model.to_owned();
        self
    }

    #[must_use]
    pub fn drops(mut self, drops: &[&str]) -> Self {
        self.drops = drops.iter().map(|drop| (*drop).to_owned()).collect();
        self
    }

    #[must_use]
    pub fn tool_requirement(mut self, tool: &str, tier: &str, required: bool) -> Self {
        self.tool = ToolRequirement {
            tool: tool.to_owned(),
            tier: tier.to_owned(),
            required,
        };
        self
    }

    #[must_use]
    pub fn properties(mut self, properties: &[BlockProperty]) -> Self {
        self.properties = properties.to_vec();
        self
    }

    #[must_use]
    pub fn behavior(mut self, behavior: BlockBehaviour) -> Self {
        self.behavior = behavior;
        self
    }

    /// Validates the chain and declares the block in [`crate::blocks`].
    ///
    /// `Err` names the field and the offending value, and nothing is registered.
    #[must_use = "a refusal names the field, and nothing is registered"]
    pub fn register(self) -> Result<String, BuildError> {
        let id = self.id.trim();
        if id.is_empty() {
            return Err(BuildError::missing("id"));
        }
        if self.hardness < 0.0 {
            return Err(BuildError::out_of_range("hardness", &self.hardness.to_string()));
        }
        if self.resistance < 0.0 {
            return Err(BuildError::out_of_range("resistance", &self.resistance.to_string()));
        }
        if self.light_level > 15 {
            return Err(BuildError::out_of_range("light_level", &self.light_level.to_string()));
        }
        if crate::blocks::get_type(id).is_some() {
            return Err(BuildError::duplicate(id));
        }
        let name = if self.name.is_empty() { id } else { self.name.as_str() };
        let drops: Vec<&str> = self.drops.iter().map(String::as_str).collect();
        let shapes = BlockShapes {
            collision: self.collision,
            outline: self.outline,
            ..BlockShapes::default()
        };
        let mut description = BlockDescription::new(id, name)
            .material(sound_material(&self.sound))
            .hardness(self.hardness)
            .resistance(self.resistance)
            .light_level(self.light_level)
            .shapes(shapes)
            .drops(&drops)
            .tool(&self.tool.tool, &self.tool.tier, self.tool.required)
            .behaviour(self.behavior.clone());
        for property in self.properties {
            description = description.property(property);
        }
        crate::blocks::register(description);
        if !self.texture.is_empty() {
            crate::textures::register(crate::textures::Texture::new(
                &self.texture,
                1,
                1,
                crate::textures::TextureFormat::Rgba8,
            ));
        }
        if !self.model.is_empty() {
            crate::models::register(crate::models::Model::new(&self.model));
        }
        Ok(crate::blocks::get_registry_id(id).unwrap_or_else(|| id.to_owned()))
    }

    /// [`BlockBuilder::register`] under the name a caller reaches for first.
    #[must_use = "a refusal names the field, and nothing is registered"]
    pub fn build(self) -> Result<String, BuildError> {
        self.register()
    }
}

/// A fluent chain for an item, ending in [`ItemBuilder::register`].
#[derive(Debug, Clone, PartialEq)]
pub struct ItemBuilder {
    id: String,
    name: String,
    max_stack_size: u32,
    durability: Option<i32>,
    texture: String,
    model: String,
    rarity: String,
    lore: Vec<String>,
    enchantability: Option<i32>,
    food: Vec<Component>,
    armor: Vec<Component>,
    tool: Vec<Component>,
    use_behavior: String,
    cooldown: u32,
    attributes: Vec<Component>,
    components: Vec<Component>,
}

impl Default for ItemBuilder {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            max_stack_size: crate::items::DEFAULT_MAX_COUNT,
            durability: None,
            texture: String::new(),
            model: String::new(),
            rarity: String::new(),
            lore: Vec::new(),
            enchantability: None,
            food: Vec::new(),
            armor: Vec::new(),
            tool: Vec::new(),
            use_behavior: String::new(),
            cooldown: 0,
            attributes: Vec::new(),
            components: Vec::new(),
        }
    }
}

impl ItemBuilder {
    /// An item with the defaults a caller would expect.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }

    #[must_use]
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    #[must_use]
    pub fn max_stack_size(mut self, max_stack_size: u32) -> Self {
        self.max_stack_size = max_stack_size;
        self
    }

    #[must_use]
    pub fn durability(mut self, durability: i32) -> Self {
        self.durability = Some(durability);
        self
    }

    #[must_use]
    pub fn texture(mut self, texture: &str) -> Self {
        self.texture = texture.to_owned();
        self
    }

    #[must_use]
    pub fn model(mut self, model: &str) -> Self {
        self.model = model.to_owned();
        self
    }

    #[must_use]
    pub fn rarity(mut self, rarity: &str) -> Self {
        self.rarity = rarity.to_owned();
        self
    }

    #[must_use]
    pub fn lore(mut self, lore: &[&str]) -> Self {
        self.lore = lore.iter().map(|line| (*line).to_owned()).collect();
        self
    }

    #[must_use]
    pub fn enchantability(mut self, enchantability: i32) -> Self {
        self.enchantability = Some(enchantability);
        self
    }

    #[must_use]
    pub fn food(mut self, food: &[Component]) -> Self {
        self.food = food.to_vec();
        self
    }

    #[must_use]
    pub fn armor(mut self, armor: &[Component]) -> Self {
        self.armor = armor.to_vec();
        self
    }

    #[must_use]
    pub fn tool(mut self, tool: &[Component]) -> Self {
        self.tool = tool.to_vec();
        self
    }

    #[must_use]
    pub fn use_behavior(mut self, behavior: &str) -> Self {
        self.use_behavior = behavior.to_owned();
        self
    }

    #[must_use]
    pub fn cooldown(mut self, cooldown: u32) -> Self {
        self.cooldown = cooldown;
        self
    }

    #[must_use]
    pub fn attributes(mut self, attributes: &[Component]) -> Self {
        self.attributes = attributes.to_vec();
        self
    }

    #[must_use]
    pub fn components(mut self, components: &[Component]) -> Self {
        self.components = components.to_vec();
        self
    }

    /// Validates the chain and declares the item in [`crate::items`].
    #[must_use = "a refusal names the field, and nothing is registered"]
    pub fn register(self) -> Result<String, BuildError> {
        let id = self.id.trim();
        if id.is_empty() {
            return Err(BuildError::missing("id"));
        }
        if self.max_stack_size == 0 || self.max_stack_size > crate::items::DEFAULT_MAX_COUNT {
            return Err(BuildError::out_of_range(
                "max_stack_size",
                &self.max_stack_size.to_string(),
            ));
        }
        if let Some(durability) = self.durability {
            if durability < 0 {
                return Err(BuildError::out_of_range("durability", &durability.to_string()));
            }
        }
        if let Some(enchantability) = self.enchantability {
            if !(0..=30).contains(&enchantability) {
                return Err(BuildError::out_of_range(
                    "enchantability",
                    &enchantability.to_string(),
                ));
            }
        }
        if crate::items::get(id).is_some() {
            return Err(BuildError::duplicate(id));
        }
        let name = if self.name.is_empty() { id } else { self.name.as_str() };
        let mut item = ItemType::new(id, name)
            .max_count(self.max_stack_size)
            .icon(&self.texture)
            .use_action(&self.use_behavior, 0)
            .cooldown(self.cooldown)
            .food(&self.food)
            .armour(&self.armor)
            .tool(&self.tool);
        if let Some(durability) = self.durability {
            item = item.durability(durability);
        }
        if !self.lore.is_empty() {
            let lines: Vec<&str> = self.lore.iter().map(String::as_str).collect();
            item = item.component(Component::lore(&lines));
        }
        if !self.model.is_empty() {
            item = item.component(Component::new("minecraft:model", &self.model));
        }
        if !self.rarity.is_empty() {
            item = item.component(Component::new("minecraft:rarity", &self.rarity));
        }
        if let Some(enchantability) = self.enchantability {
            item = item.component(Component::new(
                "minecraft:enchantability",
                &enchantability.to_string(),
            ));
        }
        for component in self.attributes.into_iter().chain(self.components) {
            item = item.component(component);
        }
        crate::items::register(item);
        Ok(crate::items::get_registry_id(id).unwrap_or_else(|| id.to_owned()))
    }

    /// [`ItemBuilder::register`] under the name a caller reaches for first.
    #[must_use = "a refusal names the field, and nothing is registered"]
    pub fn build(self) -> Result<String, BuildError> {
        self.register()
    }
}

/// A fluent chain for an entity, ending in [`EntityBuilder::register`].
#[derive(Debug, Clone, PartialEq)]
pub struct EntityBuilder {
    id: String,
    name: String,
    dimensions: (f32, f32),
    health: f32,
    attributes: Vec<(String, f32)>,
    movement: f32,
    collision: String,
    ai: String,
    loot: String,
    spawn_rules: String,
    renderer: String,
    model: String,
    sounds: String,
    components: Vec<EntityComponent>,
}

impl Default for EntityBuilder {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            dimensions: (0.6, 1.8),
            health: 20.0,
            attributes: Vec::new(),
            movement: 0.25,
            collision: String::new(),
            ai: String::new(),
            loot: String::new(),
            spawn_rules: String::new(),
            renderer: String::new(),
            model: String::new(),
            sounds: String::new(),
            components: Vec::new(),
        }
    }
}

impl EntityBuilder {
    /// An entity with the defaults a caller would expect.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }

    #[must_use]
    pub fn name(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    #[must_use]
    pub fn dimensions(mut self, width: f32, height: f32) -> Self {
        self.dimensions = (width, height);
        self
    }

    #[must_use]
    pub fn health(mut self, health: f32) -> Self {
        self.health = health;
        self
    }

    #[must_use]
    pub fn attributes(mut self, attributes: &[(&str, f32)]) -> Self {
        self.attributes = attributes
            .iter()
            .map(|(name, value)| ((*name).to_owned(), *value))
            .collect();
        self
    }

    #[must_use]
    pub fn movement(mut self, movement: f32) -> Self {
        self.movement = movement;
        self
    }

    #[must_use]
    pub fn collision(mut self, collision: &str) -> Self {
        self.collision = collision.to_owned();
        self
    }

    #[must_use]
    pub fn ai(mut self, ai: &str) -> Self {
        self.ai = ai.to_owned();
        self
    }

    #[must_use]
    pub fn loot(mut self, loot: &str) -> Self {
        self.loot = loot.to_owned();
        self
    }

    #[must_use]
    pub fn spawn_rules(mut self, spawn_rules: &str) -> Self {
        self.spawn_rules = spawn_rules.to_owned();
        self
    }

    #[must_use]
    pub fn renderer(mut self, renderer: &str) -> Self {
        self.renderer = renderer.to_owned();
        self
    }

    #[must_use]
    pub fn model(mut self, model: &str) -> Self {
        self.model = model.to_owned();
        self
    }

    #[must_use]
    pub fn sounds(mut self, sounds: &str) -> Self {
        self.sounds = sounds.to_owned();
        self
    }

    #[must_use]
    pub fn components(mut self, components: &[EntityComponent]) -> Self {
        self.components = components.to_vec();
        self
    }

    /// Validates the chain and declares the entity in [`crate::entities`].
    #[must_use = "a refusal names the field, and nothing is registered"]
    pub fn register(self) -> Result<String, BuildError> {
        let id = self.id.trim();
        if id.is_empty() {
            return Err(BuildError::missing("id"));
        }
        if self.health < 0.0 {
            return Err(BuildError::out_of_range("health", &self.health.to_string()));
        }
        if self.dimensions.0 <= 0.0 || self.dimensions.1 <= 0.0 {
            return Err(BuildError::out_of_range(
                "dimensions",
                &format!("{}x{}", self.dimensions.0, self.dimensions.1),
            ));
        }
        if self.movement < 0.0 {
            return Err(BuildError::out_of_range("movement", &self.movement.to_string()));
        }
        if crate::entities::get_by_id(id).is_some() {
            return Err(BuildError::duplicate(id));
        }
        let name = if self.name.is_empty() { id } else { self.name.as_str() };
        let mut entity = EntityType::new(id, name)
            .size(self.dimensions.0, self.dimensions.1)
            .health(self.health)
            .movement(self.movement);
        for (field, value) in self.attributes {
            entity = entity.attribute(&field, value);
        }
        for component in self.components {
            entity = entity.component(component);
        }
        // Bedrock carries the rest of an entity as components, which is where these belong.
        let extras = [
            ("minecraft:collision_box", self.collision),
            ("minecraft:ai", self.ai),
            ("minecraft:loot", self.loot),
            ("minecraft:spawn_rules", self.spawn_rules),
            ("minecraft:renderer", self.renderer),
            ("minecraft:model", self.model),
            ("minecraft:sounds", self.sounds),
        ];
        for (field, value) in extras {
            if !value.is_empty() {
                entity = entity.component(EntityComponent::new(field, &value));
            }
        }
        crate::entities::register_type(entity);
        Ok(crate::entities::get_registry_id(id).unwrap_or_else(|| id.to_owned()))
    }

    /// [`EntityBuilder::register`] under the name a caller reaches for first.
    #[must_use = "a refusal names the field, and nothing is registered"]
    pub fn build(self) -> Result<String, BuildError> {
        self.register()
    }
}

/// Why the client's own registries cannot take a new entry.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "block" => {
            "a block is the client's own Blocks registry, frozen at startup; a new block \
             cannot be added at runtime"
                .to_owned()
        }
        "item" => {
            "an item is the client's own Items registry, frozen at startup; a new item cannot \
             be added at runtime"
                .to_owned()
        }
        "entity" => {
            "an entity is the client's own Entities registry, frozen at startup; a new entity \
             cannot be added at runtime"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// Every kind a builder would have to hand to the client, with the reason it cannot.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["block", "item", "entity"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Asks the client to take a new block, item or entity at runtime, which it cannot: warns and
/// answers `false`.
pub fn register_with_the_client(kind: &str, id: &str) -> bool {
    crate::warn!(
        "a mod asked this host to register the {kind} {id} with the client: {}",
        why_unknown(kind)
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{blocks, entities, items, models, textures};

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn clear(ids: &[&str]) {
        for id in ids {
            crate::blocks::unregister(id);
            crate::items::unregister(id);
            crate::entities::unregister_type(id);
        }
    }

    #[test]
    fn a_fully_chained_block_registers_and_reads_back() {
        let _serial = serial();
        clear(&["s78:lantern"]);
        let registry_id = BlockBuilder::new()
            .id("s78:lantern")
            .name("Lantern")
            .hardness(1.5)
            .resistance(3.0)
            .sound("metal")
            .light_level(12)
            .collision((0.0, 0.0, 0.0, 1.0, 1.0, 1.0))
            .outline((0.0, 0.0, 0.0, 1.0, 1.0, 1.0))
            .texture("s78:lantern_tex")
            .model("s78:lantern_model")
            .drops(&["s78:lantern"])
            .tool_requirement("pickaxe", "iron", true)
            .properties(&[BlockProperty::facing(&["north", "south"])])
            .behavior(BlockBehaviour {
                on_break: Some(|| {}),
                ..BlockBehaviour::default()
            })
            .register()
            .expect("every field is in range and the id is free");
        assert!(registry_id.ends_with(":s78:lantern"), "{registry_id}");
        assert_eq!(
            blocks::get_type("s78:lantern").map(|block| block.name),
            Some("Lantern".to_owned())
        );
        assert_eq!(blocks::get_hardness("s78:lantern"), Some(1.5));
        assert_eq!(blocks::get_light_level("s78:lantern"), Some(12));
        assert_eq!(blocks::get_resistance("s78:lantern"), Some(3.0));
        assert_eq!(blocks::get_sound("s78:lantern"), Some("metal"));
        assert_eq!(blocks::get_drops("s78:lantern"), vec!["s78:lantern".to_owned()]);
        assert_eq!(
            blocks::get_tool_requirement("s78:lantern").map(|tool| tool.tool),
            Some("pickaxe".to_owned())
        );
        assert_eq!(
            blocks::get_collision_shape("s78:lantern"),
            Some((0.0, 0.0, 0.0, 1.0, 1.0, 1.0))
        );
        assert_eq!(blocks::get_properties("s78:lantern").len(), 1);
        assert_eq!(blocks::behaviour::callbacks("s78:lantern"), vec!["on_break"]);
        assert!(textures::get("s78:lantern_tex").is_some());
        assert!(models::get("s78:lantern_model").is_some());
        clear(&["s78:lantern"]);
        textures::unregister("s78:lantern_tex");
        models::unregister("s78:lantern_model");
    }

    #[test]
    fn a_fully_chained_item_registers_and_reads_back() {
        let _serial = serial();
        clear(&["s78:gem"]);
        ItemBuilder::new()
            .id("s78:gem")
            .name("Gem")
            .max_stack_size(16)
            .durability(64)
            .texture("s78:gem_icon")
            .model("s78:gem_model")
            .rarity("rare")
            .lore(&["a chip of glass", "warm to hold"])
            .enchantability(20)
            .food(&[Component::new("minecraft:nutrition", "4")])
            .armor(&[Component::new("minecraft:armor_slot", "head")])
            .tool(&[Component::new("minecraft:mining_speed", "2")])
            .use_behavior("eat")
            .cooldown(8)
            .attributes(&[Component::new("minecraft:attack_damage", "3")])
            .components(&[Component::new("minecraft:foil", "true")])
            .register()
            .expect("in range and free");
        assert_eq!(items::get("s78:gem").map(|item| item.name), Some("Gem".to_owned()));
        assert_eq!(items::get_max_stack_size("s78:gem"), Some(16));
        assert_eq!(items::get_durability("s78:gem"), Some(64));
        assert_eq!(items::get_texture("s78:gem"), Some("s78:gem_icon".to_owned()));
        assert_eq!(items::get_use_action("s78:gem"), Some("eat".to_owned()));
        assert_eq!(items::get_cooldown("s78:gem"), Some(8));
        assert_eq!(
            items::get_component("s78:gem", "minecraft:rarity").map(|component| component.value),
            Some("rare".to_owned())
        );
        assert_eq!(
            items::get_component("s78:gem", "minecraft:model").map(|component| component.value),
            Some("s78:gem_model".to_owned())
        );
        assert_eq!(
            items::get_component("s78:gem", "minecraft:lore")
                .map(|component| component.value.lines().count()),
            Some(2)
        );
        assert_eq!(
            items::get_component("s78:gem", "minecraft:enchantability")
                .map(|component| component.value),
            Some("20".to_owned())
        );
        assert!(items::get_component("s78:gem", "minecraft:attack_damage").is_some());
        assert!(items::get_component("s78:gem", "minecraft:foil").is_some());
        assert_eq!(items::get_food_properties("s78:gem").len(), 1);
        assert_eq!(items::is_edible("s78:gem"), Some(true));
        assert_eq!(items::get_armour_properties("s78:gem").len(), 1);
        assert_eq!(items::is_equippable("s78:gem"), Some(true));
        clear(&["s78:gem"]);
    }

    #[test]
    fn a_fully_chained_entity_registers_and_reads_back() {
        let _serial = serial();
        clear(&["s78:wisp"]);
        EntityBuilder::new()
            .id("s78:wisp")
            .name("Wisp")
            .dimensions(0.6, 1.2)
            .health(8.0)
            .movement(0.3)
            .attributes(&[("minecraft:movement", 0.3), ("minecraft:armor", 2.0)])
            .collision("0.6,1.2")
            .ai("wander")
            .loot("s78:wisp_loot")
            .spawn_rules("light,overworld")
            .renderer("s78:wisp_renderer")
            .model("s78:wisp_model")
            .sounds("s78:wisp_sound")
            .components(&[EntityComponent::new("minecraft:physics", "true")])
            .register()
            .expect("in range and free");
        let wisp = entities::get_by_id("s78:wisp").expect("registered");
        assert_eq!(wisp.name, "Wisp");
        assert_eq!(wisp.size.width, 0.6);
        assert_eq!(wisp.size.height, 1.2);
        assert_eq!(entities::get_max_health("s78:wisp"), Some(8.0));
        assert_eq!(entities::get_attributes("s78:wisp").len(), 2);
        assert_eq!(entities::get_attribute("s78:wisp", "minecraft:movement"), Some(0.3));
        let components = entities::get_components("s78:wisp");
        assert!(components.iter().any(|component| component.name == "minecraft:physics"));
        assert!(components.iter().any(|component| component.name == "minecraft:collision_box"));
        assert!(components.iter().any(|component| component.name == "minecraft:ai"));
        assert!(components.iter().any(|component| component.name == "minecraft:sounds"));
        clear(&["s78:wisp"]);
    }

    #[test]
    fn a_build_with_no_id_is_refused() {
        let _serial = serial();
        let error = BlockBuilder::new().hardness(2.0).register().expect_err("no id");
        assert_eq!(error.kind(), BuildErrorKind::Missing);
        assert_eq!(error.field(), "id");
        assert_eq!(error.to_string(), "the id is missing");
        let error = ItemBuilder::new().id("   ").register().expect_err("a blank id");
        assert_eq!(error.kind(), BuildErrorKind::Missing);
        let error = EntityBuilder::new().register().expect_err("no id");
        assert_eq!(error.kind(), BuildErrorKind::Missing);
    }

    #[test]
    fn a_duplicate_id_is_refused() {
        let _serial = serial();
        clear(&["s78:dupe"]);
        BlockBuilder::new().id("s78:dupe").register().expect("the first is free");
        let error = BlockBuilder::new().id("s78:dupe").register().expect_err("the same id again");
        assert_eq!(error.kind(), BuildErrorKind::Duplicate);
        assert_eq!(error.field(), "id");
        assert!(error.to_string().contains("s78:dupe"), "{error}");
        clear(&["s78:dupe"]);
    }

    #[test]
    fn an_out_of_range_value_is_refused_naming_the_field() {
        let _serial = serial();
        clear(&["s78:bad"]);
        let error = BlockBuilder::new()
            .id("s78:bad")
            .hardness(-2.5)
            .register()
            .expect_err("a negative hardness");
        assert_eq!(error.kind(), BuildErrorKind::OutOfRange);
        assert_eq!(error.field(), "hardness");
        assert_eq!(error.value(), "-2.5");
        assert!(error.to_string().contains("hardness"), "{error}");
        assert!(error.to_string().contains("-2.5"), "{error}");
        assert!(blocks::get_type("s78:bad").is_none(), "nothing was registered");

        let error = BlockBuilder::new()
            .id("s78:bad")
            .light_level(16)
            .register()
            .expect_err("above fifteen");
        assert_eq!(error.field(), "light_level");
        assert_eq!(error.value(), "16");

        let error = ItemBuilder::new()
            .id("s78:bad")
            .max_stack_size(200)
            .register()
            .expect_err("above a stack");
        assert_eq!(error.field(), "max_stack_size");
        assert_eq!(error.value(), "200");

        let error = EntityBuilder::new()
            .id("s78:bad")
            .health(-1.0)
            .register()
            .expect_err("a negative health");
        assert_eq!(error.field(), "health");
        assert_eq!(error.value(), "-1");
    }

    #[test]
    fn a_loop_of_five_distinct_builds_registers_five_and_a_duplicate_is_the_only_failure() {
        let _serial = serial();
        let ids: Vec<String> = (0..5).map(|i| format!("s78:loop{i}")).collect();
        for id in &ids {
            crate::blocks::unregister(id);
        }
        for (i, id) in ids.iter().enumerate() {
            let built = BlockBuilder::new()
                .id(id)
                .hardness(i as f32)
                .light_level((i * 2) as u8)
                .register();
            assert!(built.is_ok(), "{id} should register: {built:?}");
        }
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(blocks::get_hardness(id), Some(i as f32));
            assert_eq!(blocks::get_light_level(id), Some((i * 2) as u8));
        }
        let duplicate = BlockBuilder::new().id(&ids[2]).register().expect_err("the third is taken");
        assert_eq!(duplicate.kind(), BuildErrorKind::Duplicate);
        for id in &ids {
            crate::blocks::unregister(id);
        }
    }

    #[test]
    fn the_client_registries_are_refused_with_a_sentence_each() {
        let _serial = serial();
        assert!(!register_with_the_client("block", "s78:client_block"));
        assert!(!register_with_the_client("item", "s78:client_item"));
        assert!(!register_with_the_client("entity", "s78:client_entity"));
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_eq!(
            why_unknown("nothing"),
            "nothing is not something this host can see"
        );
    }

    #[test]
    fn defaults_hold_and_build_is_register() {
        let _serial = serial();
        clear(&["s78:plain", "s78:plain_item", "s78:plain_entity"]);
        assert_eq!(
            BlockBuilder::new().build().expect_err("still no id").kind(),
            BuildErrorKind::Missing
        );
        let id = BlockBuilder::new().id("s78:plain").build().expect("an id is enough");
        assert!(id.ends_with(":s78:plain"), "{id}");
        assert_eq!(blocks::get_hardness("s78:plain"), Some(1.0));
        assert_eq!(blocks::get_light_level("s78:plain"), Some(0));
        assert_eq!(blocks::get_sound("s78:plain"), Some("stone"));
        assert_eq!(
            blocks::get_type("s78:plain").map(|block| block.name),
            Some("s78:plain".to_owned()),
            "the name falls back to the id"
        );

        ItemBuilder::new().id("s78:plain_item").build().expect("defaults");
        assert_eq!(items::get_max_stack_size("s78:plain_item"), Some(64));

        EntityBuilder::new().id("s78:plain_entity").build().expect("defaults");
        assert_eq!(entities::get_max_health("s78:plain_entity"), Some(20.0));
        clear(&["s78:plain", "s78:plain_item", "s78:plain_entity"]);
    }
}
