//! Structures: the templates a mod builds and transforms, and the client's world it cannot touch.
//!
//! A **template** is real data a mod owns: a size, a block palette, block placements with
//! states, and jigsaw/anchor points with directions. The transforms are real too —
//! [`rotate`] and [`mirror`] move a local coordinate exactly, [`Template::transformed`]
//! produces a mirrored-then-rotated template, and [`place_template`] answers the absolute
//! positions it would occupy, with a [`Bounds`] that a target box can accept or reject.
//!
//! What is *not* reachable is the client's own structure generation, its structures in its
//! world, and placing a template into that world; those answer `None` or `false` with the
//! reason from [`why_unknown`]. Nothing here fakes a success.
//!
//! ```
//! use cinnaloader::structures::{self, Mirror, Rotation, Template};
//!
//! let template = Template::new("hello:hut", (2, 1, 2))
//!     .block((0, 0, 0), "minecraft:oak_planks")
//!     .block((1, 0, 1), "minecraft:glass");
//! let placed = structures::place_template(&template, (10, 64, 10), Rotation::Cw90, Mirror::None);
//! assert_eq!(placed.blocks.len(), 2);
//! assert!(placed.fits(&structures::Bounds::new((0, 64, 0), (31, 80, 31))));
//! ```

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

/// A block position, local or absolute.
pub type Pos = (i32, i32, i32);

/// How far a local coordinate is turned around Y.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    #[default]
    None,
    Cw90,
    Cw180,
    Cw270,
}

impl Rotation {
    #[must_use]
    pub fn degrees(self) -> i32 {
        match self {
            Self::None => 0,
            Self::Cw90 => 90,
            Self::Cw180 => 180,
            Self::Cw270 => 270,
        }
    }

    /// Where a local coordinate lands after the turn, inside a `size` box.
    #[must_use]
    pub fn rotate(self, pos: Pos, size: Pos) -> Pos {
        let (x, y, z) = pos;
        let (sx, _, sz) = size;
        match self {
            Self::None => (x, y, z),
            Self::Cw90 => (sz - 1 - z, y, x),
            Self::Cw180 => (sx - 1 - x, y, sz - 1 - z),
            Self::Cw270 => (z, y, sx - 1 - x),
        }
    }

    /// The size after the turn: a quarter turn swaps width and depth.
    #[must_use]
    pub fn rotate_size(self, size: Pos) -> Pos {
        match self {
            Self::None | Self::Cw180 => size,
            Self::Cw90 | Self::Cw270 => (size.2, size.1, size.0),
        }
    }
}

/// Which axis a mirror flips across.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mirror {
    #[default]
    None,
    LeftRight,
    FrontBack,
}

impl Mirror {
    /// Where a local coordinate lands after the flip, inside a `size` box.
    #[must_use]
    pub fn mirror(self, pos: Pos, size: Pos) -> Pos {
        let (x, y, z) = pos;
        let (sx, _, sz) = size;
        match self {
            Self::None => (x, y, z),
            Self::LeftRight => (sx - 1 - x, y, z),
            Self::FrontBack => (x, y, sz - 1 - z),
        }
    }
}

/// A face of a block or a jigsaw anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Direction {
    #[default]
    North,
    South,
    East,
    West,
    Up,
    Down,
}

impl Direction {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::North => "north",
            Self::South => "south",
            Self::East => "east",
            Self::West => "west",
            Self::Up => "up",
            Self::Down => "down",
        }
    }

    /// The face pointing the other way.
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }
}

impl FromStr for Direction {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "north" => Ok(Self::North),
            "south" => Ok(Self::South),
            "east" => Ok(Self::East),
            "west" => Ok(Self::West),
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            _ => Err(()),
        }
    }
}

/// An axis-aligned box, the template's own or a target to test it against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub min: Pos,
    pub max: Pos,
}

impl Bounds {
    #[must_use]
    pub fn new(min: Pos, max: Pos) -> Self {
        Self { min, max }
    }

    /// The box a template of a size occupies from the origin.
    #[must_use]
    pub fn from_size(size: Pos) -> Self {
        Self {
            min: (0, 0, 0),
            max: (size.0 - 1, size.1 - 1, size.2 - 1),
        }
    }

