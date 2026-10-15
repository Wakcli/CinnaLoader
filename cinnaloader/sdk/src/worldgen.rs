//! World generation: the pieces a mod ships, and the noise it can run for itself.
//!
//! Everything a data pack would describe, a mod registers here: biomes, features, structures,
//! ores, carvers, surface rules, placement rules, noise, density functions, dimensions,
//! dimension types, biome sources and chunk generators. That registry is real data every mod
//! can read back, and it is what a mod needs before the day the client can be told about any
//! of it.
//!
//! What is also real, and more than the section's name suggests: a seeded [`Random`] a mod
//! owns, [`Noise`] (a Perlin-shaped function over a seed, said to be *this loader's noise and
//! not bit-identical to the client's*), a [`GenerationContext`] a mod builds, and a [`Chunk`]
//! a mod's own generator fills — 16×16 columns of block names, with `get_height_at` and
//! `get_biome_at` over it. A mod can generate a world in this host; it just cannot put it into
//! the client's.
//!
//! Refused: the client's world seed, its generator and its random, placing into the live
//! world, and reordering the client's generation — all of which live inside the client's
//! world generation, which does not call out to a mod here.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! worldgen::register_biome(Biome::new("hello:sky", "Sky").temperature(0.4));
//! assert_eq!(worldgen::get_biome_settings("hello:sky").map(|biome| biome.name), Some("Sky".into()));
//!
//! let noise = Noise::new(42);
//! assert!((-1.0..=1.0).contains(&noise.at(0.5, 0.5, 0.5)));
//! let chunk = worldgen::generate_chunk(&GenerationContext::new(42, (0, 0)), &|x, z, height| {
//!     if height < 64 { "minecraft:stone" } else { "minecraft:air" }.to_owned()
//! });
//! assert_eq!(worldgen::get_height_at(&chunk, 0, 0).is_some(), true);
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// How wide a chunk is.
pub const CHUNK_WIDTH: usize = 16;
/// How tall the world is, in blocks.
pub const CHUNK_HEIGHT: usize = 384;
/// The lowest block in the world.
pub const WORLD_BOTTOM: i32 = -64;

/// A seeded random a mod owns. Bedrock's own is the client's; this one is deterministic and
/// reproducible, which is what a generator needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Random {
    state: u64,
}

impl Random {
    /// A random from a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// The same from a world seed and a position, which is how the game derives one per chunk.
    #[must_use]
    pub fn for_position(seed: u64, x: i32, z: i32) -> Self {
        let mixed = seed
            ^ (i64::from(x).wrapping_mul(0x9E37_79B9) as u64)
            ^ (i64::from(z).wrapping_mul(0x85EB_CA6B) as u64);
        Self::new(mixed)
    }

    fn next(&mut self) -> u64 {
        // xorshift64*, which is small, fast, and good enough for terrain.
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// The next 32 bits.
    pub fn next_u32(&mut self) -> u32 {
        (self.next() >> 32) as u32
    }

    /// The next number between zero and one.
    pub fn next_f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// The next number in a range.
    pub fn next_range(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        low + (self.next_f64() * (high - low + 1) as f64) as i64
    }

    /// Whether the next draw is under a chance, for ore and feature placement.
    pub fn chance(&mut self, probability: f64) -> bool {
        self.next_f64() < probability
    }

    /// Picks one of a list, or `None` when there is nothing to pick.
    pub fn pick<'a, T>(&mut self, from: &'a [T]) -> Option<&'a T> {
        if from.is_empty() {
            return None;
        }
        from.get(self.next_range(0, from.len() as i64 - 1) as usize)
    }
}

/// A Perlin-shaped noise function over a seed.
///
/// This is *this loader's* noise: same shape as the game's terrain noise, not bit-identical to
/// it. A mod using it gets reproducible, coherent terrain of its own.
#[derive(Debug, Clone)]
pub struct Noise {
    seed: u64,
    permutation: [u8; 512],
}

impl Noise {
    /// Noise from a seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut random = Random::new(seed);
        let mut table: [u8; 256] = [0; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            *slot = index as u8;
        }
        for index in (1..256).rev() {
            let swap = random.next_range(0, index as i64) as usize;
            table.swap(index, swap);
        }
        let mut permutation = [0u8; 512];
        for index in 0..512 {
            permutation[index] = table[index & 255];
        }
        Self { seed, permutation }
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    fn gradient(&self, hash: u8, x: f64, y: f64, z: f64) -> f64 {
        let h = hash & 15;
        let u = if h < 8 { x } else { y };
        let v = if h < 4 {
            y
        } else if h == 12 || h == 14 {
            x
        } else {
            z
        };
        let sign = |value: f64, high: bool| if high { value } else { -value };
        sign(u, h & 1 == 0) + sign(v, h & 2 == 0)
    }

