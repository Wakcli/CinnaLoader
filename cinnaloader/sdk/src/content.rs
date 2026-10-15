//! Content registries: one front door over the registries this crate already owns.
//!
//! The wishlist spells forty-seven `register_*` calls with no module prefix. They all live
//! here. Each one declares its kind *and* records it in the module that owns that kind, so
//! there is no parallel table to drift: a block goes to [`crate::blocks`], an item to
//! [`crate::items`], a recipe to [`crate::recipes`], an effect to [`crate::effects`], and so
//! on down the list in [`declare`]. Reading one back through its owning module works exactly
//! as if the mod had called that module directly.
//!
//! Eight kinds have no owning registry in this crate — `menu`, `attribute`,
//! `world_generator` aside, `font`, `sound_event`, `particle_renderer`, `game_rule` and
//! `advancement`. Those are kept here alone, and [`ContentKind::module`] answers `"content"`
//! for them.
//!
//! What is refused is the client's *own* registries: its block, item and entity lists, and
//! its inability to take a new entry at runtime. [`register_with_the_client`] warns and
//! answers `false` with a sentence per kind, and [`why_unknown`] and [`refusals`] say the
//! same without warning.
//!
//! ```
//! use cinnaloader::prelude::*;
//! use cinnaloader::content::{self, ContentKind};
//!
//! assert!(content::register_block("hello:s59_block"));
//! assert!(!content::register_block("hello:s59_block"), "per id, once");
//! assert!(content::count(ContentKind::Block) >= 1);
//! assert!(blocks::get_type("hello:s59_block").is_some());
//! assert_eq!(content::get(ContentKind::Block, "hello:s59_block").map(|e| e.owner), Some(id()));
//! assert!(!content::register_with_the_client(ContentKind::Item, "hello:gem"));
//! ```

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

macro_rules! content_kinds {
    ($($variant:ident => $name:literal, $module:literal, $client:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum ContentKind { $($variant),* }

        impl ContentKind {
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $name),* }
            }

            /// The module that owns this kind, or `"content"` when nothing else does.
            #[must_use]
            pub fn module(self) -> &'static str {
                match self { $(Self::$variant => $module),* }
            }

            /// The name of the client's own registry for this kind, or `""` when it has none.
            #[must_use]
            pub fn client_registry(self) -> &'static str {
                match self { $(Self::$variant => $client),* }
            }

            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                match name { $($name => Some(Self::$variant),)* _ => None }
            }

            /// Every kind this module covers.
            #[must_use]
            pub fn all() -> Vec<Self> {
                vec![$(Self::$variant),*]
            }
        }
    };
}

content_kinds! {
    Biome => "biome", "worldgen", "Biomes";
    Block => "block", "blocks", "Blocks";
    Item => "item", "items", "Items";
    Entity => "entity", "entities", "Entities";
    Effect => "effect", "effects", "Effects";
    Enchantment => "enchantment", "enchantments", "Enchantments";
    Particle => "particle", "particles", "Particles";
    Sound => "sound", "audio", "Sounds";
    Screen => "screen", "ui", "Screens";
    Menu => "menu", "content", "Menus";
    Command => "command", "commands", "Commands";
    Keybind => "keybind", "input", "KeyBindings";
    Recipe => "recipe", "recipes", "Recipes";
    RecipeSerializer => "recipe_serializer", "recipes", "RecipeSerializers";
    BlockEntity => "block_entity", "block_entities", "BlockEntities";
    Dimension => "dimension", "worldgen", "Dimensions";
    Fluid => "fluid", "fluids", "FluidTypes";
    Attribute => "attribute", "content", "Attributes";
    DamageType => "damage_type", "effects", "DamageTypes";
    LootTable => "loot_table", "loot", "LootTables";
    Feature => "feature", "worldgen", "Features";
    Structure => "structure", "structures", "Structures";
    WorldGenerator => "world_generator", "worldgen", "WorldGenerators";
    Shader => "shader", "shaders", "Shaders";
    Model => "model", "models", "";
    Texture => "texture", "textures", "";
    Font => "font", "content", "";
    Animation => "animation", "models", "";
    RenderLayer => "render_layer", "render", "RenderLayers";
    EntityRenderer => "entity_renderer", "models", "";
    BlockRenderer => "block_renderer", "models", "";
    ItemRenderer => "item_renderer", "models", "";
    ParticleRenderer => "particle_renderer", "particles", "";
    SoundEvent => "sound_event", "content", "Sounds";
    CustomComponent => "custom_component", "components", "Components";
    CustomDataType => "custom_data_type", "data", "DataSerializers";
    CustomNetworkChannel => "custom_network_channel", "channels", "";
    AiGoal => "ai_goal", "ai", "AIBehaviors";
    AiSensor => "ai_sensor", "ai", "AIBehaviors";
    Behavior => "behavior", "ai", "AIBehaviors";
    GameRule => "game_rule", "content", "GameRules";
    Statistic => "statistic", "statistics", "Statistics";
    Advancement => "advancement", "content", "Advancements";
    TradeOffer => "trade_offer", "trading", "";
    TradeProfession => "trade_profession", "trading", "";
    BrewingRecipe => "brewing_recipe", "recipes", "Recipes";
    FuelType => "fuel_type", "recipes", "";
}