    /// How many blocks along each axis.
    #[must_use]
    pub fn size(&self) -> Pos {
        (
            self.max.0 - self.min.0 + 1,
            self.max.1 - self.min.1 + 1,
            self.max.2 - self.min.2 + 1,
        )
    }

    #[must_use]
    pub fn volume(&self) -> i64 {
        let size = self.size();
        i64::from(size.0) * i64::from(size.1) * i64::from(size.2)
    }

    #[must_use]
    pub fn contains(&self, pos: Pos) -> bool {
        pos.0 >= self.min.0
            && pos.0 <= self.max.0
            && pos.1 >= self.min.1
            && pos.1 <= self.max.1
            && pos.2 >= self.min.2
            && pos.2 <= self.max.2
    }

    /// Whether this whole box sits inside `other`.
    #[must_use]
    pub fn fits_within(&self, other: &Self) -> bool {
        other.contains(self.min) && other.contains(self.max)
    }

    /// The eight corners, for drawing or a fit check.
    #[must_use]
    pub fn corners(&self) -> Vec<Pos> {
        let mut corners = Vec::with_capacity(8);
        for x in [self.min.0, self.max.0] {
            for y in [self.min.1, self.max.1] {
                for z in [self.min.2, self.max.2] {
                    corners.push((x, y, z));
                }
            }
        }
        corners
    }
}

/// One block in a template, as a palette index and its states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub pos: Pos,
    body: usize,
    pub properties: Vec<(String, String)>,
}

impl Placement {
    #[must_use]
    pub fn palette_index(&self) -> usize {
        self.body
    }
}

/// A jigsaw or anchor point, and the face it points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jigsaw {
    pub pos: Pos,
    pub name: String,
    pub target: String,
    pub pool: String,
    pub direction: Direction,
}

impl Jigsaw {
    #[must_use]
    pub fn new(pos: Pos, name: &str, direction: Direction) -> Self {
        Self {
            pos,
            name: name.to_owned(),
            target: String::new(),
            pool: String::new(),
            direction,
        }
    }

    #[must_use]
    pub fn target(mut self, target: &str) -> Self {
        self.target = target.to_owned();
        self
    }

    #[must_use]
    pub fn pool(mut self, pool: &str) -> Self {
        self.pool = pool.to_owned();
        self
    }
}

/// A template: a size, a palette of block names, placements and jigsaw points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub size: Pos,
    pub palette: Vec<String>,
    pub placements: Vec<Placement>,
    pub jigsaws: Vec<Jigsaw>,
}

impl Template {
    #[must_use]
    pub fn new(id: &str, size: Pos) -> Self {
        Self {
            id: id.to_owned(),
            name: id.to_owned(),
            size,
            palette: Vec::new(),
            placements: Vec::new(),
            jigsaws: Vec::new(),
        }
    }

    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    fn palette_index(&mut self, block: &str) -> usize {
        match self.palette.iter().position(|held| held == block) {
            Some(index) => index,
            None => {
                self.palette.push(block.to_owned());
                self.palette.len() - 1
            }
        }
    }

    #[must_use]
    pub fn block(self, pos: Pos, block: &str) -> Self {
        self.block_state(pos, block, &[])
    }

    #[must_use]
    pub fn block_state(mut self, pos: Pos, block: &str, properties: &[(&str, &str)]) -> Self {
        let body = self.palette_index(block);
        self.placements.push(Placement {
            pos,
            body,
            properties: properties
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        });
        self
    }

    #[must_use]
    pub fn jigsaw(mut self, jigsaw: Jigsaw) -> Self {
        self.jigsaws.push(jigsaw);
        self
    }