    fn fade(t: f64) -> f64 {
        t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
    }

    fn lerp(t: f64, a: f64, b: f64) -> f64 {
        a + t * (b - a)
    }

    /// The noise at a point, between -1 and 1.
    #[must_use]
    pub fn at(&self, x: f64, y: f64, z: f64) -> f64 {
        let xi = x.floor() as i32 & 255;
        let yi = y.floor() as i32 & 255;
        let zi = z.floor() as i32 & 255;
        let xf = x - x.floor();
        let yf = y - y.floor();
        let zf = z - z.floor();
        let u = Self::fade(xf);
        let v = Self::fade(yf);
        let w = Self::fade(zf);
        let at = |dx: usize, dy: usize, dz: usize| -> u8 {
            let x = (xi as usize + dx) & 255;
            let y = (yi as usize + dy) & 255;
            let z = (zi as usize + dz) & 255;
            self.permutation
                [(self.permutation[(self.permutation[x] as usize + y) & 255] as usize + z) & 255]
        };
        let g = |dx: usize, dy: usize, dz: usize, ox: f64, oy: f64, oz: f64| {
            self.gradient(at(dx, dy, dz), xf - ox, yf - oy, zf - oz)
        };
        let x1 = Self::lerp(u, g(0, 0, 0, 0.0, 0.0, 0.0), g(1, 0, 0, 1.0, 0.0, 0.0));
        let x2 = Self::lerp(u, g(0, 1, 0, 0.0, 1.0, 0.0), g(1, 1, 0, 1.0, 1.0, 0.0));
        let y1 = Self::lerp(v, x1, x2);
        let x3 = Self::lerp(u, g(0, 0, 1, 0.0, 0.0, 1.0), g(1, 0, 1, 1.0, 0.0, 1.0));
        let x4 = Self::lerp(u, g(0, 1, 1, 0.0, 1.0, 1.0), g(1, 1, 1, 1.0, 1.0, 1.0));
        let y2 = Self::lerp(v, x3, x4);
        Self::lerp(w, y1, y2)
    }

    /// The same, at two dimensions, on the plane `z = 0`.
    #[must_use]
    pub fn at_2d(&self, x: f64, z: f64) -> f64 {
        self.at(x, 0.0, z)
    }

    /// Fractal noise: several octaves added together, which is what terrain looks like.
    #[must_use]
    pub fn fractal(&self, x: f64, z: f64, octaves: u32, persistence: f64) -> f64 {
        let mut total = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        let mut normal = 0.0;
        for _ in 0..octaves {
            total += self.at_2d(x * frequency, z * frequency) * amplitude;
            normal += amplitude;
            amplitude *= persistence;
            frequency *= 2.0;
        }
        if normal == 0.0 {
            0.0
        } else {
            total / normal
        }
    }
}

impl fmt::Display for Noise {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "noise(seed {})", self.seed)
    }
}

/// A biome a mod declares.
#[derive(Debug, Clone, PartialEq)]
pub struct Biome {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub temperature: f32,
    pub downfall: f32,
    /// The block on the surface, and the one under it.
    pub surface_block: String,
    pub subsurface_block: String,
    pub features: Vec<String>,
    pub structures: Vec<String>,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered: bool,
}

impl Biome {
    /// A plains-like biome with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            temperature: 0.5,
            downfall: 0.5,
            surface_block: "minecraft:grass_block".to_owned(),
            subsurface_block: "minecraft:dirt".to_owned(),
            features: Vec::new(),
            structures: Vec::new(),
            registered: false,
        }
    }

    #[must_use]
    pub fn temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }

    #[must_use]
    pub fn downfall(mut self, downfall: f32) -> Self {
        self.downfall = downfall;
        self
    }

    #[must_use]
    pub fn surface(mut self, surface: &str, subsurface: &str) -> Self {
        self.surface_block = surface.to_owned();
        self.subsurface_block = subsurface.to_owned();
        self
    }

    #[must_use]
    pub fn feature(mut self, feature: &str) -> Self {
        self.features.push(feature.to_owned());
        self
    }

    #[must_use]
    pub fn structure(mut self, structure: &str) -> Self {
        self.structures.push(structure.to_owned());
        self
    }
}

impl fmt::Display for Biome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {:.1} temperature, surface {})",
            self.name, self.id, self.temperature, self.surface_block
        )
    }
}

