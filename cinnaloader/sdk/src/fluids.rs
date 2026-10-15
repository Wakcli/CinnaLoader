//! Fluids: a real flow simulation on a grid a mod owns, and the client's water, out of reach.
//!
//! Water and lava flowing is a cellular process, and a mod can genuinely run one here. A
//! **fluid type** is a declaration — a name, a density, a viscosity, a temperature, a
//! buoyancy, whether it is a liquid, and the block it places — with water and lava built in
//! and any mod free to register its own. A [`FluidGrid`] is a finite grid a mod allocates,
//! with [`set`](FluidGrid::set)/[`get`](FluidGrid::get) of a [`FluidCell`] and a
//! [`fill`](FluidGrid::fill) of a region. [`flow`](FluidGrid::flow) runs one real step: a
//! source drains into a hole below it, a flowing cell spreads sideways one level lower per
//! step and stops at level [`MAX_LEVEL`], a thicker fluid waits longer between spreads so lava
//! moves slower than water, and the step only ever fills an empty cell, so a body of fluid
//! settles rather than oscillating forever.
//!
//! Also real: the arithmetic. [`total_volume`](FluidGrid::total_volume) over a region,
//! [`level_at`](FluidGrid::level_at) after a step, and [`is_source`]/[`is_flowing`] of a cell.
//! A fluid can see what it [`touching`](FluidGrid::touching) touches over the six neighbours,
//! and interactions — water meeting lava — are declarations a mod registers and reads back;
//! the game decides what actually happens, so this host records the rule and never runs it.
//! Behaviour callbacks and the bucket a fluid lives in are recorded the same way, and never
//! called here.
//!
//! Refused, each with its reason from [`why_unknown`]: the client's own fluid state (`get`,
//! `get_state`, `set_state`), the height and velocity of its flowing water, the render layer
//! it draws a fluid with, and the fluids a mod's body touches in the client's world. Every one
//! answers `None`/`false`/zero after a warning, and [`refusals`] lists them.
//!
//! ```
//! use cinnaloader::fluids::{self, Fluid, FluidCell, FluidGrid};
//!
//! fluids::register(Fluid::water());
//! assert_eq!(fluids::get_density("water"), Some(1000.0));
//!
//! let mut grid = FluidGrid::new(2, 2, 1);
//! grid.set(0, 1, 0, FluidCell::source("water"));
//! assert!(grid.flow().changed, "the source drains into the hole below");
//! assert_eq!(grid.level_at(0, 0, 0), Some(1));
//! assert!(fluids::get().is_none(), "the client's fluid is out of reach");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// The level a fluid stops at: 0 is a source, 1 through 8 flow, and 8 is the last.
pub const MAX_LEVEL: u8 = 8;
/// The level a source sits at.
pub const SOURCE_LEVEL: u8 = 0;

/// One cell of a grid: which fluid, and at what level.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FluidCell {
    /// The fluid id, empty when the cell holds nothing.
    pub fluid: String,
    pub level: u8,
}

impl FluidCell {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn source(fluid: &str) -> Self {
        Self {
            fluid: fluid.to_owned(),
            level: SOURCE_LEVEL,
        }
    }

    #[must_use]
    pub fn flowing(fluid: &str, level: u8) -> Self {
        Self {
            fluid: fluid.to_owned(),
            level: level.clamp(1, MAX_LEVEL),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fluid.is_empty()
    }

    #[must_use]
    pub fn is_source(&self) -> bool {
        !self.is_empty() && self.level == SOURCE_LEVEL
    }

    #[must_use]
    pub fn is_flowing(&self) -> bool {
        !self.is_empty() && (1..=MAX_LEVEL).contains(&self.level)
    }

    /// How much of a block this cell holds: a source is full and the last level is the thinnest.
    #[must_use]
    pub fn volume(&self) -> f64 {
        if self.is_empty() {
            return 0.0;
        }
        let level = u16::from(self.level.min(MAX_LEVEL));
        let span = u16::from(MAX_LEVEL) + 1;
        f64::from(span - level) / f64::from(span)
    }
}

impl fmt::Display for FluidCell {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return formatter.write_str("empty");
        }
        if self.is_source() {
            return write!(formatter, "{} source", self.fluid);
        }
        write!(formatter, "{} level {}", self.fluid, self.level)
    }
}

/// What a fluid does when the client would tell it something happened. Recorded, never called
/// here — see the module docs.
#[derive(Clone, Default)]
pub struct FluidBehaviour {
    pub on_flow: Option<fn()>,
    pub on_tick: Option<fn()>,
    pub on_contact: Option<fn()>,
    pub on_spread: Option<fn()>,
}