    /// The box the template occupies from the origin.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        Bounds::from_size(self.size)
    }

    /// The block at a local position, if the template places one there.
    #[must_use]
    pub fn block_at(&self, pos: Pos) -> Option<&str> {
        self.placements
            .iter()
            .find(|placement| placement.pos == pos)
            .and_then(|placement| self.palette.get(placement.body))
            .map(String::as_str)
    }

    /// The same template, mirrored first and then turned: the order jigsaw assembly uses.
    #[must_use]
    pub fn transformed(&self, mirror: Mirror, rotation: Rotation) -> Self {
        let size = self.size;
        let move_pos = |pos: Pos| rotation.rotate(mirror.mirror(pos, size), size);
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            size: rotation.rotate_size(size),
            palette: self.palette.clone(),
            placements: self
                .placements
                .iter()
                .map(|placement| Placement {
                    pos: move_pos(placement.pos),
                    body: placement.body,
                    properties: placement.properties.clone(),
                })
                .collect(),
            jigsaws: self
                .jigsaws
                .iter()
                .map(|jigsaw| Jigsaw {
                    pos: move_pos(jigsaw.pos),
                    ..jigsaw.clone()
                })
                .collect(),
        }
    }

    /// Places the template at an origin, answering the absolute positions it occupies.
    #[must_use]
    pub fn place(&self, origin: Pos, rotation: Rotation, mirror: Mirror) -> Placed {
        place_template(self, origin, rotation, mirror)
    }
}

/// A template placed at an origin: the absolute blocks, the box, and whether it fits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub template: String,
    pub origin: Pos,
    pub rotation: Rotation,
    pub mirror: Mirror,
    pub size: Pos,
    pub blocks: Vec<(Pos, String)>,
    pub bounds: Bounds,
}

impl Placed {
    #[must_use]
    pub fn position_count(&self) -> usize {
        self.blocks.len()
    }

    #[must_use]
    pub fn block_at(&self, pos: Pos) -> Option<&str> {
        self.blocks
            .iter()
            .find(|(held, _)| *held == pos)
            .map(|(_, block)| block.as_str())
    }

    /// Whether the placed template fits inside a target box.
    #[must_use]
    pub fn fits(&self, target: &Bounds) -> bool {
        self.bounds.fits_within(target)
    }
}

/// A registered structure: a template, and the pieces it is assembled from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Structure {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub template: String,
    pub placement_rule: Option<String>,
    pub piece_type: Option<String>,
    pub processor: Option<String>,
    pub pool: Option<String>,
    pub settings: Vec<(String, String)>,
    pub registered: bool,
}

impl Structure {
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            template: String::new(),
            placement_rule: None,
            piece_type: None,
            processor: None,
            pool: None,
            settings: Vec::new(),
            registered: false,
        }
    }

    #[must_use]
    pub fn template(mut self, template: &str) -> Self {
        self.template = template.to_owned();
        self
    }

    #[must_use]
    pub fn placement_rule(mut self, rule: &str) -> Self {
        self.placement_rule = Some(rule.to_owned());
        self
    }

    #[must_use]
    pub fn piece_type(mut self, piece_type: &str) -> Self {
        self.piece_type = Some(piece_type.to_owned());
        self
    }

    #[must_use]
    pub fn processor(mut self, processor: &str) -> Self {
        self.processor = Some(processor.to_owned());
        self
    }

    #[must_use]
    pub fn pool(mut self, pool: &str) -> Self {
        self.pool = Some(pool.to_owned());
        self
    }

    #[must_use]
    pub fn setting(mut self, key: &str, value: &str) -> Self {
        self.settings.retain(|(held, _)| held != key);
        self.settings.push((key.to_owned(), value.to_owned()));
        self
    }
}

/// Which role a declaration plays in assembling a structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeclarationKind {
    PlacementRule,
    PieceType,
    Processor,
    Pool,
}

impl DeclarationKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlacementRule => "placement_rule",
            Self::PieceType => "piece_type",
            Self::Processor => "processor",
            Self::Pool => "pool",
        }
    }
}

/// A named declaration a structure refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub id: String,
    pub name: String,
    pub kind: DeclarationKind,
    pub owner: String,
    pub settings: Vec<(String, String)>,
}

impl Declaration {
    #[must_use]
    pub fn new(kind: DeclarationKind, id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            kind,
            owner: crate::metadata::id(),
            settings: Vec::new(),
        }
    }

    #[must_use]
    pub fn setting(mut self, key: &str, value: &str) -> Self {
        self.settings.retain(|(held, _)| held != key);
        self.settings.push((key.to_owned(), value.to_owned()));
        self
    }
}