/// Why the client cannot accept this kind. One sentence per kind.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match ContentKind::from_name(what) {
        Some(kind) => kind.refusal(),
        None => format!("{what} is not something this host can see"),
    }
}

impl ContentKind {
    /// The refusal long form, used by [`why_unknown`] and [`refusals`].
    #[must_use]
    pub fn refusal(self) -> String {
        use ContentKind::*;
        match self {
            Biome => "a biome is the client's own Biomes registry, built before a world loads, and it cannot take a new one at runtime".to_owned(),
            Block => "a block is the client's own Blocks registry: its list is fixed at startup, so a new block cannot be added at runtime".to_owned(),
            Item => "an item is the client's own Items registry, fixed at startup, so a new item cannot be added at runtime".to_owned(),
            Entity => "an entity is the client's own Entities registry, fixed at startup, so a new entity cannot be added at runtime".to_owned(),
            Effect => "an effect is the client's own Effects registry, filled before the world loads".to_owned(),
            Enchantment => "an enchantment is the client's own Enchantments registry, which no mod may extend at runtime".to_owned(),
            Particle => "a particle is the client's own Particles registry, which it builds from its own assets".to_owned(),
            Sound => "a sound resource is the client's own Sounds registry, loaded from its own asset packs".to_owned(),
            Screen => "a screen is the client's own Screens registry; it opens only screens it already knows".to_owned(),
            Menu => "a menu is a client-side container screen the client dictates, with no registry to add to".to_owned(),
            Command => "a command is the client's own Commands registry, which it owns".to_owned(),
            Keybind => "a key binding is the client's own KeyBindings registry, filled before it accepts input".to_owned(),
            Recipe => "a recipe is the client's own Recipes registry, read from the server it connects to".to_owned(),
            RecipeSerializer => "a recipe serializer is the client's own RecipeSerializers registry".to_owned(),
            BlockEntity => "a block entity type is the client's own BlockEntities registry".to_owned(),
            Dimension => "a dimension is the client's own Dimensions registry, built with the world generator".to_owned(),
            Fluid => "a fluid type is the client's own FluidTypes registry".to_owned(),
            Attribute => "an attribute is the client's own Attributes registry, which it fills at startup".to_owned(),
            DamageType => "a damage type is the client's own DamageTypes registry".to_owned(),
            LootTable => "a loot table is the client's own LootTables registry, replaced by the server's data".to_owned(),
            Feature => "a feature is the client's own Features registry, chosen by the world generator".to_owned(),
            Structure => "a structure is the client's own Structures registry, part of world generation".to_owned(),
            WorldGenerator => "a world generator is the client's own WorldGenerators registry, chosen before a world loads".to_owned(),
            Shader => "a shader is the client's own Shaders registry, compiled from its own assets".to_owned(),
            Model => "a model is the client's own geometry asset, not a runtime registry it will accept".to_owned(),
            Texture => "a texture is the client's own asset; a new one would have to be in its packs at startup".to_owned(),
            Font => "a font is the client's own asset, loaded with its resource packs".to_owned(),
            Animation => "an animation is the client's own asset, played only for geometry it already has".to_owned(),
            RenderLayer => "a render layer is the client's own RenderLayers registry".to_owned(),
            EntityRenderer => "an entity renderer is chosen by the client for its own entity types".to_owned(),
            BlockRenderer => "a block renderer is chosen by the client for its own blocks".to_owned(),
            ItemRenderer => "an item renderer is chosen by the client for its own items".to_owned(),
            ParticleRenderer => "a particle renderer is drawn by the client for its own particles".to_owned(),
            SoundEvent => "a sound event is the client's own Sounds registry, keyed by event name".to_owned(),
            CustomComponent => "an item component is the client's own Components registry, decoded from the wire".to_owned(),
            CustomDataType => "a data serializer is the client's own DataSerializers registry".to_owned(),
            CustomNetworkChannel => "a network channel is the client's own network, which no mod may open at runtime".to_owned(),
            AiGoal => "an AI goal is the client's own AIBehaviors registry".to_owned(),
            AiSensor => "an AI sensor is the client's own AIBehaviors registry".to_owned(),
            Behavior => "an AI behaviour is the client's own AIBehaviors registry".to_owned(),
            GameRule => "a game rule is the client's own GameRules registry, owned by its world".to_owned(),
            Statistic => "a statistic is the client's own Statistics registry".to_owned(),
            Advancement => "an advancement is the client's own Advancements registry, read from the server".to_owned(),
            TradeOffer => "a trade offer is the client's own villager data".to_owned(),
            TradeProfession => "a trade profession is the client's own villager data".to_owned(),
            BrewingRecipe => "a brewing recipe is the client's own Recipes registry".to_owned(),
            FuelType => "a fuel type is the client's own Recipes and item data".to_owned(),
        }
    }
}