impl FluidBehaviour {
    /// The callbacks that are set, by name, for a report.
    #[must_use]
    pub fn set_callbacks(&self) -> Vec<&'static str> {
        [
            ("on_flow", self.on_flow.is_some()),
            ("on_tick", self.on_tick.is_some()),
            ("on_contact", self.on_contact.is_some()),
            ("on_spread", self.on_spread.is_some()),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.then_some(name))
        .collect()
    }
}

impl fmt::Debug for FluidBehaviour {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "FluidBehaviour({:?})", self.set_callbacks())
    }
}

impl PartialEq for FluidBehaviour {
    /// Two behaviours match when they carry the same callbacks: function pointers are compared
    /// by what is set, not by where the code happens to sit.
    fn eq(&self, other: &Self) -> bool {
        self.set_callbacks() == other.set_callbacks()
    }
}

impl Eq for FluidBehaviour {}

/// A fluid type a mod declares.
#[derive(Debug, Clone, PartialEq)]
pub struct Fluid {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub density: f64,
    /// How many ticks the fluid waits between two spreads: water 1, lava 5.
    pub viscosity: f64,
    pub temperature: f64,
    pub buoyancy: f64,
    /// Whether it spreads sideways at all; a non-liquid only drains.
    pub liquid: bool,
    /// The block it places, empty when it places nothing.
    pub block: String,
    pub max_level: u8,
    pub behaviour: FluidBehaviour,
    /// Whether a mod asked this fluid to be ticked, which this host records and never does.
    pub tick_handler: bool,
    /// The bucket item that holds it, when one is declared.
    pub bucket: Option<String>,
    pub registered: bool,
}

impl Fluid {
    /// A water-like fluid with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            density: 1000.0,
            viscosity: 1.0,
            temperature: 300.0,
            buoyancy: 1.0,
            liquid: true,
            block: String::new(),
            max_level: MAX_LEVEL,
            behaviour: FluidBehaviour::default(),
            tick_handler: false,
            bucket: None,
            registered: false,
        }
    }

    /// Water, with the game's numbers.
    #[must_use]
    pub fn water() -> Self {
        Self::new("water", "Water")
            .density(1000.0)
            .viscosity(1.0)
            .temperature(300.0)
            .buoyancy(1.0)
            .block("minecraft:water")
    }

    /// Lava, which is denser, hotter and much thicker than water.
    #[must_use]
    pub fn lava() -> Self {
        Self::new("lava", "Lava")
            .density(2500.0)
            .viscosity(5.0)
            .temperature(1300.0)
            .buoyancy(-2.0)
            .block("minecraft:lava")
    }

    #[must_use]
    pub fn density(mut self, density: f64) -> Self {
        self.density = density.max(0.0);
        self
    }

    #[must_use]
    pub fn viscosity(mut self, viscosity: f64) -> Self {
        self.viscosity = viscosity.max(1.0);
        self
    }

    #[must_use]
    pub fn temperature(mut self, temperature: f64) -> Self {
        self.temperature = temperature;
        self
    }

    #[must_use]
    pub fn buoyancy(mut self, buoyancy: f64) -> Self {
        self.buoyancy = buoyancy;
        self
    }

    #[must_use]
    pub fn liquid(mut self, liquid: bool) -> Self {
        self.liquid = liquid;
        self
    }

    #[must_use]
    pub fn block(mut self, block: &str) -> Self {
        self.block = block.to_owned();
        self
    }

    #[must_use]
    pub fn max_level(mut self, max_level: u8) -> Self {
        self.max_level = max_level.clamp(1, MAX_LEVEL);
        self
    }

    #[must_use]
    pub fn behaviour(mut self, behaviour: FluidBehaviour) -> Self {
        self.behaviour = behaviour;
        self
    }

    /// How many ticks pass between two spreads: a thicker fluid waits longer.
    #[must_use]
    pub fn spread_interval(&self) -> u64 {
        self.viscosity.max(1.0).round() as u64
    }
}

impl fmt::Display for Fluid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, density {}, viscosity {})",
            self.name, self.id, self.density, self.viscosity
        )
    }
}

/// What one flow step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowReport {
    pub tick: u64,
    /// Whether any cell changed; false is a fixed point.
    pub changed: bool,
    /// How many cells filled this step.
    pub cells: usize,
}

/// A finite grid of fluid a mod owns.
#[derive(Debug, Clone, PartialEq)]
pub struct FluidGrid {
    pub width: usize,
    pub height: usize,
    pub depth: usize,
    pub cells: Vec<FluidCell>,
    pub tick: u64,
}

impl FluidGrid {
    /// A grid of empty cells.
    #[must_use]
    pub fn new(width: usize, height: usize, depth: usize) -> Self {
        Self {
            width,
            height,
            depth,
            cells: vec![FluidCell::empty(); width * height * depth],
            tick: 0,
        }
    }

    fn index(&self, x: usize, y: usize, z: usize) -> Option<usize> {
        (x < self.width && y < self.height && z < self.depth)
            .then(|| x + self.width * (z + self.depth * y))
    }