fn templates() -> &'static Mutex<BTreeMap<String, Template>> {
    static TEMPLATES: OnceLock<Mutex<BTreeMap<String, Template>>> = OnceLock::new();
    TEMPLATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_templates<T>(body: impl FnOnce(&mut BTreeMap<String, Template>) -> T) -> T {
    let mut guard = templates().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn structures() -> &'static Mutex<BTreeMap<String, Structure>> {
    static STRUCTURES: OnceLock<Mutex<BTreeMap<String, Structure>>> = OnceLock::new();
    STRUCTURES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_structures<T>(body: impl FnOnce(&mut BTreeMap<String, Structure>) -> T) -> T {
    let mut guard = structures().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

type DeclarationKey = (DeclarationKind, String);

fn declarations() -> &'static Mutex<BTreeMap<DeclarationKey, Declaration>> {
    static DECLARATIONS: OnceLock<Mutex<BTreeMap<DeclarationKey, Declaration>>> = OnceLock::new();
    DECLARATIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_declarations<T>(body: impl FnOnce(&mut BTreeMap<DeclarationKey, Declaration>) -> T) -> T {
    let mut guard = declarations().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a template, or replaces the one with that id.
pub fn register_template(template: Template) -> bool {
    with_templates(|templates| templates.insert(template.id.clone(), template).is_none())
}

/// A registered template, by id.
#[must_use]
pub fn get_template(id: &str) -> Option<Template> {
    with_templates(|templates| templates.get(id).cloned())
}

/// Every registered template, in id order.
#[must_use]
pub fn get_templates() -> Vec<Template> {
    with_templates(|templates| templates.values().cloned().collect())
}

/// Registers a structure, or replaces the one with that id.
pub fn register(structure: Structure) -> bool {
    with_structures(|structures| structures.insert(structure.id.clone(), structure).is_none())
}

/// Forgets a registered structure.
pub fn unregister(id: &str) -> bool {
    with_structures(|structures| structures.remove(id).is_some())
}

/// A registered structure, by id.
#[must_use]
pub fn get(id: &str) -> Option<Structure> {
    with_structures(|structures| structures.get(id).cloned())
}

/// Every registered structure, in id order.
#[must_use]
pub fn get_all() -> Vec<Structure> {
    with_structures(|structures| structures.values().cloned().collect())
}

/// Where a local coordinate lands after a turn, inside a `size` box.
#[must_use]
pub fn rotate(pos: Pos, size: Pos, rotation: Rotation) -> Pos {
    rotation.rotate(pos, size)
}

/// Where a local coordinate lands after a mirror, inside a `size` box.
#[must_use]
pub fn mirror(pos: Pos, size: Pos, mirror: Mirror) -> Pos {
    mirror.mirror(pos, size)
}

/// Places an owned template at an origin, mirrored then turned.
#[must_use]
pub fn place_template(template: &Template, origin: Pos, rotation: Rotation, mirror: Mirror) -> Placed {
    let transformed = template.transformed(mirror, rotation);
    let blocks = transformed
        .placements
        .iter()
        .filter_map(|placement| {
            transformed.palette.get(placement.body).map(|block| {
                (
                    (
                        origin.0 + placement.pos.0,
                        origin.1 + placement.pos.1,
                        origin.2 + placement.pos.2,
                    ),
                    block.clone(),
                )
            })
        })
        .collect();
    let bounds = Bounds::new(
        origin,
        (
            origin.0 + transformed.size.0 - 1,
            origin.1 + transformed.size.1 - 1,
            origin.2 + transformed.size.2 - 1,
        ),
    );
    Placed {
        template: transformed.id,
        origin,
        rotation,
        mirror,
        size: transformed.size,
        blocks,
        bounds,
    }
}

/// Places a registered structure at an origin, through its template.
#[must_use]
pub fn place(id: &str, origin: Pos, rotation: Rotation, mirror: Mirror) -> Option<Placed> {
    let structure = get(id)?;
    let template = get_template(&structure.template)?;
    Some(place_template(&template, origin, rotation, mirror))
}

/// Places a registered structure, picking its turn and mirror from a seed, which is how the
/// game varies one structure per placement.
#[must_use]
pub fn generate(id: &str, origin: Pos, seed: u64) -> Option<Placed> {
    let structure = get(id)?;
    let template = get_template(&structure.template)?;
    let mut random = crate::worldgen::Random::new(seed);
    let rotation = match random.next_range(0, 3) {
        0 => Rotation::None,
        1 => Rotation::Cw90,
        2 => Rotation::Cw180,
        _ => Rotation::Cw270,
    };
    let mirror = match random.next_range(0, 2) {
        0 => Mirror::None,
        1 => Mirror::LeftRight,
        _ => Mirror::FrontBack,
    };
    Some(place_template(&template, origin, rotation, mirror))
}

/// The box of a registered template, by template id or by structure id.
#[must_use]
pub fn get_bounds(id: &str) -> Option<Bounds> {
    if let Some(template) = get_template(id) {
        return Some(template.bounds());
    }
    let structure = get(id)?;
    get_template(&structure.template).map(|template| template.bounds())
}

/// The jigsaw or anchor pieces of a template, by template id or by structure id.
#[must_use]
pub fn get_pieces(id: &str) -> Vec<Jigsaw> {
    if let Some(template) = get_template(id) {
        return template.jigsaws;
    }
    match get(id).and_then(|structure| get_template(&structure.template)) {
        Some(template) => template.jigsaws,
        None => Vec::new(),
    }
}

fn register_declaration(mut declaration: Declaration, kind: DeclarationKind) -> bool {
    declaration.kind = kind;
    with_declarations(|declarations| {
        declarations
            .insert((kind, declaration.id.clone()), declaration)
            .is_none()
    })
}

pub fn register_placement_rule(declaration: Declaration) -> bool {
    register_declaration(declaration, DeclarationKind::PlacementRule)
}

pub fn register_piece_type(declaration: Declaration) -> bool {
    register_declaration(declaration, DeclarationKind::PieceType)
}

pub fn register_processor(declaration: Declaration) -> bool {
    register_declaration(declaration, DeclarationKind::Processor)
}

pub fn register_pool(declaration: Declaration) -> bool {
    register_declaration(declaration, DeclarationKind::Pool)
}

/// A declaration of a kind, by id.
#[must_use]
pub fn get_declaration(kind: DeclarationKind, id: &str) -> Option<Declaration> {
    with_declarations(|declarations| declarations.get(&(kind, id.to_owned())).cloned())
}

/// Every declaration of a kind, in id order.
#[must_use]
pub fn get_declarations(kind: DeclarationKind) -> Vec<Declaration> {
    with_declarations(|declarations| {
        declarations
            .values()
            .filter(|declaration| declaration.kind == kind)
            .cloned()
            .collect()
    })
}

fn encode_template(template: &Template) -> String {
    let mut out = format!(
        "template\t{}\t{}\t{}\t{}\n",
        template.id, template.size.0, template.size.1, template.size.2
    );
    out.push_str(&format!("name\t{}\n", template.name));
    for block in &template.palette {
        out.push_str(&format!("palette\t{block}\n"));
    }
    for placement in &template.placements {
        out.push_str(&format!(
            "block\t{}\t{}\t{}\t{}\n",
            placement.pos.0, placement.pos.1, placement.pos.2, placement.body
        ));
        for (key, value) in &placement.properties {
            out.push_str(&format!("prop\t{key}\t{value}\n"));
        }
    }
    for jigsaw in &template.jigsaws {
        out.push_str(&format!(
            "jigsaw\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            jigsaw.pos.0,
            jigsaw.pos.1,
            jigsaw.pos.2,
            jigsaw.name,
            jigsaw.target,
            jigsaw.pool,
            jigsaw.direction.as_str()
        ));
    }
    out
}

fn decode_template(text: &str) -> Option<Template> {
    let mut lines = text.lines();
    let mut header = lines.next()?.split('\t');
    if header.next()? != "template" {
        return None;
    }
    let id = header.next()?.to_owned();
    let size = (
        header.next()?.parse().ok()?,
        header.next()?.parse().ok()?,
        header.next()?.parse().ok()?,
    );
    let mut template = Template {
        id,
        name: String::new(),
        size,
        palette: Vec::new(),
        placements: Vec::new(),
        jigsaws: Vec::new(),
    };
    let mut last = None;
    for line in lines {
        let mut fields = line.split('\t');
        match fields.next()? {
            "name" => template.name = fields.collect::<Vec<_>>().join("\t"),
            "palette" => template.palette.push(fields.next().unwrap_or("").to_owned()),
            "block" => {
                let pos = (
                    fields.next()?.parse().ok()?,
                    fields.next()?.parse().ok()?,
                    fields.next()?.parse().ok()?,
                );
                let body = fields.next()?.parse().ok()?;
                template.placements.push(Placement {
                    pos,
                    body,
                    properties: Vec::new(),
                });
                last = Some(template.placements.len() - 1);
            }
            "prop" => {
                let index = last?;
                let key = fields.next()?.to_owned();
                let value = fields.collect::<Vec<_>>().join("\t");
                template.placements[index].properties.push((key, value));
            }
            "jigsaw" => {
                let pos = (
                    fields.next()?.parse().ok()?,
                    fields.next()?.parse().ok()?,
                    fields.next()?.parse().ok()?,
                );
                let name = fields.next()?.to_owned();
                let target = fields.next()?.to_owned();
                let pool = fields.next()?.to_owned();
                let direction = fields.next()?.parse().ok()?;
                template.jigsaws.push(Jigsaw {
                    pos,
                    name,
                    target,
                    pool,
                    direction,
                });
            }
            _ => {}
        }
    }
    Some(template)
}

/// Writes a registered template as text, or `None` when there is no such template.
#[must_use]
pub fn save_template(id: &str) -> Option<String> {
    get_template(id).map(|template| encode_template(&template))
}

/// Reads a template back from text, or `None` when it is malformed.
#[must_use]
pub fn load_template(text: &str) -> Option<Template> {
    decode_template(text)
}

/// Why a structure question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "structure" => {
            "the client's structures are generated inside its own chunk generator; nothing \
             here is called when it places one"
                .to_owned()
        }
        "placement" => {
            "putting a structure into the world is the client's world state, and needs the \
             hooks section 10 lists as never"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about structures in the client's world.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["structure", "placement"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn place_in_world(id: &str, origin: Pos, rotation: Rotation, mirror: Mirror) -> bool {
    crate::warn!(
        "a mod asked this host to place {id} at {origin:?} ({rotation:?}, {mirror:?}) in the \
         client's world: {}",
        why_unknown("placement")
    );
    false
}

/// The structure half that only the client can answer.
pub mod known_only_to_the_client {
    use super::{why_unknown, Pos};

    /// The structure the client generated at a position, which this host cannot read.
    #[must_use]
    pub fn get_at(_x: i32, _y: i32, _z: i32) -> Option<String> {
        crate::warn!(
            "a mod asked this host for the structure at a position: {}",
            why_unknown("structure")
        );
        None
    }

    /// The structures the client generated, which this host cannot list.
    #[must_use]
    pub fn get_structures() -> Vec<String> {
        crate::warn!(
            "a mod asked this host to list the client's structures: {}",
            why_unknown("structure")
        );
        Vec::new()
    }

    /// Puts a structure into the client's world, which this host cannot do.
    pub fn place_structure(id: &str, origin: Pos) -> bool {
        super::place_in_world(id, origin, super::Rotation::None, super::Mirror::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_templates(|templates| templates.clear());
        with_structures(|structures| structures.clear());
        with_declarations(|declarations| declarations.clear());
    }

    fn hut() -> Template {
        Template::new("hello:hut", (3, 1, 2))
            .named("Hut")
            .block_state((0, 0, 0), "minecraft:oak_planks", &[("facing", "south")])
            .block((2, 0, 1), "minecraft:glass")
            .jigsaw(
                Jigsaw::new((0, 0, 0), "hello:door", Direction::South)
                    .target("hello:hall")
                    .pool("hello:pools"),
            )
    }

    #[test]
    fn a_template_places_blocks_at_absolute_positions() {
        let _serial = serial();
        fresh();
        let template = hut();
        assert_eq!(template.bounds(), Bounds::new((0, 0, 0), (2, 0, 1)));
        assert_eq!(template.block_at((0, 0, 0)), Some("minecraft:oak_planks"));
        assert_eq!(template.block_at((1, 0, 0)), None);
        let placed = place_template(&template, (10, 64, 10), Rotation::None, Mirror::None);
        assert_eq!(placed.position_count(), 2);
        assert_eq!(placed.size, (3, 1, 2));
        assert_eq!(placed.bounds, Bounds::new((10, 64, 10), (12, 64, 11)));
        assert_eq!(placed.block_at((12, 64, 11)), Some("minecraft:glass"));
        assert_eq!(placed.block_at((10, 64, 10)), Some("minecraft:oak_planks"));
        assert!(place_template(&template, (0, 0, 0), Rotation::None, Mirror::None)
            .fits(&Bounds::new((0, 0, 0), (2, 0, 1))));
    }

    #[test]
    fn rotations_move_a_local_coordinate_exactly() {
        let _serial = serial();
        let size = (3, 1, 2);
        assert_eq!(rotate((2, 0, 1), size, Rotation::None), (2, 0, 1));
        assert_eq!(rotate((2, 0, 1), size, Rotation::Cw90), (0, 0, 2));
        assert_eq!(rotate((2, 0, 1), size, Rotation::Cw180), (0, 0, 0));
        assert_eq!(rotate((2, 0, 1), size, Rotation::Cw270), (1, 0, 0));
        assert_eq!(Rotation::Cw90.rotate_size(size), (2, 1, 3));
        assert_eq!(Rotation::Cw180.rotate_size(size), (3, 1, 2));
        assert_eq!(Rotation::Cw270.rotate_size(size), (2, 1, 3));
        // Four quarter turns come back to where they started.
        let turned = [Rotation::Cw90, Rotation::Cw90, Rotation::Cw90, Rotation::Cw90];
        let mut pos = (2, 0, 1);
        let mut box_size = size;
        for step in turned {
            pos = step.rotate(pos, box_size);
            box_size = step.rotate_size(box_size);
        }
        assert_eq!((pos, box_size), ((2, 0, 1), size));
        assert_eq!(Rotation::Cw270.degrees(), 270);
    }

    #[test]
    fn mirrors_flip_and_a_mirrored_template_rotates_predictably() {
        let _serial = serial();
        let size = (3, 1, 2);
        assert_eq!(mirror((0, 0, 0), size, Mirror::None), (0, 0, 0));
        assert_eq!(mirror((0, 0, 0), size, Mirror::LeftRight), (2, 0, 0));
        assert_eq!(mirror((0, 0, 0), size, Mirror::FrontBack), (0, 0, 1));
        let transformed = hut().transformed(Mirror::LeftRight, Rotation::Cw90);
        assert_eq!(transformed.size, (2, 1, 3));
        // The planks at (0,0,0) mirror to (2,0,0), then a quarter turn lands them at (1,0,2).
        assert_eq!(transformed.block_at((1, 0, 2)), Some("minecraft:oak_planks"));
        // The glass at (2,0,1) mirrors to (0,0,1), then lands at (0,0,0).
        assert_eq!(transformed.block_at((0, 0, 0)), Some("minecraft:glass"));
        let placed = place_template(&hut(), (0, 0, 0), Rotation::Cw90, Mirror::LeftRight);
        assert_eq!(
            placed.blocks,
            vec![
                ((1, 0, 2), "minecraft:oak_planks".to_owned()),
                ((0, 0, 0), "minecraft:glass".to_owned()),
            ]
        );
        // The jigsaw anchor turns with the blocks.
        assert_eq!(transformed.jigsaws[0].pos, (1, 0, 2));
    }

    #[test]
    fn a_bounding_box_has_corners_and_a_fit_test() {
        let _serial = serial();
        let bounds = Bounds::new((0, 0, 0), (2, 3, 1));
        assert_eq!(bounds.size(), (3, 4, 2));
        assert_eq!(bounds.volume(), 24);
        assert_eq!(bounds.corners().len(), 8);
        assert!(bounds.contains((2, 3, 1)));
        assert!(!bounds.contains((3, 0, 0)));
        let target = Bounds::new((-1, -1, -1), (4, 4, 4));
        assert!(bounds.fits_within(&target));
        assert!(!target.fits_within(&bounds));
        let outside = Bounds::new((5, 5, 5), (6, 6, 6));
        assert!(!outside.fits_within(&target));
        assert!(!place_template(&hut(), (9, 9, 9), Rotation::None, Mirror::None).fits(&target));
    }

    #[test]
    fn a_template_saves_and_loads_through_its_own_encoding() {
        let _serial = serial();
        fresh();
        let template = hut();
        assert!(register_template(template.clone()));
        assert!(!register_template(template.clone()), "the second replaces the first");
        assert_eq!(get_templates().len(), 1);
        assert_eq!(get_template("hello:hut"), Some(template.clone()));
        let text = save_template("hello:hut").expect("registered");
        assert_eq!(load_template(&text), Some(template));
        assert!(save_template("nothing").is_none());
        assert!(load_template("not a template").is_none());
    }

    #[test]
    fn a_structure_places_by_id_and_generates_the_same_from_a_seed() {
        let _serial = serial();
        fresh();
        assert!(register_template(hut()));
        assert!(register(
            Structure::new("hello:hut", "Hut")
                .template("hello:hut")
                .placement_rule("hello:surface")
                .piece_type("hello:building")
                .processor("hello:rotate")
                .pool("hello:pools")
                .setting("spacing", "8")
        ));
        let structure = get("hello:hut").expect("registered");
        assert_eq!(structure.template, "hello:hut");
        assert_eq!(structure.placement_rule.as_deref(), Some("hello:surface"));
        assert!(!structure.registered, "the client was never told");
        assert_eq!(get_all().len(), 1);
        assert!(
            !register(Structure::new("hello:hut", "Hut again").template("hello:hut")),
            "it replaces"
        );
        assert_eq!(
            get("hello:hut").map(|structure| structure.name),
            Some("Hut again".to_owned())
        );
        let placed = place("hello:hut", (0, 64, 0), Rotation::Cw180, Mirror::None).expect("template");
        assert_eq!(placed.block_at((2, 64, 1)), Some("minecraft:oak_planks"));
        assert_eq!(placed.block_at((0, 64, 0)), Some("minecraft:glass"));
        assert!(place("nothing", (0, 0, 0), Rotation::None, Mirror::None).is_none());
        let first = generate("hello:hut", (0, 64, 0), 42).expect("template");
        let again = generate("hello:hut", (0, 64, 0), 42).expect("template");
        assert_eq!(first, again, "the same seed places the same structure");
        assert!(generate("nothing", (0, 0, 0), 42).is_none());
        assert!(unregister("hello:hut"));
        assert!(!unregister("hello:hut"));
    }

    #[test]
    fn pieces_and_declarations_are_recorded() {
        let _serial = serial();
        fresh();
        register_template(hut());
        let pieces = get_pieces("hello:hut");
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].direction, Direction::South);
        assert_eq!(pieces[0].direction.opposite(), Direction::North);
        assert_eq!(pieces[0].target, "hello:hall");
        assert!(get_pieces("nothing").is_empty());
        assert_eq!(get_bounds("hello:hut"), Some(Bounds::new((0, 0, 0), (2, 0, 1))));
        assert!(get_bounds("nothing").is_none());
        assert!(register_placement_rule(Declaration::new(
            DeclarationKind::Pool,
            "hello:surface",
            "Surface"
        )
        .setting("y", "64")));
        assert_eq!(
            get_declaration(DeclarationKind::PlacementRule, "hello:surface")
                .map(|declaration| declaration.name),
            Some("Surface".to_owned()),
            "the kind is set by the function, not the caller"
        );
        assert!(register_piece_type(Declaration::new(
            DeclarationKind::PlacementRule,
            "hello:building",
            "Building"
        )));
        assert!(register_processor(Declaration::new(
            DeclarationKind::Processor,
            "hello:rotate",
            "Rotate"
        )));
        assert!(register_pool(Declaration::new(DeclarationKind::Pool, "hello:pools", "Pools")));
        assert_eq!(get_declarations(DeclarationKind::PieceType).len(), 1);
        assert_eq!(get_declarations(DeclarationKind::Pool).len(), 1);
    }

    #[test]
    fn the_clients_structures_and_placing_into_its_world_are_refused() {
        let _serial = serial();
        assert!(known_only_to_the_client::get_at(0, 64, 0).is_none());
        assert!(known_only_to_the_client::get_structures().is_empty());
        assert!(!known_only_to_the_client::place_structure("hello:hut", (0, 64, 0)));
        assert_eq!(refusals().len(), 2);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
