//! The world, and dimensions: what a mod can declare here, and what only the client knows.
//!
//! This is the first section where the honest answer is mostly "not from here". The client's
//! world — its chunks, blocks, entities, biome, time, weather, border and save file — lives
//! inside the client, and section 10 lists the capabilities that would read or change it as
//! `Never` with the reason (`needs hooks into the client's world and entity state`). Guessing
//! at a world here would be worse than saying nothing, so every one of those names answers
//! `None` with a reason, and [`why_unknown`] is where the reasons live.
//!
//! What *is* real, and is what a mod actually needs before hooks exist, is the **dimension
//! registry**: a mod declares the dimensions it would add, with their type, bounds, gravity,
//! time, biome source, ambient properties and respawn rules, and every mod can read that
//! registry. [`dimensions::create`] and [`dimensions::teleport_to_dimension`] then answer
//! honestly that this host cannot make the client act on a declaration — that is
//! `CustomDimensions`, which section 10 reports as `never`.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! dimensions::register(DimensionDescription::new("skylands", "Skylands")
//!     .kind(DimensionKind::Custom)
//!     .bounds(-64, 320)
//!     .gravity(0.6)
//!     .biome_source("minecraft:sky"));
//! assert!(dimensions::get_by_name("Skylands").is_some());
//! assert!(!dimensions::create("skylands"), "the client is not told: no hook for it");
//! ```

/// Why a world question has no answer in this host.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "world" => {
            "the client's world lives in its own state; reading it needs the hooks section 10 \
             lists as never"
                .to_owned()
        }
        "chunks" | "chunk" => {
            "chunks belong to the client's world stream; a mod's copy would be a guess".to_owned()
        }
        "blocks" => "block state at a position is the client's; reading it needs those hooks".to_owned(),
        "entities" | "players" => {
            "entities and players are the client's objects, not reachable from this host".to_owned()
        }
        "biome" | "weather" | "time" => {
            "the world's weather and time are part of the client's world state".to_owned()
        }
        "seed" => "the seed is the client's, and this host cannot ask for it".to_owned(),
        "spawn" => "the world spawn is the client's state, and writing it needs those hooks".to_owned(),
        "border" => "the world border is the client's state".to_owned(),
        "save" => "this loader never touches the client's save files".to_owned(),
        "snapshot" => "a snapshot of the world would be a snapshot of the client's memory".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What the loader knows about the world it is in, which is nothing but its name from a
/// pack. Every field is what the client would have to answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorldInfo {
    pub id: String,
    pub name: String,
    pub dimensions: Vec<String>,
}

/// The world the client is in. `None`, with [`why_unknown`] saying why.
#[must_use]
pub fn get() -> Option<WorldInfo> {
    None
}

/// The same, by id.
#[must_use]
pub fn get_by_id(_id: &str) -> Option<WorldInfo> {
    None
}

/// The same, by name.
#[must_use]
pub fn get_by_name(_name: &str) -> Option<WorldInfo> {
    None
}

/// Every world. There is none: this host sees the client's, and only the client knows it.
#[must_use]
pub fn get_all() -> Vec<WorldInfo> {
    Vec::new()
}

/// Whether a world is loaded. `None`: the client's world is not something this host can ask
/// about.
#[must_use]
pub fn is_loaded() -> Option<bool> {
    None
}

/// Asks the client to load a world, which this host cannot do. Recorded, and said out loud.
pub fn load(_id: &str) -> bool {
    no_hook("load a world")
}

/// Asks the client to unload its world. Same answer.
pub fn unload() -> bool {
    no_hook("unload a world")
}

pub(crate) fn no_hook(what: &str) -> bool {
    crate::warn!(
        "a mod asked this host to {what}: the client's world is out of reach, so nothing was \
         done; the request is in the log"
    );
    false
}

/// A request this host cannot carry out, for a caller that wants to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub what: String,
    pub why: String,
}

fn refusal(what: &str) -> Refusal {
    Refusal {
        what: what.to_owned(),
        why: why_unknown(what),
    }
}

/// What the loader will not pretend to do about the world, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<Refusal> {
    [
        "world",
        "chunks",
        "blocks",
        "entities",
        "players",
        "biome",
        "weather",
        "time",
        "seed",
        "spawn",
        "border",
        "save",
        "snapshot",
    ]
    .iter()
    .map(|what| refusal(what))
    .collect()
}