    fn coords(&self, index: usize) -> (usize, usize, usize) {
        // the layout is x + width * (z + depth * y), so y divides and z is the remainder
        let x = index % self.width;
        let rest = index / self.width;
        (x, rest / self.depth, rest % self.depth)
    }

    /// Sets one cell, refusing a position outside the grid.
    pub fn set(&mut self, x: usize, y: usize, z: usize, cell: FluidCell) -> bool {
        match self.index(x, y, z) {
            Some(index) => {
                self.cells[index] = cell;
                true
            }
            None => false,
        }
    }

    /// One cell, or `None` when the position is outside the grid.
    #[must_use]
    pub fn get(&self, x: usize, y: usize, z: usize) -> Option<FluidCell> {
        self.index(x, y, z).map(|index| self.cells[index].clone())
    }

    /// Fills the `(width, height, depth)` box after a corner and answers how many cells it set.
    #[must_use]
    pub fn fill(
        &mut self,
        x: usize,
        y: usize,
        z: usize,
        size: (usize, usize, usize),
        cell: FluidCell,
    ) -> usize {
        let (width, height, depth) = size;
        let mut filled = 0;
        for dy in 0..height {
            for dz in 0..depth {
                for dx in 0..width {
                    if self.set(x + dx, y + dy, z + dz, cell.clone()) {
                        filled += 1;
                    }
                }
            }
        }
        filled
    }

    /// The level of the fluid at a position, or `None` when it is empty or outside.
    #[must_use]
    pub fn level_at(&self, x: usize, y: usize, z: usize) -> Option<u8> {
        self.get(x, y, z).filter(|cell| !cell.is_empty()).map(|cell| cell.level)
    }

    /// The fluid id at a position.
    #[must_use]
    pub fn fluid_at(&self, x: usize, y: usize, z: usize) -> Option<String> {
        self.get(x, y, z).filter(|cell| !cell.is_empty()).map(|cell| cell.fluid)
    }

    /// Whether the cell is a permanent source.
    #[must_use]
    pub fn is_source_at(&self, x: usize, y: usize, z: usize) -> bool {
        self.get(x, y, z).is_some_and(|cell| cell.is_source())
    }

    /// Whether the cell is a flowing cell.
    #[must_use]
    pub fn is_flowing_at(&self, x: usize, y: usize, z: usize) -> bool {
        self.get(x, y, z).is_some_and(|cell| cell.is_flowing())
    }

    /// The fluid volume of every non-empty cell added up.
    #[must_use]
    pub fn total_volume(&self) -> f64 {
        // std seeds a float sum with -0.0, which is a poor answer to show for an empty grid
        let total: f64 = self.cells.iter().map(FluidCell::volume).sum();
        if total == 0.0 {
            0.0
        } else {
            total
        }
    }

    /// Where the six neighbours that hold a fluid are.
    #[must_use]
    pub fn touching(&self, x: usize, y: usize, z: usize) -> Vec<(usize, usize, usize)> {
        let mut found = Vec::new();
        for (dx, dy, dz) in [(1i64, 0i64, 0i64), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
            let nx = x as i64 + dx;
            let ny = y as i64 + dy;
            let nz = z as i64 + dz;
            if nx < 0 || ny < 0 || nz < 0 {
                continue;
            }
            let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
            if self.index(nx, ny, nz).is_some_and(|index| !self.cells[index].is_empty()) {
                found.push((nx, ny, nz));
            }
        }
        found
    }

    /// One flow step, from a snapshot so it never sees its own output.
    ///
    /// A source drains into an empty cell below; any cell spreads sideways one level lower,
    /// stopping at its fluid's last level. A fluid only acts on ticks its viscosity allows.
    pub fn flow(&mut self) -> FlowReport {
        self.tick += 1;
        let now = self.tick;
        let mut next = self.cells.clone();
        // a flow step only ever fills empty cells, so a body of fluid can never oscillate
        for (index, cell) in self.cells.iter().enumerate() {
            if cell.is_empty() {
                continue;
            }
            let fluid = lookup(&cell.fluid);
            if !now.is_multiple_of(fluid.spread_interval()) {
                continue;
            }
            let (x, y, z) = self.coords(index);
            if y > 0 {
                if let Some(below) = self.index(x, y - 1, z) {
                    if next[below].is_empty() {
                        let level = if cell.is_source() { 1 } else { cell.level };
                        next[below] = FluidCell::flowing(&cell.fluid, level);
                    }
                }
            }
            if fluid.liquid && cell.level < fluid.max_level {
                let level = (cell.level + 1).min(fluid.max_level);
                for (nx, nz) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let nx = x as i64 + nx;
                    let nz = z as i64 + nz;
                    if nx < 0 || nz < 0 {
                        continue;
                    }
                    if let Some(side) = self.index(nx as usize, y, nz as usize) {
                        if next[side].is_empty() {
                            next[side] = FluidCell::flowing(&cell.fluid, level);
                        }
                    }
                }
            }
        }
        let changed = next != self.cells;
        let cells = self
            .cells
            .iter()
            .zip(&next)
            .filter(|(before, after)| before != after)
            .count();
        self.cells = next;
        FlowReport {
            tick: now,
            changed,
            cells,
        }
    }

    /// Runs flow steps until one changes nothing, or `max_steps`, and answers how many ran.
    #[must_use]
    pub fn run_until_stable(&mut self, max_steps: usize) -> usize {
        for step in 1..=max_steps {
            if !self.flow().changed {
                return step;
            }
        }
        max_steps
    }
}