/// One registered thing: what it is, what it is called and who declared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: ContentKind,
    pub id: String,
    pub owner: String,
}

impl Entry {
    /// The module that owns this kind.
    #[must_use]
    pub fn module(&self) -> &'static str {
        self.kind.module()
    }
}

impl std::fmt::Display for Entry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} {} ({} by {})", self.kind.as_str(), self.id, self.module(), self.owner)
    }
}

fn index() -> &'static Mutex<BTreeMap<(ContentKind, String), Entry>> {
    static INDEX: OnceLock<Mutex<BTreeMap<(ContentKind, String), Entry>>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<(ContentKind, String), Entry>) -> T) -> T {
    let mut guard = index().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a kind and records it in the module that owns it. `false` when the id is taken.
pub fn register(kind: ContentKind, id: &str) -> bool {
    let taken = with(|index| {
        if index.contains_key(&(kind, id.to_owned())) {
            return true;
        }
        index.insert(
            (kind, id.to_owned()),
            Entry {
                kind,
                id: id.to_owned(),
                owner: crate::metadata::id(),
            },
        );
        false
    });
    if taken {
        return false;
    }
    declare(kind, id);
    true
}

/// Forgets a kind, in this index and in the module that owns it.
pub fn unregister(kind: ContentKind, id: &str) -> bool {
    let had = with(|index| index.remove(&(kind, id.to_owned())).is_some());
    if !had {
        return false;
    }
    forget(kind, id);
    true
}

/// One registered thing, by kind and id.
#[must_use]
pub fn get(kind: ContentKind, id: &str) -> Option<Entry> {
    with(|index| index.get(&(kind, id.to_owned())).cloned())
}

/// Every registered thing, by kind and then id.
#[must_use]
pub fn all() -> Vec<Entry> {
    with(|index| index.values().cloned().collect())
}

/// Every registered thing of one kind.
#[must_use]
pub fn all_of(kind: ContentKind) -> Vec<Entry> {
    with(|index| {
        index
            .values()
            .filter(|entry| entry.kind == kind)
            .cloned()
            .collect()
    })
}

/// How many of one kind are registered.
#[must_use]
pub fn count(kind: ContentKind) -> usize {
    with(|index| index.values().filter(|entry| entry.kind == kind).count())
}

/// The kinds that have at least one entry, in kind order.
#[must_use]
pub fn kinds() -> Vec<ContentKind> {
    let mut kinds: Vec<ContentKind> = with(|index| index.keys().map(|(kind, _)| *kind).collect());
    kinds.sort();
    kinds.dedup();
    kinds
}

/// Every refusal, as `(kind, sentence)`.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ContentKind::all()
        .into_iter()
        .map(|kind| (kind.as_str().to_owned(), kind.refusal()))
        .collect()
}