/// Every `world::` question the client would have to answer, with the same honest `None` and
/// the reason in [`why_unknown`]. They are separate functions rather than one because the
/// checklist is a list of names, and a name that is missing cannot be answered for.
pub mod known_only_to_the_client {
    /// The id of the world the client is in.
    #[must_use]
    pub fn get_id() -> Option<String> {
        None
    }

    /// Its name.
    #[must_use]
    pub fn get_name() -> Option<String> {
        None
    }

    /// Its seed.
    #[must_use]
    pub fn get_seed() -> Option<i64> {
        None
    }

    /// The world's clock, in ticks.
    #[must_use]
    pub fn get_time() -> Option<u64> {
        None
    }

    /// The time of day, in ticks.
    #[must_use]
    pub fn get_day_time() -> Option<u64> {
        None
    }

    /// How long the world has existed, in ticks.
    #[must_use]
    pub fn get_game_time() -> Option<u64> {
        None
    }

    /// The difficulty.
    #[must_use]
    pub fn get_difficulty() -> Option<String> {
        None
    }

    /// The world's game rules.
    #[must_use]
    pub fn get_game_rules() -> Vec<(String, String)> {
        Vec::new()
    }

    /// Where players respawn.
    #[must_use]
    pub fn get_spawn_position() -> Option<(f64, f64, f64)> {
        None
    }

    /// Asks the client to move the world spawn. This host cannot.
    pub fn set_spawn_position(_x: f64, _y: f64, _z: f64) -> bool {
        super::no_hook("move the world spawn")
    }

    /// The world border.
    #[must_use]
    pub fn get_border() -> Option<(f64, f64)> {
        None
    }

    /// How tall the world is.
    #[must_use]
    pub fn get_height() -> Option<i32> {
        None
    }

    /// The lowest block.
    #[must_use]
    pub fn get_min_height() -> Option<i32> {
        None
    }

    /// The highest block.
    #[must_use]
    pub fn get_max_height() -> Option<i32> {
        None
    }

    /// The chunks the client has loaded.
    #[must_use]
    pub fn get_chunks() -> Vec<(i32, i32)> {
        Vec::new()
    }

    /// One chunk's position, when the client has it.
    #[must_use]
    pub fn get_chunk(_x: i32, _z: i32) -> Option<(i32, i32)> {
        None
    }

    /// Asks for a chunk to be loaded.
    pub fn load_chunk(_x: i32, _z: i32) -> bool {
        super::no_hook("load a chunk")
    }

    /// Asks for a chunk to be unloaded.
    pub fn unload_chunk(_x: i32, _z: i32) -> bool {
        super::no_hook("unload a chunk")
    }

    /// Whether the client has a chunk loaded.
    #[must_use]
    pub fn is_chunk_loaded(_x: i32, _z: i32) -> Option<bool> {
        None
    }

    /// Asks the client to send a chunk request of its own. This host has no channel to it.
    pub fn request_chunk(_x: i32, _z: i32) -> bool {
        super::no_hook("request a chunk from the server")
    }

    /// The players in the world.
    #[must_use]
    pub fn get_players() -> Vec<String> {
        Vec::new()
    }

    /// The entities in the world.
    #[must_use]
    pub fn get_entities() -> Vec<String> {
        Vec::new()
    }

    /// The players within a distance of a point.
    #[must_use]
    pub fn get_players_in_range(_x: f64, _y: f64, _z: f64, _range: f64) -> Vec<String> {
        Vec::new()
    }

    /// The entities within a distance of a point.
    #[must_use]
    pub fn get_entities_in_range(_x: f64, _y: f64, _z: f64, _range: f64) -> Vec<String> {
        Vec::new()
    }

    /// The entities inside a box.
    #[must_use]
    pub fn get_entities_in_box(
        _from: (f64, f64, f64),
        _to: (f64, f64, f64),
    ) -> Vec<String> {
        Vec::new()
    }

    /// The blocks inside a box.
    #[must_use]
    pub fn get_blocks_in_box(
        _from: (i32, i32, i32),
        _to: (i32, i32, i32),
    ) -> Vec<(i32, i32, i32, String)> {
        Vec::new()
    }

    /// The weather.
    #[must_use]
    pub fn get_weather() -> Option<String> {
        None
    }

    /// Asks the client to change the weather. This host cannot.
    pub fn set_weather(_weather: &str) -> bool {
        super::no_hook("change the weather")
    }