/// A named generation piece: features, structures, ores, carvers, rules, noise and density
/// functions all register as one of these, because from the loader's side they are the same
/// thing — a name, a kind and the settings a mod gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationPiece {
    pub id: String,
    pub name: String,
    pub kind: PieceKind,
    pub owner: String,
    /// Settings a mod recorded, as `key = value` in registration order.
    pub settings: Vec<(String, String)>,
}

/// What kind of generation piece it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PieceKind {
    Feature,
    Structure,
    Ore,
    Carver,
    SurfaceRule,
    PlacementRule,
    Noise,
    DensityFunction,
    Dimension,
    DimensionType,
    BiomeSource,
    ChunkGenerator,
}

impl PieceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::Structure => "structure",
            Self::Ore => "ore",
            Self::Carver => "carver",
            Self::SurfaceRule => "surface_rule",
            Self::PlacementRule => "placement_rule",
            Self::Noise => "noise",
            Self::DensityFunction => "density_function",
            Self::Dimension => "dimension",
            Self::DimensionType => "dimension_type",
            Self::BiomeSource => "biome_source",
            Self::ChunkGenerator => "chunk_generator",
        }
    }
}

impl GenerationPiece {
    /// A piece of a kind, with no settings yet.
    #[must_use]
    pub fn new(kind: PieceKind, id: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: id.to_owned(),
            kind,
            owner: crate::metadata::id(),
            settings: Vec::new(),
        }
    }

    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    #[must_use]
    pub fn setting(mut self, key: &str, value: &str) -> Self {
        self.settings.retain(|(held, _)| held != key);
        self.settings.push((key.to_owned(), value.to_owned()));
        self
    }

    #[must_use]
    pub fn setting_of(&self, key: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value.as_str())
    }
}

impl fmt::Display for GenerationPiece {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}{})",
            self.name,
            self.id,
            self.kind.as_str(),
            match self.settings.len() {
                0 => String::new(),
                count => format!(", {count} setting(s)"),
            }
        )
    }
}

/// Where a chunk is, and what it is generated from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationContext {
    pub seed: u64,
    pub chunk: (i32, i32),
    pub dimension: String,
    pub biome_source: String,
}

impl GenerationContext {
    /// A context for a chunk of a seed.
    #[must_use]
    pub fn new(seed: u64, chunk: (i32, i32)) -> Self {
        Self {
            seed,
            chunk,
            dimension: "minecraft:overworld".to_owned(),
            biome_source: String::new(),
        }
    }

    #[must_use]
    pub fn dimension(mut self, dimension: &str) -> Self {
        self.dimension = dimension.to_owned();
        self
    }

    #[must_use]
    pub fn biome_source(mut self, biome_source: &str) -> Self {
        self.biome_source = biome_source.to_owned();
        self
    }

    /// Where the chunk starts, in the world.
    #[must_use]
    pub fn origin(&self) -> (i32, i32) {
        (self.chunk.0 * CHUNK_WIDTH as i32, self.chunk.1 * CHUNK_WIDTH as i32)
    }

    /// A random seeded for this chunk, which is what a generator uses so the same seed makes
    /// the same world.
    #[must_use]
    pub fn random(&self) -> Random {
        Random::for_position(self.seed, self.chunk.0, self.chunk.1)
    }
}

/// A chunk a mod's own generator filled: 16×16 columns of block names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub position: (i32, i32),
    /// `blocks[x][z][y]`, all `CHUNK_HEIGHT` of it.
    pub blocks: Vec<Vec<Vec<String>>>,
    /// The biome id each column is in.
    pub biomes: Vec<Vec<String>>,
    /// Flattened heights, filled in as blocks are placed.
    pub heights: Vec<Vec<i32>>,
}

impl Chunk {
    /// An empty chunk: air everywhere, nothing generated yet.
    #[must_use]
    pub fn empty(position: (i32, i32)) -> Self {
        Self {
            position,
            blocks: vec![vec![vec![String::new(); CHUNK_HEIGHT]; CHUNK_WIDTH]; CHUNK_WIDTH],
            biomes: vec![vec![String::new(); CHUNK_WIDTH]; CHUNK_WIDTH],
            heights: vec![vec![WORLD_BOTTOM; CHUNK_WIDTH]; CHUNK_WIDTH],
        }
    }

    /// Sets one block, refusing a position outside the chunk.
    pub fn set_block(&mut self, x: usize, y: i32, z: usize, block: &str) -> bool {
        if x >= CHUNK_WIDTH || z >= CHUNK_WIDTH {
            return false;
        }
        let Some(level) = level_of(y) else {
            return false;
        };
        self.blocks[x][z][level] = block.to_owned();
        if !block.is_empty() && block != "minecraft:air" && y > self.heights[x][z] {
            self.heights[x][z] = y;
        }
        true
    }