/// Asks the client to accept a new one at runtime, which it cannot: warns and answers `false`.
pub fn register_with_the_client(kind: ContentKind, id: &str) -> bool {
    crate::warn!(
        "a mod asked this host to register {} '{}' with the client: {}",
        kind.as_str(),
        id,
        kind.refusal()
    );
    false
}

fn declare(kind: ContentKind, id: &str) -> bool {
    use ContentKind::*;
    match kind {
        Biome => crate::worldgen::register_biome(crate::worldgen::Biome::new(id, id)),
        Block => crate::blocks::register(crate::blocks::BlockDescription::new(id, id)),
        Item => crate::items::register(crate::items::ItemType::new(id, id)),
        Entity => crate::entities::register_type(crate::entities::EntityType::new(id, id)),
        Effect => crate::effects::register(crate::effects::EffectType::new(id, id)),
        Enchantment => crate::enchantments::register(crate::enchantments::Enchantment::new(id, id)),
        Particle => crate::particles::register(crate::particles::ParticleType::new(id)),
        Sound => crate::audio::register_sound(crate::audio::Sound::new(id, id)),
        Screen => crate::ui::register_screen(crate::ui::Screen::new(id, id)),
        Command => crate::commands::register_root_command(crate::commands::Command::new(id, id)),
        Keybind => crate::input::register_keybind(id, "key.keyboard.unknown"),
        Recipe => crate::recipes::register(crate::recipes::shaped(id, id, 1)),
        RecipeSerializer => crate::recipes::register_serializer(id),
        BlockEntity => {
            crate::block_entities::register_type(crate::block_entities::BlockEntityType::new(id, id))
        }
        Dimension => crate::worldgen::register_dimension(crate::worldgen::GenerationPiece::new(
            crate::worldgen::PieceKind::Dimension,
            id,
        )),
        Fluid => crate::fluids::register(crate::fluids::Fluid::new(id, id)),
        DamageType => crate::effects::damage::register_type(crate::effects::damage::DamageType::new(id)),
        LootTable => {
            crate::loot::register_table(crate::loot::Table::new(id, crate::loot::TableKind::Generic))
        }
        Feature => crate::worldgen::register_feature(crate::worldgen::GenerationPiece::new(
            crate::worldgen::PieceKind::Feature,
            id,
        )),
        Structure => crate::structures::register(crate::structures::Structure::new(id, id)),
        WorldGenerator => crate::worldgen::register_chunk_generator(
            crate::worldgen::GenerationPiece::new(crate::worldgen::PieceKind::ChunkGenerator, id),
        ),
        Shader => crate::shaders::register(crate::shaders::Shader::new(
            id,
            crate::shaders::ShaderStage::Vertex,
        )),
        Model => crate::models::register(crate::models::Model::new(id)),
        Texture => crate::textures::register(crate::textures::Texture::new(
            id,
            1,
            1,
            crate::textures::TextureFormat::Rgba8,
        )),
        Animation => crate::models::register_animation(crate::models::Animation::new(id)),
        RenderLayer => crate::render::register_render_layer(id),
        EntityRenderer => crate::models::register_entity_renderer(id),
        BlockRenderer => crate::models::register_block_renderer(id),
        ItemRenderer => crate::models::register_item_renderer(id),
        CustomComponent => crate::components::register(crate::components::ComponentType::new(id)),
        CustomDataType => crate::data::register_data_type(crate::data::DataType::new(
            id,
            crate::data::ValueKind::String,
        )),
        CustomNetworkChannel => crate::channels::register(crate::channels::Channel::new(id)),
        AiGoal => crate::ai::register_goal(crate::ai::Goal::new(id, 0)),
        AiSensor => crate::ai::register_sensor(crate::ai::Sensor::new(id, 0.0)),
        Behavior => crate::ai::register_behavior(crate::ai::Behaviour::new(id, id)),
        Statistic => crate::statistics::register(crate::statistics::Statistic::new(id, "custom")),
        TradeOffer => crate::trading::register_offer(crate::trading::TradeOffer::new(id, "custom")),
        TradeProfession => crate::trading::register_profession(id),
        BrewingRecipe => crate::recipes::register_brewing_recipe(crate::recipes::Recipe::new(
            crate::recipes::RecipeType::Brewing,
            id,
            crate::items::ItemStack::new(id, 1),
        )),
        FuelType => crate::recipes::register_fuel(id, crate::recipes::COAL_FUEL),
        // menu, attribute, font, sound_event, particle_renderer, game_rule, advancement:
        // the client's own, with no registry in this crate to delegate to.
        Menu | Attribute | Font | SoundEvent | ParticleRenderer | GameRule | Advancement => true,
    }
}