    /// The biome at a position.
    #[must_use]
    pub fn get_biome(_x: i32, _y: i32, _z: i32) -> Option<String> {
        None
    }

    /// The dimension the client is in.
    #[must_use]
    pub fn get_dimension() -> Option<String> {
        None
    }

    /// Where the client's world is saved, which this loader never reads.
    #[must_use]
    pub fn get_save_path() -> Option<String> {
        None
    }

    /// What the client knows about its world, beyond the name.
    #[must_use]
    pub fn get_metadata() -> Vec<(String, String)> {
        Vec::new()
    }

    /// Asks the client to save its world.
    pub fn save() -> bool {
        super::no_hook("save the world")
    }

    /// Notes that a mod changed something in the world it does not own.
    pub fn mark_dirty() -> bool {
        super::no_hook("mark the world dirty")
    }

    /// Takes a snapshot of the world. `None`: a snapshot would be of the client's memory.
    #[must_use]
    pub fn create_snapshot() -> Option<u64> {
        None
    }

    /// Puts a snapshot back.
    pub fn restore_snapshot(_snapshot: u64) -> bool {
        super::no_hook("restore a world snapshot")
    }
}

/// The dimension registry: what mods declare here, and what this host cannot do with it.
///
/// A declaration is real data every mod can read; telling the *client* to add the dimension
/// is not something this host can do, which is `CustomDimensions`, reported `never` in
/// section 10.
///
/// ```
/// use cinnaloader::prelude::*;
///
/// dimensions::register(DimensionDescription::new("skylands", "Skylands").gravity(0.6));
/// assert_eq!(dimensions::get_gravity("skylands"), Some(0.6));
/// # dimensions::unregister("skylands");
/// ```
pub mod dimensions {
    use super::why_unknown;
    use std::collections::BTreeMap;
    use std::fmt;
    use std::sync::{Mutex, OnceLock};