    /// One block, or `None` when the position is outside the chunk.
    #[must_use]
    pub fn block(&self, x: usize, y: i32, z: usize) -> Option<&str> {
        if x >= CHUNK_WIDTH || z >= CHUNK_WIDTH {
            return None;
        }
        let level = level_of(y)?;
        Some(self.blocks[x][z][level].as_str())
    }

    /// The highest solid block in a column.
    #[must_use]
    pub fn height_at(&self, x: usize, z: usize) -> Option<i32> {
        if x >= CHUNK_WIDTH || z >= CHUNK_WIDTH {
            return None;
        }
        Some(self.heights[x][z])
    }

    /// The biome of a column.
    #[must_use]
    pub fn biome_at(&self, x: usize, z: usize) -> Option<&str> {
        if x >= CHUNK_WIDTH || z >= CHUNK_WIDTH {
            return None;
        }
        Some(self.biomes[x][z].as_str())
    }

    pub fn set_biome(&mut self, x: usize, z: usize, biome: &str) -> bool {
        if x >= CHUNK_WIDTH || z >= CHUNK_WIDTH {
            return false;
        }
        self.biomes[x][z] = biome.to_owned();
        true
    }

    /// How many blocks in the chunk are not air.
    #[must_use]
    pub fn solid_count(&self) -> usize {
        self.blocks
            .iter()
            .flat_map(|column| column.iter())
            .flat_map(|column| column.iter())
            .filter(|block| !block.is_empty() && block.as_str() != "minecraft:air")
            .count()
    }
}

impl fmt::Display for Chunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "chunk ({}, {}) with {} solid blocks",
            self.position.0,
            self.position.1,
            self.solid_count()
        )
    }
}

/// Which level of a chunk a world `y` is, or `None` outside the world.
#[must_use]
pub fn level_of(y: i32) -> Option<usize> {
    let level = y - WORLD_BOTTOM;
    (0..CHUNK_HEIGHT as i32).contains(&level).then_some(level as usize)
}