impl fmt::Display for FluidGrid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "fluid grid {}x{}x{}, {} cells, volume {}",
            self.width,
            self.height,
            self.depth,
            self.cells.iter().filter(|cell| !cell.is_empty()).count(),
            self.total_volume()
        )
    }
}

/// The rule the game applies when one fluid meets another. This host records it; the game runs
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum InteractionRule {
    #[default]
    Stone,
    Obsidian,
    Extinguish,
    Custom,
}

impl InteractionRule {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stone => "stone",
            Self::Obsidian => "obsidian",
            Self::Extinguish => "extinguish",
            Self::Custom => "custom",
        }
    }
}

/// A declared meeting of two fluids, and what it leaves behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluidInteraction {
    pub a: String,
    pub b: String,
    /// The block left where they meet, empty when they only cancel.
    pub result: String,
    pub rule: InteractionRule,
}

impl FluidInteraction {
    #[must_use]
    pub fn new(a: &str, b: &str, result: &str) -> Self {
        Self {
            a: a.to_owned(),
            b: b.to_owned(),
            result: result.to_owned(),
            rule: InteractionRule::Stone,
        }
    }

    #[must_use]
    pub fn rule(mut self, rule: InteractionRule) -> Self {
        self.rule = rule;
        self
    }

    /// The pair this interaction is keyed by, whichever order it was declared in.
    #[must_use]
    pub fn key(&self) -> (String, String) {
        pair(&self.a, &self.b)
    }
}

impl fmt::Display for FluidInteraction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} meets {} -> {} ({})",
            self.a,
            self.b,
            self.result,
            self.rule.as_str()
        )
    }
}

fn pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_owned(), b.to_owned())
    } else {
        (b.to_owned(), a.to_owned())
    }
}

fn bare(id: &str) -> &str {
    id.rsplit_once(':').map_or(id, |(_, tail)| tail)
}