    /// What kind of dimension a mod is describing.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub enum DimensionKind {
        /// The overworld.
        Overworld,
        /// The nether.
        Nether,
        /// The end.
        End,
        /// A dimension of the mod's own.
        #[default]
        Custom,
    }

    impl DimensionKind {
        #[must_use]
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Overworld => "overworld",
                Self::Nether => "nether",
                Self::End => "end",
                Self::Custom => "custom",
            }
        }
    }

    impl fmt::Display for DimensionKind {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.as_str())
        }
    }

    /// Where a player goes when they die there.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct RespawnRules {
        /// The dimension they respawn in, by id; empty means the one they died in.
        pub dimension: String,
        /// Whether the respawn point is the world spawn rather than the bed.
        pub world_spawn: bool,
        pub respawn_anchor_works: bool,
    }

    /// What a dimension looks like, for a caller that has to render or reason about it.
    #[derive(Debug, Clone, PartialEq)]
    pub struct AmbientProperties {
        /// How bright it is, 0.0 to 1.0.
        pub light: f32,
        /// The fog colour, `0xRRGGBB`.
        pub fog_color: u32,
        pub sky_color: u32,
        /// Whether it has a sky at all.
        pub has_sky: bool,
        pub has_weather: bool,
    }

    impl Default for AmbientProperties {
        fn default() -> Self {
            Self {
                light: 1.0,
                fog_color: 0x00c0_ffff,
                sky_color: 0x0078_a0ff,
                has_sky: true,
                has_weather: true,
            }
        }
    }

    /// A dimension a mod declares: what it is called, how big it is, and how it behaves.
    #[derive(Debug, Clone, PartialEq)]
    pub struct DimensionDescription {
        pub id: String,
        pub name: String,
        pub kind: DimensionKind,
        /// The mod that declared it.
        pub owner: String,
        /// Lowest and highest block the dimension has.
        pub bounds: (i32, i32),
        /// How strong gravity is there.
        pub gravity: f32,
        /// The dimension's own clock, in ticks.
        pub time: u64,
        pub biome_source: String,
        pub ambient: AmbientProperties,
        pub respawn: RespawnRules,
        /// Whether the client has been told about it, which this host cannot do.
        pub created: bool,
    }

    impl DimensionDescription {
        /// A dimension with the defaults a caller would expect: overworld bounds, normal
        /// gravity, a sky.
        #[must_use]
        pub fn new(id: &str, name: &str) -> Self {
            Self {
                id: id.to_owned(),
                name: name.to_owned(),
                kind: DimensionKind::Custom,
                owner: crate::metadata::id(),
                bounds: (-64, 320),
                gravity: 1.0,
                time: 0,
                biome_source: String::new(),
                ambient: AmbientProperties::default(),
                respawn: RespawnRules::default(),
                created: false,
            }
        }

        #[must_use]
        pub fn kind(mut self, kind: DimensionKind) -> Self {
            self.kind = kind;
            self
        }

        #[must_use]
        pub fn bounds(mut self, minimum: i32, maximum: i32) -> Self {
            self.bounds = (minimum.min(maximum), minimum.max(maximum));
            self
        }

        #[must_use]
        pub fn gravity(mut self, gravity: f32) -> Self {
            self.gravity = gravity;
            self
        }

        #[must_use]
        pub fn time(mut self, ticks: u64) -> Self {
            self.time = ticks;
            self
        }

        #[must_use]
        pub fn biome_source(mut self, source: &str) -> Self {
            self.biome_source = source.to_owned();
            self
        }

        #[must_use]
        pub fn ambient(mut self, ambient: AmbientProperties) -> Self {
            self.ambient = ambient;
            self
        }

        #[must_use]
        pub fn respawn(mut self, respawn: RespawnRules) -> Self {
            self.respawn = respawn;
            self
        }

        /// How tall it is, in blocks.
        #[must_use]
        pub fn height(&self) -> i32 {
            self.bounds.1 - self.bounds.0
        }
    }

    impl fmt::Display for DimensionDescription {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                formatter,
                "{} ({}, {} to {}, gravity {})",
                self.name, self.id, self.bounds.0, self.bounds.1, self.gravity
            )
        }
    }

    fn registry() -> &'static Mutex<BTreeMap<String, DimensionDescription>> {
        static REGISTRY: OnceLock<Mutex<BTreeMap<String, DimensionDescription>>> = OnceLock::new();
        REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    pub(crate) fn with<T>(body: impl FnOnce(&mut BTreeMap<String, DimensionDescription>) -> T) -> T {
        let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
        body(&mut guard)
    }

    /// Declares a dimension, or replaces the one with that id. Answers whether it is new.
    pub fn register(description: DimensionDescription) -> bool {
        with(|registry| registry.insert(description.id.clone(), description).is_none())
    }

    /// Forgets a dimension a mod declared.
    pub fn unregister(id: &str) -> bool {
        with(|registry| registry.remove(id).is_some())
    }

    /// A dimension a mod declared, by id.
    #[must_use]
    pub fn get(id: &str) -> Option<DimensionDescription> {
        with(|registry| registry.get(id).cloned())
    }

    /// A dimension a mod declared, by name.
    #[must_use]
    pub fn get_by_name(name: &str) -> Option<DimensionDescription> {
        with(|registry| {
            registry
                .values()
                .find(|description| description.name == name)
                .cloned()
        })
    }

    /// Every dimension a mod declared, in id order.
    #[must_use]
    pub fn get_all() -> Vec<DimensionDescription> {
        with(|registry| registry.values().cloned().collect())
    }

    /// Every dimension a mod declared, by id.
    #[must_use]
    pub fn get_by_id(id: &str) -> Option<DimensionDescription> {
        get(id)
    }

    /// Asks the client to create a dimension from a declaration. This host cannot: that is
    /// `CustomDimensions`, which section 10 reports as `never`, and the client's world
    /// generation is its own.
    pub fn create(id: &str) -> bool {
        let known = get(id).is_some();
        crate::warn!(
            "a mod asked this host to create the dimension {id}: the client cannot be told to add \
             one from here ({}), so the declaration stays a declaration",
            why_unknown("world")
        );
        let _ = known;
        false
    }

    /// The kind of a declared dimension.
    #[must_use]
    pub fn get_type(id: &str) -> Option<DimensionKind> {
        get(id).map(|description| description.kind)
    }

    /// A declared dimension's lowest and highest block.
    #[must_use]
    pub fn get_bounds(id: &str) -> Option<(i32, i32)> {
        get(id).map(|description| description.bounds)
    }

    /// A declared dimension's gravity.
    #[must_use]
    pub fn get_gravity(id: &str) -> Option<f32> {
        get(id).map(|description| description.gravity)
    }

    /// A declared dimension's own clock, in ticks.
    #[must_use]
    pub fn get_time(id: &str) -> Option<u64> {
        get(id).map(|description| description.time)
    }

    /// A declared dimension's biome source.
    #[must_use]
    pub fn get_biome_source(id: &str) -> Option<String> {
        get(id).map(|description| description.biome_source)
    }

    /// A declared dimension's ambient properties.
    #[must_use]
    pub fn get_ambient_properties(id: &str) -> Option<AmbientProperties> {
        get(id).map(|description| description.ambient)
    }

    /// A declared dimension's respawn rules.
    #[must_use]
    pub fn get_respawn_rules(id: &str) -> Option<RespawnRules> {
        get(id).map(|description| description.respawn)
    }

    /// Asks the client to move the player to a dimension. This host cannot move the player, and
    /// says so rather than pretending.
    pub fn teleport_to_dimension(id: &str) -> bool {
        crate::warn!(
            "a mod asked this host to teleport to {id}: the player is the client's, and moving them \
             needs hooks this host does not have"
        );
        false
    }

    /// Whether the client has been told about a declared dimension, which it never has.
    #[must_use]
    pub fn is_created(id: &str) -> bool {
        get(id).is_some_and(|description| description.created)
    }
}