fn biomes() -> &'static Mutex<BTreeMap<String, Biome>> {
    static BIOMES: OnceLock<Mutex<BTreeMap<String, Biome>>> = OnceLock::new();
    BIOMES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_biomes<T>(body: impl FnOnce(&mut BTreeMap<String, Biome>) -> T) -> T {
    let mut guard = biomes().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn pieces() -> &'static Mutex<BTreeMap<(PieceKind, String), GenerationPiece>> {
    static PIECES: OnceLock<Mutex<BTreeMap<(PieceKind, String), GenerationPiece>>> = OnceLock::new();
    PIECES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_pieces<T>(body: impl FnOnce(&mut BTreeMap<(PieceKind, String), GenerationPiece>) -> T) -> T {
    let mut guard = pieces().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a biome.
pub fn register_biome(biome: Biome) -> bool {
    with_biomes(|biomes| biomes.insert(biome.id.clone(), biome).is_none())
}

/// Forgets a biome.
pub fn unregister_biome(id: &str) -> bool {
    with_biomes(|biomes| biomes.remove(id).is_some())
}

/// Everything a mod declared about a biome, under the name the wishlist uses for it.
#[must_use]
pub fn get_biome_settings(id: &str) -> Option<Biome> {
    with_biomes(|biomes| biomes.get(id).cloned())
}

/// A biome by name.
#[must_use]
pub fn get_biome_by_name(name: &str) -> Option<Biome> {
    with_biomes(|biomes| biomes.values().find(|biome| biome.name == name).cloned())
}

/// Every biome a mod declared.
#[must_use]
pub fn get_biomes() -> Vec<Biome> {
    with_biomes(|biomes| biomes.values().cloned().collect())
}

/// Registers a generation piece.
pub fn register_piece(piece: GenerationPiece) -> bool {
    with_pieces(|pieces| pieces.insert((piece.kind, piece.id.clone()), piece).is_none())
}

/// Forgets a generation piece.
pub fn unregister_piece(kind: PieceKind, id: &str) -> bool {
    with_pieces(|pieces| pieces.remove(&(kind, id.to_owned())).is_some())
}

/// A generation piece of a kind.
#[must_use]
pub fn get_piece(kind: PieceKind, id: &str) -> Option<GenerationPiece> {
    with_pieces(|pieces| pieces.get(&(kind, id.to_owned())).cloned())
}

/// Every generation piece, or every one of a kind.
#[must_use]
pub fn get_pieces(kind: Option<PieceKind>) -> Vec<GenerationPiece> {
    with_pieces(|pieces| {
        pieces
            .values()
            .filter(|piece| kind.is_none_or(|kind| piece.kind == kind))
            .cloned()
            .collect()
    })
}

/// Registers a feature.
pub fn register_feature(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

pub fn unregister_feature(id: &str) -> bool {
    unregister_piece(PieceKind::Feature, id)
}

/// Registers a structure.
pub fn register_structure(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

pub fn unregister_structure(id: &str) -> bool {
    unregister_piece(PieceKind::Structure, id)
}

/// Registers an ore.
pub fn register_ore(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers a carver.
pub fn register_carver(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers a surface rule.
pub fn register_surface_rule(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers a placement rule.
pub fn register_placement_rule(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers noise settings.
pub fn register_noise(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers a density function.
pub fn register_density_function(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers a dimension, which is section 15's registry for the generation side.
pub fn register_dimension(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers a dimension type.
pub fn register_dimension_type(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers a biome source.
pub fn register_biome_source(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Registers a chunk generator a mod owns.
pub fn register_chunk_generator(piece: GenerationPiece) -> bool {
    register_piece(piece)
}

/// Records that a mod wants to be called after a chunk is generated. This host calls its own
/// generator; nothing here calls a mod's callback for the client's chunks.
pub fn register_chunk_callback(id: &str) -> bool {
    match get_piece(PieceKind::ChunkGenerator, id) {
        Some(piece) => {
            register_piece(piece.setting("chunk_callback", "yes"));
            true
        }
        None => false,
    }
}

/// Records that a mod wants to be called when a chunk is populated.
pub fn register_population_callback(id: &str) -> bool {
    register_chunk_callback(id)
}

/// The noise settings a mod registered.
#[must_use]
pub fn get_noise_settings(id: &str) -> Option<GenerationPiece> {
    get_piece(PieceKind::Noise, id)
}

/// The generation settings of a chunk generator.
#[must_use]
pub fn get_generation_settings(id: &str) -> Option<GenerationPiece> {
    get_piece(PieceKind::ChunkGenerator, id)
}

/// The settings of a structure.
#[must_use]
pub fn get_structure_settings(id: &str) -> Option<GenerationPiece> {
    get_piece(PieceKind::Structure, id)
}

/// Generates a chunk with a generator a mod supplies: for every column the generator gives a
/// height and a block, and the chunk is filled up to it.
#[must_use]
pub fn generate_chunk(
    context: &GenerationContext,
    generator: &dyn Fn(usize, usize, i32) -> String,
) -> Chunk {
    let mut chunk = Chunk::empty(context.chunk);
    let noise = Noise::new(context.seed);
    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_WIDTH {
            let height = surface_height(&noise, context, x, z);
            for y in WORLD_BOTTOM..=height {
                let block = generator(x, z, y);
                chunk.set_block(x, y, z, &block);
            }
            for y in height + 1..WORLD_BOTTOM + CHUNK_HEIGHT as i32 {
                chunk.set_block(x, y, z, "minecraft:air");
            }
        }
    }
    chunk
}

/// The height a column's surface sits at, from the context's seed: fractal noise over a
/// sea-level base, which is the shape terrain has.
#[must_use]
pub fn surface_height(noise: &Noise, context: &GenerationContext, x: usize, z: usize) -> i32 {
    let (origin_x, origin_z) = context.origin();
    let world_x = f64::from(origin_x + x as i32) / 64.0;
    let world_z = f64::from(origin_z + z as i32) / 64.0;
    let shape = noise.fractal(world_x, world_z, 4, 0.5);
    (64.0 + shape * 24.0).round() as i32
}

/// Fills a chunk a mod already has with a generator, which is `populate` for a chunk a mod
/// owns.
pub fn populate_chunk(
    chunk: &mut Chunk,
    context: &GenerationContext,
    generator: &dyn Fn(usize, usize, i32) -> String,
) -> usize {
    let mut placed = 0;
    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_WIDTH {
            let height = chunk.height_at(x, z).unwrap_or(WORLD_BOTTOM);
            let target = generator(x, z, height + 1);
            if chunk.set_block(x, height + 1, z, &target) {
                placed += 1;
            }
        }
        let _ = context;
    }
    placed
}

/// Puts one feature's blocks into a chunk a mod owns, at a position.
pub fn place_feature(
    chunk: &mut Chunk,
    x: usize,
    y: i32,
    z: usize,
    block: &str,
) -> bool {
    chunk.set_block(x, y, z, block)
}

/// Puts one structure's blocks into a chunk a mod owns, from its corner, filling the
/// `(width, height, depth)` box after it.
pub fn place_structure(
    chunk: &mut Chunk,
    x: usize,
    y: i32,
    z: usize,
    size: (usize, usize, usize),
    block: &str,
) -> usize {
    let (width, height, depth) = size;
    let mut placed = 0;
    for dx in 0..width {
        for dy in 0..height {
            for dz in 0..depth {
                if chunk.set_block(x + dx, y + dy as i32, z + dz, block) {
                    placed += 1;
                }
            }
        }
    }
    placed
}

/// The height of a column in a chunk a mod owns.
#[must_use]
pub fn get_height_at(chunk: &Chunk, x: usize, z: usize) -> Option<i32> {
    chunk.height_at(x, z)
}

/// The biome of a column in a chunk a mod owns.
#[must_use]
pub fn get_biome_at(chunk: &Chunk, x: usize, z: usize) -> Option<String> {
    chunk.biome_at(x, z).map(str::to_owned)
}

/// Adds a placed feature to a biome a mod declared.
pub fn add_placed_feature(biome: &str, feature: &str) -> bool {
    with_biomes(|biomes| match biomes.get_mut(biome) {
        Some(biome) => {
            if !biome.features.iter().any(|held| held == feature) {
                biome.features.push(feature.to_owned());
            }
            true
        }
        None => false,
    })
}

/// Adds a structure set to a biome a mod declared.
pub fn add_structure_set(biome: &str, structure: &str) -> bool {
    with_biomes(|biomes| match biomes.get_mut(biome) {
        Some(biome) => {
            if !biome.structures.iter().any(|held| held == structure) {
                biome.structures.push(structure.to_owned());
            }
            true
        }
        None => false,
    })
}

/// Why a world-generation question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "world" => {
            "the client's world generation is inside the client; nothing here is called when \
             it generates a chunk"
                .to_owned()
        }
        "seed" => "the world seed is the client's, from the server".to_owned(),
        "generator" => "the client's generator is its own, in its own binary".to_owned(),
        "order" => "the order the client generates in is not something this host can reach".to_owned(),
        "random" => {
            "the client's own random is its own; a mod generates with the seeded Random this \
             host provides instead"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["world", "seed", "generator", "order", "random"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's world seed. `None`.
#[must_use]
pub fn get_seed() -> Option<u64> {
    None
}

/// The client's own random. `None`.
#[must_use]
pub fn get_random() -> Option<Random> {
    None
}

/// The client's generator. `None`.
#[must_use]
pub fn get_generator() -> Option<String> {
    None
}

/// Reorders the client's generation. `false`.
pub fn modify_generation_order(_name: &str) -> bool {
    crate::warn!(
        "a mod asked this host to reorder world generation: {}",
        why_unknown("order")
    );
    false
}

/// A generation context pointed at the client's world, which this host cannot build.
#[must_use]
pub fn create_generation_context() -> Option<GenerationContext> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_biomes(|biomes| biomes.clear());
        with_pieces(|pieces| pieces.clear());
    }

    #[test]
    fn a_seeded_random_is_reproducible() {
        let _serial = serial();
        let mut left = Random::new(42);
        let mut right = Random::new(42);
        for _ in 0..8 {
            assert_eq!(left.next_u32(), right.next_u32(), "the same seed gives the same draws");
        }
        let mut other = Random::new(43);
        assert_ne!(Random::new(42).next_u32(), other.next_u32());
        let mut random = Random::new(7);
        for _ in 0..64 {
            let value = random.next_f64();
            assert!((0.0..1.0).contains(&value), "{value}");
        }
        for _ in 0..64 {
            let value = random.next_range(3, 7);
            assert!((3..=7).contains(&value), "{value}");
        }
        assert_eq!(random.next_range(5, 5), 5);
        assert_eq!(random.pick::<u8>(&[]), None);
        assert!(random.pick(&["a", "b"]).is_some());
        // The same chunk of the same seed always starts the same way.
        assert_eq!(
            Random::for_position(9, 1, 2).next_u32(),
            Random::for_position(9, 1, 2).next_u32()
        );
        assert!(Random::new(1).chance(1.0));
        assert!(!Random::new(1).chance(0.0));
    }

    #[test]
    fn noise_is_coherent_and_bounded() {
        let _serial = serial();
        let noise = Noise::new(42);
        assert_eq!(noise.seed(), 42);
        for x in 0..40 {
            for z in 0..40 {
                let value = noise.at_2d(f64::from(x) / 8.0, f64::from(z) / 8.0);
                assert!((-1.0..=1.0).contains(&value), "{value}");
            }
        }
        // Nearby points are closer together than distant ones: it is coherent, not white.
        // One pair can coincide by chance, so this looks at a hundred of each.
        let mut near = 0.0f64;
        let mut far = 0.0f64;
        for step in 0..100 {
            let x = f64::from(step) * 0.1;
            near = near.max((noise.at_2d(x, 0.0) - noise.at_2d(x + 0.001, 0.0)).abs());
            far = far.max((noise.at_2d(x, 0.0) - noise.at_2d(x + 5.0, 0.0)).abs());
        }
        assert!(near < far, "nearby {near} should be closer than distant {far}");
        assert!(near < 0.1, "and a step that small barely moves it: {near}");
        // The same seed gives the same noise, and another seed does not.
        assert_eq!(noise.at_2d(1.5, 2.5), Noise::new(42).at_2d(1.5, 2.5));
        assert_ne!(noise.at_2d(1.5, 2.5), Noise::new(43).at_2d(1.5, 2.5));
        let fractal = noise.fractal(0.5, 0.5, 4, 0.5);
        assert!((-1.0..=1.0).contains(&fractal), "{fractal}");
        assert_eq!(noise.fractal(0.5, 0.5, 0, 0.5), 0.0);
        assert!(noise.to_string().contains("42"));
    }

    #[test]
    fn a_biome_is_registered_with_its_settings() {
        let _serial = serial();
        fresh();
        assert!(register_biome(
            Biome::new("hello:sky", "Sky")
                .temperature(0.4)
                .downfall(0.1)
                .surface("minecraft:snow_block", "minecraft:dirt")
                .feature("hello:clouds")
                .structure("hello:tower")
        ));
        let biome = get_biome_settings("hello:sky").expect("registered");
        assert_eq!(biome.name, "Sky");
        assert_eq!(biome.temperature, 0.4);
        assert_eq!(biome.surface_block, "minecraft:snow_block");
        assert_eq!(biome.features, vec!["hello:clouds".to_owned()]);
        assert!(!biome.registered, "the client was never told");
        assert!(biome.to_string().contains("Sky"));
        assert_eq!(get_biome_by_name("Sky").map(|biome| biome.id), Some("hello:sky".to_owned()));
        assert_eq!(get_biomes().len(), 1);
        assert!(add_placed_feature("hello:sky", "hello:trees"));
        assert!(!add_placed_feature("nothing", "hello:trees"));
        assert!(add_structure_set("hello:sky", "hello:village"));
        assert_eq!(
            get_biome_settings("hello:sky").map(|biome| biome.features.len()),
            Some(2)
        );
        assert!(!register_biome(Biome::new("hello:sky", "Sky again")), "it replaces");
        assert!(unregister_biome("hello:sky"));
        assert!(!unregister_biome("hello:sky"));
    }

    #[test]
    fn every_generation_piece_registers_under_its_own_kind() {
        let _serial = serial();
        fresh();
        assert!(register_feature(
            GenerationPiece::new(PieceKind::Feature, "hello:trees")
                .named("Trees")
                .setting("count", "8")
        ));
        assert!(register_ore(GenerationPiece::new(PieceKind::Ore, "hello:copper")));
        assert!(register_carver(GenerationPiece::new(PieceKind::Carver, "hello:cave")));
        assert!(register_surface_rule(GenerationPiece::new(PieceKind::SurfaceRule, "hello:rule")));
        assert!(register_placement_rule(GenerationPiece::new(PieceKind::PlacementRule, "hello:place")));
        assert!(register_noise(GenerationPiece::new(PieceKind::Noise, "hello:noise")));
        assert!(register_density_function(GenerationPiece::new(PieceKind::DensityFunction, "hello:density")));
        assert!(register_dimension(GenerationPiece::new(PieceKind::Dimension, "hello:sky")));
        assert!(register_dimension_type(GenerationPiece::new(PieceKind::DimensionType, "hello:sky_type")));
        assert!(register_biome_source(GenerationPiece::new(PieceKind::BiomeSource, "hello:source")));
        assert!(register_chunk_generator(GenerationPiece::new(PieceKind::ChunkGenerator, "hello:gen")));
        assert!(register_structure(GenerationPiece::new(PieceKind::Structure, "hello:tower")));

        assert_eq!(get_pieces(None).len(), 12);
        assert_eq!(get_pieces(Some(PieceKind::Feature)).len(), 1);
        assert_eq!(get_pieces(Some(PieceKind::Ore)).len(), 1);
        let feature = get_piece(PieceKind::Feature, "hello:trees").expect("registered");
        assert_eq!(feature.name, "Trees");
        assert_eq!(feature.setting_of("count"), Some("8"));
        assert_eq!(feature.setting_of("nothing"), None);
        assert!(get_noise_settings("hello:noise").is_some());
        assert!(get_generation_settings("hello:gen").is_some());
        assert!(get_structure_settings("hello:tower").is_some());
        assert!(register_chunk_callback("hello:gen"));
        assert!(!register_chunk_callback("nothing"));
        assert!(!register_population_callback("nothing"));
        assert_eq!(PieceKind::Noise.as_str(), "noise");
        assert!(unregister_feature("hello:trees"));
        assert!(!unregister_feature("hello:trees"));
        assert!(unregister_structure("hello:tower"));
        assert!(!unregister_piece(PieceKind::Ore, "nothing"));
    }

    #[test]
    fn a_chunk_a_mod_generates_holds_what_it_was_told() {
        let _serial = serial();
        let mut chunk = Chunk::empty((0, 0));
        assert!(chunk.set_block(0, 64, 0, "minecraft:stone"));
        assert!(!chunk.set_block(16, 64, 0, "minecraft:stone"), "outside the chunk");
        assert!(!chunk.set_block(0, WORLD_BOTTOM - 1, 0, "minecraft:stone"), "below the world");
        assert!(!chunk.set_block(0, WORLD_BOTTOM + CHUNK_HEIGHT as i32, 0, "minecraft:stone"));
        assert_eq!(chunk.block(0, 64, 0), Some("minecraft:stone"));
        assert_eq!(chunk.block(16, 64, 0), None);
        assert_eq!(chunk.height_at(0, 0), Some(64));
        assert_eq!(chunk.height_at(16, 0), None);
        assert!(chunk.set_biome(0, 0, "hello:sky"));
        assert_eq!(chunk.biome_at(0, 0), Some("hello:sky"));
        assert!(!chunk.set_biome(16, 0, "hello:sky"));
        assert_eq!(get_biome_at(&chunk, 0, 0), Some("hello:sky".to_owned()));
        assert_eq!(get_height_at(&chunk, 0, 0), Some(64));
        assert_eq!(chunk.solid_count(), 1);
        assert!(chunk.to_string().contains("1 solid"));
        assert_eq!(level_of(WORLD_BOTTOM), Some(0));
        assert_eq!(level_of(WORLD_BOTTOM - 1), None);
    }

    #[test]
    fn a_generator_makes_the_same_chunk_twice_from_the_same_seed() {
        let _serial = serial();
        let context = GenerationContext::new(42, (0, 0));
        assert_eq!(context.origin(), (0, 0));
        assert_eq!(context.dimension, "minecraft:overworld");
        let generator = |_x: usize, _z: usize, y: i32| -> String {
            if y < 64 {
                "minecraft:stone".to_owned()
            } else {
                "minecraft:air".to_owned()
            }
        };
        let left = generate_chunk(&context, &generator);
        let right = generate_chunk(&context, &generator);
        assert_eq!(left.blocks, right.blocks, "the same seed makes the same chunk");
        assert!(left.solid_count() > 0);
        assert!(left.height_at(0, 0).is_some());

        let elsewhere = GenerationContext::new(42, (1, 0));
        assert_eq!(elsewhere.origin(), (16, 0));
        let other = generate_chunk(&elsewhere, &generator);
        assert_ne!(
            left.heights, other.heights,
            "the same seed at a different chunk samples different noise"
        );

        let noise = Noise::new(42);
        let height = surface_height(&noise, &context, 0, 0);
        assert!((40..90).contains(&height), "{height}");

        let mut chunk = Chunk::empty((0, 0));
        chunk.set_block(0, 64, 0, "minecraft:stone");
        let grass = |_x: usize, _z: usize, _y: i32| -> String {
            "minecraft:grass_block".to_owned()
        };
        let placed = populate_chunk(&mut chunk, &context, &grass);
        assert_eq!(placed, CHUNK_WIDTH * CHUNK_WIDTH);
        assert_eq!(chunk.block(0, 65, 0), Some("minecraft:grass_block"));
        assert!(place_feature(&mut chunk, 1, 65, 1, "minecraft:oak_log"));
        assert_eq!(
            place_structure(&mut chunk, 2, 65, 2, (2, 2, 2), "minecraft:oak_planks"),
            8
        );
    }

    #[test]
    fn the_clients_own_generation_is_refused_with_a_reason() {
        let _serial = serial();
        assert!(get_seed().is_none());
        assert!(get_random().is_none());
        assert!(get_generator().is_none());
        assert!(!modify_generation_order("hello:first"));
        assert!(create_generation_context().is_none());
        assert_eq!(refusals().len(), 5);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