fn fluids() -> &'static Mutex<BTreeMap<String, Fluid>> {
    static FLUIDS: OnceLock<Mutex<BTreeMap<String, Fluid>>> = OnceLock::new();
    FLUIDS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_fluids<T>(body: impl FnOnce(&mut BTreeMap<String, Fluid>) -> T) -> T {
    let mut guard = fluids().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn interactions() -> &'static Mutex<BTreeMap<(String, String), FluidInteraction>> {
    static INTERACTIONS: OnceLock<Mutex<BTreeMap<(String, String), FluidInteraction>>> =
        OnceLock::new();
    INTERACTIONS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_interactions<T>(
    body: impl FnOnce(&mut BTreeMap<(String, String), FluidInteraction>) -> T,
) -> T {
    let mut guard = interactions().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn lookup(id: &str) -> Fluid {
    get_fluid(id).unwrap_or_else(|| Fluid::new(id, id))
}

/// Declares a fluid type, or replaces the one with that id.
pub fn register(fluid: Fluid) -> bool {
    with_fluids(|fluids| fluids.insert(fluid.id.clone(), fluid).is_none())
}

/// Forgets a declared fluid type.
pub fn unregister(id: &str) -> bool {
    with_fluids(|fluids| fluids.remove(id).is_some())
}

/// Registers water and lava, answering how many were new.
pub fn register_defaults() -> usize {
    [Fluid::water(), Fluid::lava()]
        .into_iter()
        .filter(|fluid| register(fluid.clone()))
        .count()
}

/// A declared fluid type, by id.
#[must_use]
pub fn get_fluid(id: &str) -> Option<Fluid> {
    with_fluids(|fluids| fluids.get(id).cloned())
}

/// The same, by the name a mod gave it.
#[must_use]
pub fn get_by_name(name: &str) -> Option<Fluid> {
    with_fluids(|fluids| fluids.values().find(|fluid| fluid.name == name).cloned())
}

/// Every declared fluid type, in id order.
#[must_use]
pub fn get_all() -> Vec<Fluid> {
    with_fluids(|fluids| fluids.values().cloned().collect())
}

/// A declared fluid's density.
#[must_use]
pub fn get_density(id: &str) -> Option<f64> {
    get_fluid(id).map(|fluid| fluid.density)
}

/// A declared fluid's viscosity, in ticks between spreads.
#[must_use]
pub fn get_viscosity(id: &str) -> Option<f64> {
    get_fluid(id).map(|fluid| fluid.viscosity)
}

/// A declared fluid's temperature.
#[must_use]
pub fn get_temperature(id: &str) -> Option<f64> {
    get_fluid(id).map(|fluid| fluid.temperature)
}

/// A declared fluid's buoyancy.
#[must_use]
pub fn get_buoyancy(id: &str) -> Option<f64> {
    get_fluid(id).map(|fluid| fluid.buoyancy)
}

/// The block a declared fluid places.
#[must_use]
pub fn get_block(id: &str) -> Option<String> {
    get_fluid(id).map(|fluid| fluid.block)
}

/// The last level a declared fluid spreads at.
#[must_use]
pub fn get_max_level(id: &str) -> Option<u8> {
    get_fluid(id).map(|fluid| fluid.max_level)
}

/// Whether a declared fluid is a liquid that spreads sideways.
#[must_use]
pub fn is_liquid(id: &str) -> Option<bool> {
    get_fluid(id).map(|fluid| fluid.liquid)
}

/// Records the callbacks a fluid would run when the client fires one. This host never calls
/// them.
pub fn register_behavior(id: &str, behaviour: FluidBehaviour) -> bool {
    with_fluids(|fluids| match fluids.get_mut(id) {
        Some(fluid) => {
            fluid.behaviour = behaviour;
            true
        }
        None => false,
    })
}

/// The behaviour a mod put on a declared fluid, which this host never runs.
#[must_use]
pub fn get_behaviour(id: &str) -> Option<FluidBehaviour> {
    get_fluid(id).map(|fluid| fluid.behaviour)
}

/// Which callbacks are set on a declared fluid, for a report.
#[must_use]
pub fn behaviour_callbacks(id: &str) -> Vec<&'static str> {
    get_behaviour(id)
        .map(|behaviour| behaviour.set_callbacks())
        .unwrap_or_default()
}

/// Records that a mod wants this fluid ticked. This host records it and never ticks it.
pub fn register_tick_handler(id: &str) -> bool {
    with_fluids(|fluids| match fluids.get_mut(id) {
        Some(fluid) => {
            fluid.tick_handler = true;
            true
        }
        None => false,
    })
}

/// Whether a mod asked for this fluid to be ticked.
#[must_use]
pub fn has_tick_handler(id: &str) -> bool {
    get_fluid(id).is_some_and(|fluid| fluid.tick_handler)
}

/// Declares the bucket item that holds a fluid.
pub fn register_bucket(id: &str, item: &str) -> bool {
    with_fluids(|fluids| match fluids.get_mut(id) {
        Some(fluid) => {
            fluid.bucket = Some(item.to_owned());
            true
        }
        None => false,
    })
}

/// The bucket item a declared fluid lives in.
#[must_use]
pub fn get_bucket(id: &str) -> Option<String> {
    get_fluid(id).and_then(|fluid| fluid.bucket)
}

/// Changes how many ticks a declared fluid waits between spreads.
pub fn modify_flow(id: &str, viscosity: f64) -> bool {
    with_fluids(|fluids| match fluids.get_mut(id) {
        Some(fluid) => {
            fluid.viscosity = viscosity.max(1.0);
            true
        }
        None => false,
    })
}

/// Changes the last level a declared fluid spreads at.
pub fn modify_spread(id: &str, max_level: u8) -> bool {
    with_fluids(|fluids| match fluids.get_mut(id) {
        Some(fluid) => {
            fluid.max_level = max_level.clamp(1, MAX_LEVEL);
            true
        }
        None => false,
    })
}

/// Declares what two fluids leave behind where they meet, keyed whichever way round they are
/// given.
pub fn register_interaction(a: &str, b: &str, result: &str) -> bool {
    let interaction = FluidInteraction::new(a, b, result);
    with_interactions(|interactions| interactions.insert(interaction.key(), interaction).is_none())
}

/// Forgets a declared interaction.
pub fn unregister_interaction(a: &str, b: &str) -> bool {
    with_interactions(|interactions| interactions.remove(&pair(a, b)).is_some())
}

/// The rule for two fluids meeting: a mod's declaration, or the game's own for water and lava.
#[must_use]
pub fn get_interaction(a: &str, b: &str) -> Option<FluidInteraction> {
    let key = pair(a, b);
    with_interactions(|interactions| interactions.get(&key).cloned())
        .or_else(|| default_interaction(a, b))
}

/// The rule the game itself applies when water meets lava. This host only reports it.
#[must_use]
pub fn default_interaction(a: &str, b: &str) -> Option<FluidInteraction> {
    let (a, b) = (bare(a), bare(b));
    let water_lava = (a == "water" && b == "lava") || (a == "lava" && b == "water");
    water_lava.then(|| FluidInteraction::new("water", "lava", "minecraft:stone"))
}

/// Every interaction a fluid at a position declares with a different fluid it touches.
#[must_use]
pub fn interactions_at(grid: &FluidGrid, x: usize, y: usize, z: usize) -> Vec<FluidInteraction> {
    let Some(cell) = grid.get(x, y, z).filter(|cell| !cell.is_empty()) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (nx, ny, nz) in grid.touching(x, y, z) {
        if let Some(other) = grid.get(nx, ny, nz) {
            if other.fluid != cell.fluid {
                if let Some(interaction) = get_interaction(&cell.fluid, &other.fluid) {
                    if !found.contains(&interaction) {
                        found.push(interaction);
                    }
                }
            }
        }
    }
    found
}

/// Whether a cell is a permanent source.
#[must_use]
pub fn is_source(cell: &FluidCell) -> bool {
    cell.is_source()
}

/// Whether a cell is flowing.
#[must_use]
pub fn is_flowing(cell: &FluidCell) -> bool {
    cell.is_flowing()
}

/// Whether a cell holds nothing.
#[must_use]
pub fn is_empty(cell: &FluidCell) -> bool {
    cell.is_empty()
}

/// Whether a fluid may flow into a cell: an empty one, or one that already holds it.
#[must_use]
pub fn can_flow_into(grid: &FluidGrid, x: usize, y: usize, z: usize, fluid: &str) -> bool {
    match grid.get(x, y, z) {
        None => false,
        Some(cell) => cell.is_empty() || cell.fluid == fluid,
    }
}

/// Whether the fluid in a cell can be pushed aside: a flowing cell can, a source cannot.
#[must_use]
pub fn can_displace(grid: &FluidGrid, x: usize, y: usize, z: usize) -> bool {
    grid.get(x, y, z)
        .is_some_and(|cell| !cell.is_empty() && !cell.is_source())
}

/// Why a fluid question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "state" => {
            "the fluid at a position is the client's world state; reading or writing it needs \
             the hooks section 10 lists as never"
                .to_owned()
        }
        "flow" => {
            "how the client's water flows, its height and its velocity, is simulated inside the \
             client and never crosses the boundary"
                .to_owned()
        }
        "render" => "the layer a fluid is drawn with is chosen by the client's pipeline".to_owned(),
        "world" => {
            "the fluids a body touches in the client's world are the client's to know".to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about fluids.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["state", "flow", "render", "world"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own fluid, which this host cannot read.
pub mod known_only_to_the_client {
    use super::{why_unknown, FluidCell};

    /// The fluid at a position.
    #[must_use]
    pub fn get() -> Option<FluidCell> {
        crate::warn!("a mod asked this host for the client's fluid: {}", why_unknown("state"));
        None
    }

    /// The client's fluid state at a position.
    #[must_use]
    pub fn get_state(_x: i32, _y: i32, _z: i32) -> Option<FluidCell> {
        crate::warn!(
            "a mod asked this host for the client's fluid state: {}",
            why_unknown("state")
        );
        None
    }

    /// Sets the client's fluid state at a position.
    pub fn set_state(_x: i32, _y: i32, _z: i32, _fluid: &str, _level: u8) -> bool {
        crate::warn!(
            "a mod asked this host to set the client's fluid state: {}",
            why_unknown("state")
        );
        false
    }

    /// The height of the client's flowing water at a position.
    #[must_use]
    pub fn get_height(_x: i32, _y: i32, _z: i32) -> f64 {
        crate::warn!(
            "a mod asked this host for the height of the client's water: {}",
            why_unknown("flow")
        );
        0.0
    }

    /// The velocity of the client's flowing water at a position.
    #[must_use]
    pub fn get_velocity(_x: i32, _y: i32, _z: i32) -> (f64, f64, f64) {
        crate::warn!(
            "a mod asked this host for the velocity of the client's water: {}",
            why_unknown("flow")
        );
        (0.0, 0.0, 0.0)
    }

    /// The fluid neighbours of a position in the client's world.
    #[must_use]
    pub fn get_neighbors(_x: i32, _y: i32, _z: i32) -> Vec<(i32, i32, i32)> {
        crate::warn!(
            "a mod asked this host which fluids touch a position in the client's world: {}",
            why_unknown("world")
        );
        Vec::new()
    }

    /// Sets the layer the client draws a fluid with.
    pub fn register_render_layer(_fluid: &str, _layer: &str) -> bool {
        crate::warn!(
            "a mod asked this host to set a fluid's render layer: {}",
            why_unknown("render")
        );
        false
    }
}

pub use known_only_to_the_client::{
    get, get_height, get_neighbors, get_state, get_velocity, register_render_layer, set_state,
};

#[test]
fn a_cell_index_round_trips_on_every_grid_shape() {
    let _serial = crate::test_lock::serial();
    for shape in [(1, 2, 1), (1, 3, 1), (2, 3, 4), (3, 3, 3), (12, 1, 1)] {
        let grid = FluidGrid::new(shape.0, shape.1, shape.2);
        for y in 0..shape.1 {
            for z in 0..shape.2 {
                for x in 0..shape.0 {
                    let index = grid
                        .index(x, y, z)
                        .unwrap_or_else(|| panic!("{shape:?} has no index for {x},{y},{z}"));
                    assert_eq!(
                        grid.coords(index),
                        (x, y, z),
                        "{shape:?} index/coords disagree at {x},{y},{z}"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_fluids(|fluids| fluids.clear());
        with_interactions(|interactions| interactions.clear());
    }

    #[test]
    fn a_source_over_a_hole_drains_downward() {
        let _serial = serial();
        register_defaults();
        let mut grid = FluidGrid::new(1, 2, 1);
        assert!(grid.set(0, 1, 0, FluidCell::source("water")));
        let report = grid.flow();
        assert!(report.changed);
        assert_eq!(report.cells, 1);
        assert_eq!(grid.level_at(0, 0, 0), Some(1), "the hole takes the source's water");
        assert_eq!(grid.fluid_at(0, 0, 0).as_deref(), Some("water"));
        assert!(grid.is_source_at(0, 1, 0), "a source does not empty");
    }

    #[test]
    fn a_level_drops_by_one_a_step_and_stops_at_eight() {
        let _serial = serial();
        register_defaults();
        let mut grid = FluidGrid::new(12, 1, 1);
        grid.set(0, 0, 0, FluidCell::source("water"));
        for step in 1..=8u8 {
            assert!(grid.flow().changed, "step {step} moved");
            assert_eq!(
                grid.level_at(usize::from(step), 0, 0),
                Some(step),
                "one cell further out, one level lower"
            );
        }
        for _ in 0..8 {
            grid.flow();
        }
        assert_eq!(grid.level_at(8, 0, 0), Some(MAX_LEVEL));
        assert_eq!(grid.level_at(9, 0, 0), None, "level 8 does not spread further");
        for x in 0..12 {
            if let Some(level) = grid.level_at(x, 0, 0) {
                assert!(level <= MAX_LEVEL, "{level} exceeds the last level");
            }
        }
    }

    #[test]
    fn a_region_total_volume_adds_up() {
        let mut grid = FluidGrid::new(2, 2, 1);
        assert_eq!(grid.total_volume(), 0.0);
        assert!(
            grid.total_volume().is_sign_positive(),
            "an empty grid must not answer with a negative zero"
        );
        grid.set(0, 0, 0, FluidCell::source("water"));
        grid.set(1, 0, 0, FluidCell::flowing("water", 1));
        assert_eq!(grid.total_volume(), 1.0 + 8.0 / 9.0, "a source and a level-1 cell");
        assert_eq!(
            grid.fill(0, 1, 0, (2, 1, 1), FluidCell::source("water")),
            2,
            "two more full cells"
        );
        assert_eq!(grid.total_volume(), 3.0 + 8.0 / 9.0);
    }

    #[test]
    fn a_flow_reaches_a_fixed_point() {
        let _serial = serial();
        register_defaults();
        let mut grid = FluidGrid::new(5, 4, 5);
        grid.set(2, 3, 2, FluidCell::source("water"));
        let steps = grid.run_until_stable(500);
        assert!(steps < 500, "a settled grid is reached in {steps} steps, not forever");
        let before = grid.cells.clone();
        let report = grid.flow();
        assert!(!report.changed, "a settled grid does not change again");
        assert_eq!(report.cells, 0);
        assert_eq!(grid.cells, before, "only the tick moved, not a cell");
        assert!(grid.total_volume() > 0.0);
    }

    #[test]
    fn lava_spreads_slower_than_water() {
        let _serial = serial();
        register_defaults();
        assert_eq!(get_viscosity("water"), Some(1.0));
        assert_eq!(get_viscosity("lava"), Some(5.0));
        let mut water = FluidGrid::new(10, 1, 1);
        let mut lava = FluidGrid::new(10, 1, 1);
        water.set(0, 0, 0, FluidCell::source("water"));
        lava.set(0, 0, 0, FluidCell::source("lava"));

        water.flow();
        lava.flow();
        assert_eq!(water.level_at(1, 0, 0), Some(1), "water moves on the first tick");
        assert_eq!(lava.level_at(1, 0, 0), None, "lava waits five ticks");

        for _ in 0..4 {
            lava.flow();
        }
        assert_eq!(lava.level_at(1, 0, 0), Some(1), "lava moves on its fifth tick");
    }

    #[test]
    fn a_declared_fluid_can_be_read_back() {
        let _serial = serial();
        fresh();
        assert!(register(Fluid::water()));
        assert!(!register(Fluid::water()), "the second registration replaces the first");
        assert_eq!(get_fluid("water").map(|fluid| fluid.name), Some("Water".to_owned()));
        assert_eq!(get_by_name("Water").map(|fluid| fluid.id), Some("water".to_owned()));
        assert_eq!(get_density("water"), Some(1000.0));
        assert_eq!(get_viscosity("water"), Some(1.0));
        assert_eq!(get_temperature("water"), Some(300.0));
        assert_eq!(get_buoyancy("water"), Some(1.0));
        assert_eq!(get_block("water"), Some("minecraft:water".to_owned()));
        assert_eq!(get_max_level("water"), Some(MAX_LEVEL));
        assert_eq!(is_liquid("water"), Some(true));
        assert_eq!(get_all().len(), 1);
        assert!(unregister("water"));
        assert!(get_fluid("water").is_none());
    }

    #[test]
    fn interactions_buckets_and_handlers_are_declared() {
        let _serial = serial();
        fresh();
        assert!(register(Fluid::water()));
        assert!(register(Fluid::lava()));
        assert!(register_bucket("water", "minecraft:water_bucket"));
        assert_eq!(get_bucket("water"), Some("minecraft:water_bucket".to_owned()));
        assert!(register_tick_handler("water"));
        assert!(has_tick_handler("water"));
        assert!(!register_tick_handler("nothing"));

        let behaviour = FluidBehaviour {
            on_flow: Some(|| {}),
            ..FluidBehaviour::default()
        };
        assert!(register_behavior("water", behaviour));
        assert_eq!(behaviour_callbacks("water"), vec!["on_flow"]);
        assert!(!register_behavior("nothing", FluidBehaviour::default()));

        assert!(register_interaction("acid", "lava", "minecraft:basalt"));
        let interaction = get_interaction("lava", "acid").expect("order does not matter");
        assert_eq!(interaction.result, "minecraft:basalt");
        assert!(unregister_interaction("acid", "lava"));
        assert!(get_interaction("acid", "lava").is_none());

        let default = default_interaction("minecraft:water", "lava").expect("the game's own rule");
        assert_eq!(default.result, "minecraft:stone");
        assert!(default_interaction("water", "acid").is_none());

        assert!(modify_flow("water", 4.0));
        assert_eq!(get_viscosity("water"), Some(4.0));
        assert!(modify_spread("water", 4));
        assert_eq!(get_max_level("water"), Some(4));
        assert!(!modify_flow("nothing", 2.0));
    }

    #[test]
    fn a_fluid_sees_what_it_touches() {
        let _serial = serial();
        fresh();
        register(Fluid::water());
        register(Fluid::lava());
        register_interaction("water", "lava", "minecraft:stone");

        let mut grid = FluidGrid::new(3, 2, 1);
        grid.set(1, 0, 0, FluidCell::source("water"));
        grid.set(2, 0, 0, FluidCell::source("lava"));
        grid.set(0, 1, 0, FluidCell::flowing("water", 3));

        assert_eq!(grid.touching(1, 0, 0), vec![(2, 0, 0)], "the lava to its right");
        assert!(can_flow_into(&grid, 0, 0, 0, "water"), "an empty cell takes water");
        assert!(!can_flow_into(&grid, 2, 0, 0, "water"), "a different fluid is in the way");
        assert!(can_flow_into(&grid, 1, 0, 0, "water"), "the same fluid is part of the body");

        assert!(!can_displace(&grid, 0, 0, 0), "an empty cell has nothing to displace");
        assert!(!can_displace(&grid, 1, 0, 0), "a source is permanent");
        assert!(can_displace(&grid, 0, 1, 0), "a flowing cell can be pushed aside");

        let meetings = interactions_at(&grid, 1, 0, 0);
        assert_eq!(meetings.len(), 1);
        assert_eq!(meetings[0].result, "minecraft:stone");

        assert!(is_source(&grid.get(1, 0, 0).expect("it is there")));
        assert!(is_flowing(&grid.get(0, 1, 0).expect("it is there")));
        assert!(is_empty(&grid.get(0, 0, 0).expect("it is there")));
    }

    #[test]
    fn every_fluid_question_about_the_client_is_refused_with_a_reason() {
        let _serial = serial();
        assert!(known_only_to_the_client::get().is_none());
        assert!(known_only_to_the_client::get_state(0, 64, 0).is_none());
        assert!(!known_only_to_the_client::set_state(0, 64, 0, "water", 0));
        assert_eq!(known_only_to_the_client::get_height(0, 64, 0), 0.0);
        assert_eq!(
            known_only_to_the_client::get_velocity(0, 64, 0),
            (0.0, 0.0, 0.0)
        );
        assert!(known_only_to_the_client::get_neighbors(0, 64, 0).is_empty());
        assert!(!known_only_to_the_client::register_render_layer("water", "translucent"));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