pub use dimensions::{AmbientProperties, DimensionDescription, DimensionKind, RespawnRules};

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        dimensions::with(|registry| registry.clear());
    }

    #[test]
    fn the_clients_world_is_refused_with_a_reason_every_time() {
        let _serial = serial();
        fresh();
        assert!(get().is_none());
        assert!(dimensions::get_by_id("overworld").is_none());
        assert!(dimensions::get_by_name("World").is_none());
        assert!(dimensions::get_all().is_empty());
        assert_eq!(is_loaded(), None);
        assert!(!load("anything"));
        assert!(!unload());
        assert_eq!(refusals().len(), 13);
        for refusal in refusals() {
            assert!(!refusal.why.is_empty(), "{} has no reason", refusal.what);
            assert_ne!(
                refusal.why,
                format!("{} is not something this host can see", refusal.what)
            );
        }
    }

    #[test]
    fn a_declared_dimension_can_be_read_back() {
        let _serial = serial();
        fresh();
        assert!(dimensions::register(
            DimensionDescription::new("skylands", "Skylands")
                .kind(DimensionKind::Custom)
                .bounds(320, -64)
                .gravity(0.6)
                .time(12_000)
                .biome_source("minecraft:sky")
                .respawn(RespawnRules {
                    dimension: "skylands".to_owned(),
                    world_spawn: true,
                    respawn_anchor_works: false,
                })
        ));
        assert!(
            !dimensions::register(DimensionDescription::new("skylands", "Skylands again")),
            "registering the same id replaces it"
        );
        let found = dimensions::get_by_name("Skylands again").expect("the second registration replaced it");
        assert_eq!(found.bounds, (-64, 320), "bounds are ordered for the caller");
        assert_eq!(found.height(), 384);
        assert_eq!(dimensions::get_type("skylands"), Some(DimensionKind::Custom));
        assert_eq!(dimensions::get_by_id("skylands").map(|d| d.gravity), Some(1.0));
        assert_eq!(dimensions::get_time("skylands"), Some(0), "the replacement reset it");
        assert!(dimensions::get_ambient_properties("skylands").is_some());
        assert_eq!(
            dimensions::get_respawn_rules("skylands").map(|rules| rules.world_spawn),
            Some(false)
        );
        assert_eq!(dimensions::get_all().len(), 1);
        assert!(!dimensions::is_created("skylands"), "the client was never told");
        assert!(dimensions::unregister("skylands"));
        assert!(!dimensions::unregister("skylands"));
    }

    #[test]
    fn creating_a_dimension_is_refused_rather_than_faked() {
        let _serial = serial();
        fresh();
        fresh();
        dimensions::register(DimensionDescription::new("skylands", "Skylands"));
        assert!(!dimensions::create("skylands"));
        assert!(!dimensions::create("never-declared"));
        assert!(!dimensions::teleport_to_dimension("skylands"));
        assert!(!dimensions::is_created("skylands"));
    }

    #[test]
    fn a_dimension_description_renders_for_a_report() {
        let _serial = serial();
        fresh();
        let description = DimensionDescription::new("skylands", "Skylands").bounds(-64, 320);
        let text = description.to_string();
        assert!(text.contains("Skylands"), "{text}");
        assert!(text.contains("skylands"), "{text}");
        assert!(text.contains("-64 to 320"), "{text}");
        assert_eq!(DimensionKind::Nether.to_string(), "nether");
    }
}