fn forget(kind: ContentKind, id: &str) -> bool {
    use ContentKind::*;
    match kind {
        Biome => crate::worldgen::unregister_biome(id),
        Block => crate::blocks::unregister(id),
        Item => crate::items::unregister(id),
        Entity => crate::entities::unregister_type(id),
        Effect => crate::effects::unregister(id),
        Enchantment => crate::enchantments::unregister(id),
        Particle => crate::particles::unregister(id),
        Sound => crate::audio::unregister_sound(id),
        Screen => crate::ui::unregister_screen(id),
        Command => crate::commands::unregister(id),
        Keybind => crate::input::unregister_keybind(id),
        Recipe => crate::recipes::unregister(id),
        BlockEntity => crate::block_entities::unregister_type(id),
        Dimension => crate::worldgen::unregister_piece(crate::worldgen::PieceKind::Dimension, id),
        Fluid => crate::fluids::unregister(id),
        DamageType => crate::effects::damage::unregister_type(id),
        LootTable => crate::loot::unregister_table(id),
        Feature => crate::worldgen::unregister_feature(id),
        Structure => crate::structures::unregister(id),
        WorldGenerator => {
            crate::worldgen::unregister_piece(crate::worldgen::PieceKind::ChunkGenerator, id)
        }
        Shader => crate::shaders::unregister(id),
        Model => crate::models::unregister(id),
        Texture => crate::textures::unregister(id),
        Animation => crate::models::unregister_animation(id),
        RenderLayer => crate::render::unregister_render_layer(id),
        CustomComponent => crate::components::unregister(id),
        CustomNetworkChannel => crate::channels::unregister(id),
        Statistic => crate::statistics::unregister(id),
        BrewingRecipe => crate::recipes::unregister(id),
        // No unregister in the owning module, or no owner at all: the index entry is enough.
        Menu | Attribute | RecipeSerializer | Font | EntityRenderer | BlockRenderer
        | ItemRenderer | ParticleRenderer | SoundEvent | CustomDataType | AiGoal | AiSensor
        | Behavior | GameRule | Advancement | TradeOffer | TradeProfession | FuelType => true,
    }
}

macro_rules! register_fns {
    ($($fn_name:ident => $variant:ident),* $(,)?) => {
        $(
            /// Declares this kind, recording it in both `content` and the owning module.
            pub fn $fn_name(id: &str) -> bool {
                register(ContentKind::$variant, id)
            }
        )*
    };
}

register_fns! {
    register_biome => Biome,
    register_block => Block,
    register_item => Item,
    register_entity => Entity,
    register_effect => Effect,
    register_enchantment => Enchantment,
    register_particle => Particle,
    register_sound => Sound,
    register_screen => Screen,
    register_menu => Menu,
    register_command => Command,
    register_keybind => Keybind,
    register_recipe => Recipe,
    register_recipe_serializer => RecipeSerializer,
    register_block_entity => BlockEntity,
    register_dimension => Dimension,
    register_fluid => Fluid,
    register_attribute => Attribute,
    register_damage_type => DamageType,
    register_loot_table => LootTable,
    register_feature => Feature,
    register_structure => Structure,
    register_world_generator => WorldGenerator,
    register_shader => Shader,
    register_model => Model,
    register_texture => Texture,
    register_font => Font,
    register_animation => Animation,
    register_render_layer => RenderLayer,
    register_entity_renderer => EntityRenderer,
    register_block_renderer => BlockRenderer,
    register_item_renderer => ItemRenderer,
    register_particle_renderer => ParticleRenderer,
    register_sound_event => SoundEvent,
    register_custom_component => CustomComponent,
    register_custom_data_type => CustomDataType,
    register_custom_network_channel => CustomNetworkChannel,
    register_ai_goal => AiGoal,
    register_ai_sensor => AiSensor,
    register_behavior => Behavior,
    register_game_rule => GameRule,
    register_statistic => Statistic,
    register_advancement => Advancement,
    register_trade_offer => TradeOffer,
    register_trade_profession => TradeProfession,
    register_brewing_recipe => BrewingRecipe,
    register_fuel_type => FuelType,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|index| index.clear());
    }

    #[test]
    fn every_kind_has_a_name_and_a_sentence() {
        let _serial = serial();
        assert_eq!(ContentKind::all().len(), 47);
        for kind in ContentKind::all() {
            assert!(!kind.as_str().is_empty());
            assert_eq!(ContentKind::from_name(kind.as_str()), Some(kind));
            assert!(!kind.refusal().is_empty(), "{} has no reason", kind.as_str());
            assert_ne!(
                kind.refusal(),
                format!("{} is not something this host can see", kind.as_str())
            );
        }
        assert_eq!(ContentKind::from_name("nothing"), None);
        assert_eq!(refusals().len(), 47);
    }

    #[test]
    fn a_duplicate_is_refused_for_five_different_kinds() {
        let _serial = serial();
        fresh();
        assert!(register_block("hello:plugin_diamond"));
        assert!(!register_block("hello:plugin_diamond"));
        assert!(register_item("hello:plugin_gem"));
        assert!(!register_item("hello:plugin_gem"));
        assert!(register_entity("hello:plugin_mob"));
        assert!(!register_entity("hello:plugin_mob"));
        assert!(register_effect("hello:plugin_haste"));
        assert!(!register_effect("hello:plugin_haste"));
        assert!(register_statistic("hello:plugin_mined"));
        assert!(!register_statistic("hello:plugin_mined"));
    }

    #[test]
    fn an_entry_reads_back_with_kind_id_and_owner() {
        let _serial = serial();
        fresh();
        assert!(register_block("hello:ledger_block"));
        let entry = get(ContentKind::Block, "hello:ledger_block").expect("recorded");
        assert_eq!(entry.id, "hello:ledger_block");
        assert_eq!(entry.kind, ContentKind::Block);
        assert_eq!(entry.owner, crate::metadata::id());
        assert_eq!(entry.module(), "blocks");
        assert_eq!(count(ContentKind::Block), 1);
        assert!(all_of(ContentKind::Block).iter().any(|held| held.id == "hello:ledger_block"));
        assert!(kinds().contains(&ContentKind::Block));
        assert!(all().iter().any(|held| held.id == "hello:ledger_block"));
    }

    #[test]
    fn registering_delegates_to_the_owning_module() {
        let _serial = serial();
        fresh();
        assert!(register_block("hello:delegated_block"));
        assert!(register_item("hello:delegated_item"));
        assert!(register_entity("hello:delegated_entity"));
        assert!(register_effect("hello:delegated_effect"));
        assert!(register_enchantment("hello:delegated_enchant"));
        assert!(register_particle("hello:delegated_particle"));
        assert!(register_fluid("hello:delegated_fluid"));
        assert!(register_biome("hello:delegated_biome"));
        assert!(register_recipe("hello:delegated_recipe"));
        assert!(register_texture("hello:delegated_texture"));
        assert!(register_shader("hello:delegated_shader"));
        assert!(register_model("hello:delegated_model"));

        assert!(crate::blocks::get_type("hello:delegated_block").is_some());
        assert!(crate::items::get("hello:delegated_item").is_some());
        assert!(crate::entities::get_by_id("hello:delegated_entity").is_some());
        assert!(crate::effects::get_by_id("hello:delegated_effect").is_some());
        assert!(crate::enchantments::get("hello:delegated_enchant").is_some());
        assert!(crate::particles::get_type("hello:delegated_particle").is_some());
        assert!(crate::fluids::get_fluid("hello:delegated_fluid").is_some());
        assert!(crate::worldgen::get_biome_settings("hello:delegated_biome").is_some());
        assert!(crate::recipes::get_by_id("hello:delegated_recipe").is_some());
        assert!(crate::textures::get("hello:delegated_texture").is_some());
        assert!(crate::shaders::get("hello:delegated_shader").is_some());
        assert!(crate::models::get("hello:delegated_model").is_some());

        assert!(register_loot_table("hello:delegated_loot"));
        assert!(crate::loot::get_table("hello:delegated_loot").is_some());
        assert!(register_block_entity("hello:delegated_tile"));
        assert!(crate::block_entities::get_type("hello:delegated_tile").is_some());
        assert!(register_custom_network_channel("hello:delegated_channel"));
        assert!(crate::channels::get("hello:delegated_channel").is_some());
        assert!(register_statistic("hello:delegated_stat"));
        assert!(crate::statistics::get_statistics().iter().any(|stat| stat.name == "hello:delegated_stat"));
        assert!(register_fuel_type("hello:delegated_fuel"));
        assert_eq!(crate::recipes::get_fuel_time("hello:delegated_fuel"), Some(crate::recipes::COAL_FUEL));
        assert!(register_ai_goal("hello:delegated_goal"));
        assert!(crate::ai::get_registered_goal("hello:delegated_goal").is_some());
        assert!(register_trade_offer("hello:delegated_offer"));
        assert!(crate::trading::get_offer("hello:delegated_offer").is_some());
    }

    #[test]
    fn unregister_forgets_the_entry_and_the_owner() {
        let _serial = serial();
        fresh();
        assert!(register_block("hello:gone_block"));
        assert!(register_item("hello:gone_item"));
        assert!(unregister(ContentKind::Block, "hello:gone_block"));
        assert!(!unregister(ContentKind::Block, "hello:gone_block"));
        assert!(crate::blocks::get_type("hello:gone_block").is_none());
        assert_eq!(count(ContentKind::Block), 0);
        assert_eq!(count(ContentKind::Item), 1, "the other kind is untouched");
        assert!(unregister(ContentKind::Item, "hello:gone_item"));
        assert!(crate::items::get("hello:gone_item").is_none());
    }

    #[test]
    fn the_client_registries_are_refused_with_a_sentence_each() {
        let _serial = serial();
        assert!(!register_with_the_client(ContentKind::Block, "hello:client_block"));
        assert!(!register_with_the_client(ContentKind::Item, "hello:client_item"));
        assert_eq!(why_unknown("block"), ContentKind::Block.refusal());
        assert_eq!(
            why_unknown("nothing at all"),
            "nothing at all is not something this host can see"
        );
        assert_eq!(ContentKind::Block.client_registry(), "Blocks");
        assert!(crate::registry::CLIENT_REGISTRIES.contains(&ContentKind::Item.client_registry()));
    }
}
