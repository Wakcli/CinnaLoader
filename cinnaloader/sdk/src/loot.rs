//! Loot tables: the ones a mod ships, and a seeded generator that really rolls them.
//!
//! A table is data a mod registers — pools, entries, weights, count ranges, conditions and
//! functions — and this host rolls it for real. Every roll runs off a small splitmix64
//! generator this host owns, so the **same seed gives the same drops** and a different seed
//! does not; a mod can reproduce a drop a player saw, and a test can pin one down.
//!
//! An entry is selected by a **real weighted draw** over the pool's eligible entries, an
//! entry's conditions are evaluated before it can be drawn, and a failed condition excludes
//! the entry rather than returning an empty stack. Counts come from a number provider — a
//! fixed number or a min/max range — and land inside the range they declare.
//!
//! What is refused is the client's half: the client's own loot tables from its data pack, the
//! server's resolution of the table that actually dropped an item, the client's block and
//! entity drop rules, and anything that would roll against a live entity, block or fishing
//! hook, because all of that needs the client's world. Those answer `None` or empty with the
//! reason in [`why_unknown`], and the refusing calls log a warning.
//!
//! ```
//! use cinnaloader::prelude::*;
//! use cinnaloader::loot;
//!
//! loot::register_table(
//!     loot::Table::new("hello:grass", loot::TableKind::Block).pool(
//!         loot::Pool::new(loot::NumberProvider::fixed(1))
//!             .entry(
//!                 loot::Entry::item("minecraft:short_grass", loot::NumberProvider::fixed(1))
//!                     .weight(3),
//!             )
//!             .entry(
//!                 loot::Entry::item("minecraft:wheat_seeds", loot::NumberProvider::uniform(1, 3))
//!                     .weight(1),
//!             ),
//!     ),
//! );
//! let first = loot::generate("hello:grass", 2026);
//! let again = loot::generate("hello:grass", 2026);
//! assert_eq!(first, again, "the same seed gives the same drops");
//! assert!(!first.is_empty());
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::inventory::Inventory;
use crate::items::ItemStack;

/// The seed a generator uses when a caller has not set one.
pub const DEFAULT_SEED: u64 = 0;

/// The table the game's own fishing uses, which is the id a mod registers one under.
pub const FISHING_TABLE: &str = "minecraft:gameplay/fishing";

/// A seeded generator a mod owns. Small, deterministic and reproducible, which is what a
/// loot roll needs; the client's own random is not reachable from here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootRandom {
    state: u64,
}

impl LootRandom {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        // splitmix64
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// The next 32 bits.
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// The next number between zero and one.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// The next number in an inclusive range.
    pub fn next_range(&mut self, low: u32, high: u32) -> u32 {
        if high <= low {
            return low;
        }
        low + (self.next_f64() * f64::from(high - low + 1)) as u32
    }

    /// Whether the next draw is under a chance.
    pub fn chance(&mut self, probability: f64) -> bool {
        self.next_f64() < probability
    }

    /// Picks one item by weight, skipping anything with no weight, or `None` when nothing
    /// has any.
    pub fn weighted<'a, T>(&mut self, items: &'a [T], weight: impl Fn(&T) -> f64) -> Option<&'a T> {
        let mut total = 0.0;
        for item in items {
            let held = weight(item);
            if held > 0.0 {
                total += held;
            }
        }
        if total <= 0.0 {
            return None;
        }
        let mut point = self.next_f64() * total;
        for item in items {
            let held = weight(item);
            if held <= 0.0 {
                continue;
            }
            point -= held;
            if point < 0.0 {
                return Some(item);
            }
        }
        items.iter().rev().find(|item| weight(*item) > 0.0)
    }
}

/// What a table is for, which is what the game files a table under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TableKind {
    Generic,
    Block,
    Entity,
    Container,
    Fishing,
}

impl TableKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Block => "block",
            Self::Entity => "entity",
            Self::Container => "container",
            Self::Fishing => "fishing",
        }
    }
}

/// How many an entry drops: one number, or a range the generator draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberProvider {
    Fixed(u32),
    Uniform { min: u32, max: u32 },
}

impl NumberProvider {
    #[must_use]
    pub fn fixed(count: u32) -> Self {
        Self::Fixed(count)
    }

    #[must_use]
    pub fn uniform(min: u32, max: u32) -> Self {
        if min <= max {
            Self::Uniform { min, max }
        } else {
            Self::Uniform { min: max, max: min }
        }
    }

    /// The smallest count it can give.
    #[must_use]
    pub fn min(self) -> u32 {
        match self {
            Self::Fixed(count) | Self::Uniform { min: count, .. } => count,
        }
    }

    /// The largest.
    #[must_use]
    pub fn max(self) -> u32 {
        match self {
            Self::Fixed(count) => count,
            Self::Uniform { max, .. } => max,
        }
    }

    /// Draws a count.
    #[must_use]
    pub fn roll(self, rng: &mut LootRandom) -> u32 {
        match self {
            Self::Fixed(count) => count,
            Self::Uniform { min, max } => rng.next_range(min, max),
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Fixed(_) => "constant",
            Self::Uniform { .. } => "uniform",
        }
    }
}

/// What a condition tests, over the context the caller supplies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionKind {
    Chance,
    Level,
    Luck,
    Flag,
    Tool,
    EntityType,
}

/// A predicate that gates a pool or an entry. A failed one excludes the entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub kind: ConditionKind,
    pub value: f64,
    pub flag: bool,
    pub item: String,
}

impl Condition {
    /// Passes with a probability.
    #[must_use]
    pub fn chance(probability: f64) -> Self {
        Self {
            kind: ConditionKind::Chance,
            value: probability.clamp(0.0, 1.0),
            flag: false,
            item: String::new(),
        }
    }

    /// Passes when the context's level is at least this.
    #[must_use]
    pub fn level(at_least: u32) -> Self {
        Self {
            kind: ConditionKind::Level,
            value: f64::from(at_least),
            flag: false,
            item: String::new(),
        }
    }

    /// Passes when the context's luck is at least this.
    #[must_use]
    pub fn luck(at_least: f64) -> Self {
        Self {
            kind: ConditionKind::Luck,
            value: at_least,
            flag: false,
            item: String::new(),
        }
    }

    /// Passes when the context's flag is what this expects.
    #[must_use]
    pub fn flag(expected: bool) -> Self {
        Self {
            kind: ConditionKind::Flag,
            value: 0.0,
            flag: expected,
            item: String::new(),
        }
    }

    /// Passes when the context's tool is this item.
    #[must_use]
    pub fn tool(item: &str) -> Self {
        Self {
            kind: ConditionKind::Tool,
            value: 0.0,
            flag: false,
            item: item.to_owned(),
        }
    }

    /// Passes when the context's entity is this type.
    #[must_use]
    pub fn entity_type(entity: &str) -> Self {
        Self {
            kind: ConditionKind::EntityType,
            value: 0.0,
            flag: false,
            item: entity.to_owned(),
        }
    }

    /// Whether it passes for a context. A chance draw consumes the generator.
    #[must_use]
    pub fn evaluate(&self, context: &LootContext, rng: &mut LootRandom) -> bool {
        match self.kind {
            ConditionKind::Chance => rng.chance(self.value),
            ConditionKind::Level => f64::from(context.level) >= self.value,
            ConditionKind::Luck => context.luck >= self.value,
            ConditionKind::Flag => context.flag == self.flag,
            ConditionKind::Tool => context.tool.as_deref() == Some(self.item.as_str()),
            ConditionKind::EntityType => context.entity == self.item,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.kind {
            ConditionKind::Chance => "chance",
            ConditionKind::Level => "level",
            ConditionKind::Luck => "luck",
            ConditionKind::Flag => "flag",
            ConditionKind::Tool => "tool",
            ConditionKind::EntityType => "entity_type",
        }
    }
}

/// What a function does to a drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionKind {
    SetCount,
    AddCount,
    MultiplyCount,
    SetName,
    Enchant,
}

/// A modification applied to a stack after it is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub kind: FunctionKind,
    pub value: f64,
    pub name: String,
}

impl Function {
    #[must_use]
    pub fn set_count(count: u32) -> Self {
        Self {
            kind: FunctionKind::SetCount,
            value: f64::from(count),
            name: String::new(),
        }
    }

    #[must_use]
    pub fn add_count(count: i64) -> Self {
        Self {
            kind: FunctionKind::AddCount,
            value: count as f64,
            name: String::new(),
        }
    }

    #[must_use]
    pub fn multiply_count(factor: f64) -> Self {
        Self {
            kind: FunctionKind::MultiplyCount,
            value: factor,
            name: String::new(),
        }
    }

    #[must_use]
    pub fn set_name(name: &str) -> Self {
        Self {
            kind: FunctionKind::SetName,
            value: 0.0,
            name: name.to_owned(),
        }
    }

    #[must_use]
    pub fn enchant(name: &str) -> Self {
        Self {
            kind: FunctionKind::Enchant,
            value: 0.0,
            name: name.to_owned(),
        }
    }

    /// Applies it, and answers whether it changed the stack.
    pub fn apply(&self, stack: &mut ItemStack, rng: &mut LootRandom) -> bool {
        match self.kind {
            FunctionKind::SetCount => {
                stack.set_count(clamp_count(self.value));
                true
            }
            FunctionKind::AddCount => {
                let next = (i64::from(stack.count()) + self.value as i64).max(0);
                stack.set_count(next as u32);
                true
            }
            FunctionKind::MultiplyCount => {
                let next = (f64::from(stack.count()) * self.value).round().max(0.0);
                stack.set_count(next as u32);
                true
            }
            FunctionKind::SetName => {
                stack.set_name(&self.name);
                true
            }
            FunctionKind::Enchant => {
                let level = rng.next_range(1, 3) as i32;
                stack.add_enchantment(&self.name, level);
                true
            }
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.kind {
            FunctionKind::SetCount => "set_count",
            FunctionKind::AddCount => "add_count",
            FunctionKind::MultiplyCount => "multiply_count",
            FunctionKind::SetName => "set_name",
            FunctionKind::Enchant => "enchant_randomly",
        }
    }
}

fn clamp_count(value: f64) -> u32 {
    if value <= 0.0 {
        0
    } else {
        value as u32
    }
}

/// One thing a pool can draw: an item entry, an empty, or a group of children.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: String,
    pub item: Option<String>,
    pub weight: u32,
    /// A quality modifier: it adds `quality * luck` to the weight, as the game's does.
    pub quality: i32,
    pub count: NumberProvider,
    pub conditions: Vec<Condition>,
    pub functions: Vec<Function>,
    pub children: Vec<Entry>,
}

impl Entry {
    /// An entry that drops a count of an item.
    #[must_use]
    pub fn item(item: &str, count: NumberProvider) -> Self {
        Self {
            kind: "item".to_owned(),
            item: Some(item.to_owned()),
            weight: 1,
            quality: 0,
            count,
            conditions: Vec::new(),
            functions: Vec::new(),
            children: Vec::new(),
        }
    }

    /// An entry that drops nothing, which is how a table says "no drop".
    #[must_use]
    pub fn empty() -> Self {
        Self {
            kind: "empty".to_owned(),
            item: None,
            weight: 1,
            quality: 0,
            count: NumberProvider::fixed(0),
            conditions: Vec::new(),
            functions: Vec::new(),
            children: Vec::new(),
        }
    }

    /// An entry that takes every child whose conditions pass.
    #[must_use]
    pub fn group() -> Self {
        Self {
            kind: "group".to_owned(),
            item: None,
            weight: 1,
            quality: 0,
            count: NumberProvider::fixed(1),
            conditions: Vec::new(),
            functions: Vec::new(),
            children: Vec::new(),
        }
    }

    /// An entry that picks one child by weight.
    #[must_use]
    pub fn alternatives() -> Self {
        Self {
            kind: "alternatives".to_owned(),
            item: None,
            weight: 1,
            quality: 0,
            count: NumberProvider::fixed(1),
            conditions: Vec::new(),
            functions: Vec::new(),
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn weight(mut self, weight: u32) -> Self {
        self.weight = weight;
        self
    }

    #[must_use]
    pub fn quality(mut self, quality: i32) -> Self {
        self.quality = quality;
        self
    }

    #[must_use]
    pub fn count(mut self, count: NumberProvider) -> Self {
        self.count = count;
        self
    }

    /// A condition the entry has to pass to be drawn.
    #[must_use]
    pub fn condition(mut self, condition: Condition) -> Self {
        self.conditions.push(condition);
        self
    }

    /// The same, for the common case: a chance to drop at all.
    #[must_use]
    pub fn chance(self, probability: f64) -> Self {
        self.condition(Condition::chance(probability))
    }

    #[must_use]
    pub fn function(mut self, function: Function) -> Self {
        self.functions.push(function);
        self
    }

    #[must_use]
    pub fn child(mut self, child: Entry) -> Self {
        self.children.push(child);
        self
    }

    /// The weight the draw uses, after the quality modifier.
    #[must_use]
    pub fn effective_weight(&self, luck: f64) -> f64 {
        (f64::from(self.weight) + f64::from(self.quality) * luck).max(0.0)
    }

    /// Whether every one of its conditions passes.
    #[must_use]
    pub fn passes(&self, context: &LootContext, rng: &mut LootRandom) -> bool {
        self.conditions
            .iter()
            .all(|condition| condition.evaluate(context, rng))
    }

    /// Draws what this entry drops, assuming its own conditions already passed.
    #[must_use]
    pub fn produce(&self, context: &LootContext, rng: &mut LootRandom) -> Vec<ItemStack> {
        let mut drops = Vec::new();
        if self.children.is_empty() {
            if let Some(item) = &self.item {
                let count = self.count.roll(rng);
                if count > 0 {
                    drops.push(ItemStack::new(item, count));
                }
            }
        } else if matches!(self.kind.as_str(), "group" | "sequence") {
            for child in &self.children {
                if child.passes(context, rng) {
                    drops.extend(child.produce(context, rng));
                }
            }
        } else {
            let mut eligible: Vec<&Entry> = Vec::new();
            for child in &self.children {
                if child.passes(context, rng) {
                    eligible.push(child);
                }
            }
            if let Some(child) = rng.weighted(&eligible, |child| child.effective_weight(context.luck)) {
                drops.extend(child.produce(context, rng));
            }
        }
        for function in &self.functions {
            for stack in &mut drops {
                apply_function(stack, function, rng);
            }
        }
        drops.retain(|stack| !stack.is_empty());
        drops
    }
}

impl fmt::Display for Entry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = self.item.as_deref().unwrap_or(&self.kind);
        write!(formatter, "{label} (weight {})", self.weight)
    }
}

/// A pool: how many rolls it makes, what it draws from, and what has to hold for it to roll.
#[derive(Debug, Clone, PartialEq)]
pub struct Pool {
    pub id: String,
    pub rolls: NumberProvider,
    pub entries: Vec<Entry>,
    pub conditions: Vec<Condition>,
    pub functions: Vec<Function>,
}

impl Pool {
    #[must_use]
    pub fn new(rolls: NumberProvider) -> Self {
        Self {
            id: String::new(),
            rolls,
            entries: Vec::new(),
            conditions: Vec::new(),
            functions: Vec::new(),
        }
    }

    /// Gives it a name, so it can be registered and reused.
    #[must_use]
    pub fn named(mut self, id: &str) -> Self {
        self.id = id.to_owned();
        self
    }

    #[must_use]
    pub fn entry(mut self, entry: Entry) -> Self {
        self.entries.push(entry);
        self
    }

    #[must_use]
    pub fn condition(mut self, condition: Condition) -> Self {
        self.conditions.push(condition);
        self
    }

    #[must_use]
    pub fn function(mut self, function: Function) -> Self {
        self.functions.push(function);
        self
    }

    /// Whether the pool rolls at all.
    #[must_use]
    pub fn passes(&self, context: &LootContext, rng: &mut LootRandom) -> bool {
        self.conditions
            .iter()
            .all(|condition| condition.evaluate(context, rng))
    }

    /// Rolls the pool: each roll picks one eligible entry by weight.
    #[must_use]
    pub fn draw(&self, context: &LootContext, rng: &mut LootRandom) -> Vec<ItemStack> {
        if !self.passes(context, rng) {
            return Vec::new();
        }
        let rolls = self.rolls.roll(rng);
        let mut drops = Vec::new();
        for _ in 0..rolls {
            let mut eligible: Vec<&Entry> = Vec::new();
            for entry in &self.entries {
                if entry.passes(context, rng) {
                    eligible.push(entry);
                }
            }
            if let Some(entry) = rng.weighted(&eligible, |entry| entry.effective_weight(context.luck)) {
                drops.extend(entry.produce(context, rng));
            }
        }
        for function in &self.functions {
            for stack in &mut drops {
                apply_function(stack, function, rng);
            }
        }
        drops.retain(|stack| !stack.is_empty());
        drops
    }
}

impl fmt::Display for Pool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "pool ({} rolls, {} entries)",
            self.rolls.name(),
            self.entries.len()
        )
    }
}

/// A loot table a mod ships.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub kind: TableKind,
    pub pools: Vec<Pool>,
    pub functions: Vec<Function>,
}

impl Table {
    #[must_use]
    pub fn new(id: &str, kind: TableKind) -> Self {
        Self {
            id: id.to_owned(),
            name: id.to_owned(),
            owner: crate::metadata::id(),
            kind,
            pools: Vec::new(),
            functions: Vec::new(),
        }
    }

    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    #[must_use]
    pub fn pool(mut self, pool: Pool) -> Self {
        self.pools.push(pool);
        self
    }

    #[must_use]
    pub fn function(mut self, function: Function) -> Self {
        self.functions.push(function);
        self
    }

    /// Every entry in every pool, in pool order.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        self.pools
            .iter()
            .flat_map(|pool| pool.entries.iter().cloned())
            .collect()
    }

    /// Adds an entry to the first pool, making one when there is none.
    pub fn add_entry(&mut self, entry: Entry) {
        if self.pools.is_empty() {
            self.pools.push(Pool::new(NumberProvider::fixed(1)));
        }
        if let Some(pool) = self.pools.first_mut() {
            pool.entries.push(entry);
        }
    }

    fn entry_mut(&mut self, index: usize) -> Option<&mut Entry> {
        let mut left = index;
        for pool in &mut self.pools {
            if left < pool.entries.len() {
                return pool.entries.get_mut(left);
            }
            left -= pool.entries.len();
        }
        None
    }

    /// Removes an entry by its position across the pools.
    pub fn remove_entry(&mut self, index: usize) -> Option<Entry> {
        let mut left = index;
        for pool in &mut self.pools {
            if left < pool.entries.len() {
                return Some(pool.entries.remove(left));
            }
            left -= pool.entries.len();
        }
        None
    }

    /// Replaces an entry by position, and answers whether there was one.
    pub fn replace_entry(&mut self, index: usize, entry: Entry) -> bool {
        match self.entry_mut(index) {
            Some(slot) => {
                *slot = entry;
                true
            }
            None => false,
        }
    }

    /// Rolls every pool, applies the table's functions, and answers the drops.
    #[must_use]
    pub fn draw(&self, context: &LootContext) -> Vec<ItemStack> {
        let mut rng = context.random();
        let mut drops = Vec::new();
        for pool in &self.pools {
            drops.extend(pool.draw(context, &mut rng));
        }
        for function in &self.functions {
            for stack in &mut drops {
                apply_function(stack, function, &mut rng);
            }
        }
        drops.retain(|stack| !stack.is_empty());
        drops
    }
}

impl fmt::Display for Table {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {} pools)",
            self.id,
            self.kind.as_str(),
            self.pools.len()
        )
    }
}

/// What the caller supplies to a roll: a seed, and the values a condition tests.
#[derive(Debug, Clone, PartialEq)]
pub struct LootContext {
    pub seed: u64,
    pub level: u32,
    pub luck: f64,
    pub flag: bool,
    pub tool: Option<String>,
    pub entity: String,
}

impl LootContext {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            level: 0,
            luck: 0.0,
            flag: false,
            tool: None,
            entity: String::new(),
        }
    }

    #[must_use]
    pub fn level(mut self, level: u32) -> Self {
        self.level = level;
        self
    }

    #[must_use]
    pub fn luck(mut self, luck: f64) -> Self {
        self.luck = luck;
        self
    }

    #[must_use]
    pub fn flag(mut self, flag: bool) -> Self {
        self.flag = flag;
        self
    }

    #[must_use]
    pub fn tool(mut self, tool: &str) -> Self {
        self.tool = Some(tool.to_owned());
        self
    }

    #[must_use]
    pub fn entity(mut self, entity: &str) -> Self {
        self.entity = entity.to_owned();
        self
    }

    /// A generator seeded from this context.
    #[must_use]
    pub fn random(&self) -> LootRandom {
        LootRandom::new(self.seed)
    }
}

impl fmt::Display for LootContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "context (seed {}, level {})", self.seed, self.level)
    }
}

/// What a drop modifier does to a whole roll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LootModifierKind {
    Multiply,
    Add,
    Replace,
    Clear,
    SetCount,
}

/// A modification applied to a table's drops, which is what a bonus round or a game rule is.
#[derive(Debug, Clone, PartialEq)]
pub struct LootModifier {
    pub kind: LootModifierKind,
    pub value: f64,
    pub item: String,
}

impl LootModifier {
    #[must_use]
    pub fn multiply(factor: f64) -> Self {
        Self {
            kind: LootModifierKind::Multiply,
            value: factor,
            item: String::new(),
        }
    }

    #[must_use]
    pub fn add(item: &str, count: u32) -> Self {
        Self {
            kind: LootModifierKind::Add,
            value: f64::from(count),
            item: item.to_owned(),
        }
    }

    #[must_use]
    pub fn replace(item: &str, count: u32) -> Self {
        Self {
            kind: LootModifierKind::Replace,
            value: f64::from(count),
            item: item.to_owned(),
        }
    }

    #[must_use]
    pub fn clear() -> Self {
        Self {
            kind: LootModifierKind::Clear,
            value: 0.0,
            item: String::new(),
        }
    }

    #[must_use]
    pub fn set_count(count: u32) -> Self {
        Self {
            kind: LootModifierKind::SetCount,
            value: f64::from(count),
            item: String::new(),
        }
    }

    /// Applies it to a list of drops, and answers how many it touched.
    pub fn apply(&self, drops: &mut Vec<ItemStack>) -> usize {
        match self.kind {
            LootModifierKind::Multiply => {
                for stack in drops.iter_mut() {
                    let next = (f64::from(stack.count()) * self.value).round().max(0.0);
                    stack.set_count(next as u32);
                }
                drops.len()
            }
            LootModifierKind::Add => {
                let count = clamp_count(self.value);
                if count == 0 {
                    0
                } else {
                    drops.push(ItemStack::new(&self.item, count));
                    1
                }
            }
            LootModifierKind::Replace => {
                drops.clear();
                let count = clamp_count(self.value);
                if count > 0 {
                    drops.push(ItemStack::new(&self.item, count));
                }
                1
            }
            LootModifierKind::Clear => {
                let cleared = drops.len();
                drops.clear();
                cleared
            }
            LootModifierKind::SetCount => {
                let count = clamp_count(self.value);
                for stack in drops.iter_mut() {
                    stack.set_count(count);
                }
                drops.len()
            }
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.kind {
            LootModifierKind::Multiply => "multiply",
            LootModifierKind::Add => "add",
            LootModifierKind::Replace => "replace",
            LootModifierKind::Clear => "clear",
            LootModifierKind::SetCount => "set_count",
        }
    }
}

/// Applies a function to a stack. The free-function spelling of [`Function::apply`].
pub fn apply_function(stack: &mut ItemStack, function: &Function, rng: &mut LootRandom) -> bool {
    function.apply(stack, rng)
}

/// Evaluates a condition against a context. The free-function spelling of
/// [`Condition::evaluate`].
pub fn evaluate_condition(condition: &Condition, context: &LootContext, rng: &mut LootRandom) -> bool {
    condition.evaluate(context, rng)
}

/// Applies a modifier to a list of drops, and answers how many it touched.
pub fn modify_drops(drops: &mut Vec<ItemStack>, modifier: &LootModifier) -> usize {
    modifier.apply(drops)
}

static SEED: AtomicU64 = AtomicU64::new(DEFAULT_SEED);

/// Sets the seed a context starts from, and answers the previous one.
pub fn set_seed(seed: u64) -> u64 {
    SEED.swap(seed, Ordering::Relaxed)
}

/// The context a roll starts from, seeded with whatever [`set_seed`] last set.
#[must_use]
pub fn get_context() -> LootContext {
    LootContext::new(SEED.load(Ordering::Relaxed))
}

fn tables() -> &'static Mutex<BTreeMap<String, Table>> {
    static TABLES: OnceLock<Mutex<BTreeMap<String, Table>>> = OnceLock::new();
    TABLES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_tables<T>(body: impl FnOnce(&mut BTreeMap<String, Table>) -> T) -> T {
    let mut guard = tables().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn pools() -> &'static Mutex<BTreeMap<String, Pool>> {
    static POOLS: OnceLock<Mutex<BTreeMap<String, Pool>>> = OnceLock::new();
    POOLS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_pools<T>(body: impl FnOnce(&mut BTreeMap<String, Pool>) -> T) -> T {
    let mut guard = pools().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn names() -> &'static Mutex<BTreeMap<String, Vec<String>>> {
    static NAMES: OnceLock<Mutex<BTreeMap<String, Vec<String>>>> = OnceLock::new();
    NAMES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_names<T>(body: impl FnOnce(&mut BTreeMap<String, Vec<String>>) -> T) -> T {
    let mut guard = names().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn register_name(category: &str, name: &str) -> bool {
    with_names(|names| {
        let list = names.entry(category.to_owned()).or_default();
        if list.iter().any(|held| held == name) {
            false
        } else {
            list.push(name.to_owned());
            true
        }
    })
}

fn get_names(category: &str) -> Vec<String> {
    with_names(|names| names.get(category).cloned().unwrap_or_default())
}

/// Registers a table, or replaces the one with that id.
pub fn register_table(table: Table) -> bool {
    with_tables(|tables| tables.insert(table.id.clone(), table).is_none())
}

/// Forgets a table.
pub fn unregister_table(id: &str) -> bool {
    with_tables(|tables| tables.remove(id).is_some())
}

/// A table by id.
#[must_use]
pub fn get_table(id: &str) -> Option<Table> {
    with_tables(|tables| tables.get(id).cloned())
}

/// Every registered table, in id order.
#[must_use]
pub fn get_tables() -> Vec<Table> {
    with_tables(|tables| tables.values().cloned().collect())
}

/// Every entry of a table, pools flattened in order.
#[must_use]
pub fn get_entries(table: &str) -> Vec<Entry> {
    match get_table(table) {
        Some(table) => table.entries(),
        None => Vec::new(),
    }
}

/// Adds an entry to a table's first pool.
pub fn add_entry(table: &str, entry: Entry) -> bool {
    with_tables(|tables| match tables.get_mut(table) {
        Some(table) => {
            table.add_entry(entry);
            true
        }
        None => false,
    })
}

/// Removes an entry by position across a table's pools.
pub fn remove_entry(table: &str, index: usize) -> Option<Entry> {
    with_tables(|tables| tables.get_mut(table).and_then(|table| table.remove_entry(index)))
}

/// Replaces an entry by position, and answers whether there was one.
pub fn replace_entry(table: &str, index: usize, entry: Entry) -> bool {
    with_tables(|tables| tables.get_mut(table).is_some_and(|table| table.replace_entry(index, entry)))
}

/// Rolls a table with a seed, and answers the drops.
#[must_use]
pub fn generate(table: &str, seed: u64) -> Vec<ItemStack> {
    generate_with(table, &LootContext::new(seed))
}

/// The same, with a caller's own context.
#[must_use]
pub fn generate_with(table: &str, context: &LootContext) -> Vec<ItemStack> {
    match get_table(table) {
        Some(table) => table.draw(context),
        None => Vec::new(),
    }
}

/// The drops for a block, which is the table a mod registered under the block's name.
#[must_use]
pub fn generate_for_block(block: &str, seed: u64) -> Vec<ItemStack> {
    generate_for(block, TableKind::Block, seed)
}

/// The same for an entity.
#[must_use]
pub fn generate_for_entity(entity: &str, seed: u64) -> Vec<ItemStack> {
    generate_for(entity, TableKind::Entity, seed)
}

/// The same for a container.
#[must_use]
pub fn generate_for_container(container: &str, seed: u64) -> Vec<ItemStack> {
    generate_for(container, TableKind::Container, seed)
}

/// The same for the game's fishing table.
#[must_use]
pub fn generate_for_fishing(seed: u64) -> Vec<ItemStack> {
    generate_for(FISHING_TABLE, TableKind::Fishing, seed)
}

fn generate_for(name: &str, kind: TableKind, seed: u64) -> Vec<ItemStack> {
    match get_table(name) {
        Some(table) if table.kind == kind || table.kind == TableKind::Generic => {
            table.draw(&LootContext::new(seed))
        }
        _ => Vec::new(),
    }
}

/// Gives a roll's drops to an inventory a mod owns, and answers how many stacks fit whole.
pub fn give_drops(inventory: &mut Inventory, drops: &[ItemStack]) -> usize {
    let mut given = 0;
    for stack in drops {
        if inventory.add_item(stack).is_ok_and(|left| left == 0) {
            given += 1;
        }
    }
    given
}

/// Records that a mod has a condition kind.
pub fn register_condition(name: &str) -> bool {
    register_name("condition", name)
}

/// Records that a mod has a function kind.
pub fn register_function(name: &str) -> bool {
    register_name("function", name)
}

/// Records that a mod has an entry type.
pub fn register_entry_type(name: &str) -> bool {
    register_name("entry_type", name)
}

/// Records that a mod has a number provider.
pub fn register_number_provider(name: &str) -> bool {
    register_name("number_provider", name)
}

/// Records that a mod has a drop modifier kind.
pub fn register_modifier(name: &str) -> bool {
    register_name("modifier", name)
}

/// Registers a named pool a table can use. An unnamed pool cannot be registered.
pub fn register_pool(pool: Pool) -> bool {
    if pool.id.is_empty() {
        return false;
    }
    with_pools(|pools| pools.insert(pool.id.clone(), pool).is_none())
}

/// A named pool.
#[must_use]
pub fn get_pool(id: &str) -> Option<Pool> {
    with_pools(|pools| pools.get(id).cloned())
}

#[must_use]
pub fn get_conditions() -> Vec<String> {
    get_names("condition")
}

#[must_use]
pub fn get_functions() -> Vec<String> {
    get_names("function")
}

#[must_use]
pub fn get_entry_types() -> Vec<String> {
    get_names("entry_type")
}

#[must_use]
pub fn get_number_providers() -> Vec<String> {
    get_names("number_provider")
}

#[must_use]
pub fn get_modifiers() -> Vec<String> {
    get_names("modifier")
}

/// Why a loot question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client" => {
            "the client's loot tables come from its own data pack, which this host cannot \
             read; it rolls the tables a mod registers here instead"
                .to_owned()
        }
        "server" => {
            "the server resolves the table that actually dropped an item, and this host is \
             not the server, so it never sees that roll"
                .to_owned()
        }
        "rules" => {
            "what a block or entity leaves behind is the client's drop rules, which live in \
             the client's data and are not reachable from a mod here"
                .to_owned()
        }
        "world" => {
            "rolling a table against a live entity, block or fishing hook would need the \
             client's world, which this host does not hold"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client", "server", "rules", "world"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own loot table. `None`, and a warning.
#[must_use]
pub fn get_client_table(id: &str) -> Option<Table> {
    crate::warn!(
        "a mod asked this host for the client's loot table {id}: {}",
        why_unknown("client")
    );
    None
}

/// The table the server resolved. `None`, and a warning.
#[must_use]
pub fn get_server_table(id: &str) -> Option<Table> {
    crate::warn!(
        "a mod asked this host for the server's loot table {id}: {}",
        why_unknown("server")
    );
    None
}

/// The client's drop rules for a block or entity. Empty, and a warning.
#[must_use]
pub fn get_drop_rules(subject: &str) -> Vec<String> {
    crate::warn!(
        "a mod asked this host for the client's drop rules for {subject}: {}",
        why_unknown("rules")
    );
    Vec::new()
}

/// Rolls a table in the client's world. Empty, and a warning: this host has no world.
#[must_use]
pub fn drop_in_world(table: &str, position: (i32, i32, i32)) -> Vec<ItemStack> {
    crate::warn!(
        "a mod asked this host to roll {table} in the client's world at {position:?}: {}",
        why_unknown("world")
    );
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_tables(|tables| tables.clear());
        with_pools(|pools| pools.clear());
        with_names(|names| names.clear());
    }

    fn sample_table() -> Table {
        Table::new("hello:grass", TableKind::Block).pool(
            Pool::new(NumberProvider::fixed(1))
                .entry(Entry::item("minecraft:short_grass", NumberProvider::fixed(1)).weight(3))
                .entry(Entry::item("minecraft:wheat_seeds", NumberProvider::uniform(1, 3)).weight(1)),
        )
    }

    #[test]
    fn the_same_seed_gives_the_same_drops_and_a_different_seed_does_not() {
        let _serial = serial();
        fresh();
        assert!(register_table(sample_table()));
        let first = generate("hello:grass", 7);
        let again = generate("hello:grass", 7);
        assert_eq!(first, again, "seed 7 twice gives the same drops");

        let mut distinct: Vec<Vec<ItemStack>> = Vec::new();
        for seed in 0..40u64 {
            let drops = generate("hello:grass", seed);
            if !distinct.contains(&drops) {
                distinct.push(drops);
            }
        }
        assert!(distinct.len() > 1, "different seeds give different drops");
        assert!(generate("nothing", 7).is_empty(), "a table that is not there drops nothing");

        let previous = set_seed(99);
        assert_eq!(get_context().seed, 99);
        assert_eq!(set_seed(previous), 99, "and it answers the one before");
    }

    #[test]
    fn a_pools_roll_range_is_respected() {
        let _serial = serial();
        fresh();
        register_table(Table::new("hello:rolls", TableKind::Generic).pool(
            Pool::new(NumberProvider::uniform(1, 3))
                .entry(Entry::item("minecraft:cobblestone", NumberProvider::fixed(1))),
        ));
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..64u64 {
            let drops = generate("hello:rolls", seed);
            assert!((1..=3).contains(&drops.len()), "seed {seed} gave {}", drops.len());
            seen.insert(drops.len());
        }
        assert_eq!(seen.len(), 3, "every count in the range shows up: {seen:?}");

        register_table(Table::new("hello:noroll", TableKind::Generic).pool(
            Pool::new(NumberProvider::fixed(0))
                .entry(Entry::item("minecraft:stone", NumberProvider::fixed(1))),
        ));
        assert!(generate("hello:noroll", 3).is_empty(), "zero rolls drop nothing");
    }

    #[test]
    fn a_count_is_always_inside_its_declared_range() {
        let _serial = serial();
        fresh();
        register_table(Table::new("hello:seeds", TableKind::Generic).pool(
            Pool::new(NumberProvider::fixed(1))
                .entry(Entry::item("minecraft:wheat_seeds", NumberProvider::uniform(2, 5))),
        ));
        for seed in 0..64u64 {
            for stack in generate("hello:seeds", seed) {
                assert!((2..=5).contains(&stack.count()), "seed {seed} gave {}", stack.count());
            }
        }

        register_table(Table::new("hello:empties", TableKind::Generic).pool(
            Pool::new(NumberProvider::fixed(1))
                .entry(Entry::item("minecraft:air", NumberProvider::fixed(0))),
        ));
        assert!(generate("hello:empties", 1).is_empty(), "a zero count is no drop");
    }

    #[test]
    fn a_failed_condition_excludes_an_entry() {
        let _serial = serial();
        fresh();
        register_table(Table::new("hello:gated", TableKind::Generic).pool(
            Pool::new(NumberProvider::fixed(1))
                .entry(Entry::item("minecraft:diamond", NumberProvider::fixed(1)).condition(Condition::level(5))),
        ));
        let low = LootContext::new(1).level(4);
        let high = LootContext::new(1).level(5);
        assert!(generate_with("hello:gated", &low).is_empty(), "level 4 is under the gate");
        assert_eq!(generate_with("hello:gated", &high).len(), 1, "level 5 passes it");

        register_table(Table::new("hello:never", TableKind::Generic).pool(
            Pool::new(NumberProvider::fixed(1))
                .entry(Entry::item("minecraft:stone", NumberProvider::fixed(1)).chance(0.0)),
        ));
        assert!(generate("hello:never", 9).is_empty(), "a chance of zero never drops");

        assert!(Condition::flag(true).evaluate(&LootContext::new(1).flag(true), &mut LootRandom::new(1)));
        assert!(!Condition::tool("minecraft:diamond_pickaxe")
            .evaluate(&LootContext::new(1).tool("minecraft:iron_pickaxe"), &mut LootRandom::new(1)));
        assert!(Condition::entity_type("minecraft:zombie")
            .evaluate(&LootContext::new(1).entity("minecraft:zombie"), &mut LootRandom::new(1)));
    }

    #[test]
    fn the_weighted_draw_favours_the_heavier_entry() {
        let _serial = serial();
        fresh();
        register_table(Table::new("hello:weights", TableKind::Generic).pool(
            Pool::new(NumberProvider::fixed(1))
                .entry(Entry::item("minecraft:diamond", NumberProvider::fixed(1)).weight(9))
                .entry(Entry::item("minecraft:coal", NumberProvider::fixed(1)).weight(1)),
        ));
        let mut diamonds = 0;
        let mut coal = 0;
        for seed in 0..400u64 {
            for stack in generate("hello:weights", seed) {
                match stack.item() {
                    "minecraft:diamond" => diamonds += 1,
                    "minecraft:coal" => coal += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(diamonds + coal, 400, "exactly one entry per roll");
        assert!(diamonds > 300, "a weight of nine wins most draws: {diamonds}");
        assert!(coal > 0, "and a weight of one still wins some: {coal}");
        assert!(diamonds > coal * 3, "{diamonds} should dwarf {coal}");
    }

    #[test]
    fn functions_sub_entries_and_modifiers_change_the_drops() {
        let _serial = serial();
        fresh();
        let mut stack = ItemStack::new("minecraft:iron_ore", 1);
        assert!(apply_function(&mut stack, &Function::set_count(4), &mut LootRandom::new(1)));
        assert_eq!(stack.count(), 4);
        assert!(apply_function(&mut stack, &Function::add_count(2), &mut LootRandom::new(1)));
        assert_eq!(stack.count(), 6);
        assert!(apply_function(&mut stack, &Function::multiply_count(2.0), &mut LootRandom::new(1)));
        assert_eq!(stack.count(), 12);
        assert!(apply_function(&mut stack, &Function::set_name("Ore"), &mut LootRandom::new(1)));
        assert_eq!(stack.name(), Some("Ore"));

        let picks = Entry::alternatives()
            .child(Entry::item("minecraft:a", NumberProvider::fixed(1)).weight(1))
            .child(Entry::item("minecraft:b", NumberProvider::fixed(1)).weight(1));
        assert_eq!(picks.produce(&LootContext::new(1), &mut LootRandom::new(1)).len(), 1, "one child");
        let all = Entry::group()
            .child(Entry::item("minecraft:a", NumberProvider::fixed(1)))
            .child(Entry::item("minecraft:b", NumberProvider::fixed(1)));
        assert_eq!(all.produce(&LootContext::new(1), &mut LootRandom::new(1)).len(), 2, "every child");

        let mut drops = vec![ItemStack::new("minecraft:iron_ore", 2)];
        assert_eq!(modify_drops(&mut drops, &LootModifier::multiply(3.0)), 1);
        assert_eq!(drops[0].count(), 6);
        assert_eq!(modify_drops(&mut drops, &LootModifier::add("minecraft:coal", 1)), 1);
        assert_eq!(drops.len(), 2);
        assert_eq!(modify_drops(&mut drops, &LootModifier::clear()), 2);
        assert!(drops.is_empty());

        let mut inventory = Inventory::player();
        assert_eq!(give_drops(&mut inventory, &[ItemStack::new("minecraft:iron_ore", 3)]), 1);
        assert_eq!(inventory.count("minecraft:iron_ore"), 3);
    }

    #[test]
    fn every_registered_name_reads_back() {
        let _serial = serial();
        fresh();
        assert!(register_condition("hello:raining"));
        assert!(!register_condition("hello:raining"), "once");
        assert!(register_function("hello:smelt"));
        assert!(!register_function("hello:smelt"));
        assert!(register_entry_type("hello:tag"));
        assert!(!register_entry_type("hello:tag"));
        assert!(register_number_provider("hello:binomial"));
        assert!(!register_number_provider("hello:binomial"));
        assert!(register_modifier("hello:fortune"));
        assert!(!register_modifier("hello:fortune"));
        assert!(get_conditions().contains(&"hello:raining".to_owned()));
        assert!(get_functions().contains(&"hello:smelt".to_owned()));
        assert!(get_entry_types().contains(&"hello:tag".to_owned()));
        assert!(get_number_providers().contains(&"hello:binomial".to_owned()));
        assert!(get_modifiers().contains(&"hello:fortune".to_owned()));

        assert!(register_pool(Pool::new(NumberProvider::fixed(1)).named("hello:gems")));
        assert!(!register_pool(Pool::new(NumberProvider::fixed(1)).named("hello:gems")), "once");
        assert!(!register_pool(Pool::new(NumberProvider::fixed(1))), "an unnamed pool cannot register");
        assert!(get_pool("hello:gems").is_some());
        assert!(get_pool("nothing").is_none());

        assert_eq!(NumberProvider::uniform(4, 2).min(), 2, "a backwards range is sorted");
        assert_eq!(NumberProvider::uniform(4, 2).max(), 4);
        assert_eq!(NumberProvider::fixed(3).name(), "constant");
        assert_eq!(NumberProvider::uniform(1, 2).name(), "uniform");
    }

    #[test]
    fn tables_are_registered_listed_read_and_removed() {
        let _serial = serial();
        fresh();
        assert!(register_table(sample_table()));
        assert!(!register_table(sample_table()), "the second replaces the first");
        assert_eq!(get_tables().len(), 1);
        assert_eq!(get_table("hello:grass").map(|table| table.kind), Some(TableKind::Block));
        assert_eq!(get_entries("hello:grass").len(), 2);

        assert!(add_entry("hello:grass", Entry::item("minecraft:dirt", NumberProvider::fixed(1))));
        assert_eq!(get_entries("hello:grass").len(), 3);
        assert!(!add_entry("nothing", Entry::empty()));
        let removed = remove_entry("hello:grass", 2).expect("there");
        assert_eq!(removed.item.as_deref(), Some("minecraft:dirt"));
        assert!(remove_entry("hello:grass", 2).is_none());
        assert!(replace_entry("hello:grass", 0, Entry::item("minecraft:stone", NumberProvider::fixed(1))));
        assert_eq!(get_entries("hello:grass")[0].item.as_deref(), Some("minecraft:stone"));
        assert!(!replace_entry("hello:grass", 9, Entry::empty()));

        assert_eq!(generate_for_block("hello:grass", 3), generate("hello:grass", 3));
        assert!(generate_for_entity("hello:grass", 3).is_empty(), "a block table is not an entity table");

        register_table(Table::new(FISHING_TABLE, TableKind::Fishing).pool(
            Pool::new(NumberProvider::fixed(1)).entry(Entry::item("minecraft:cod", NumberProvider::fixed(1))),
        ));
        assert_eq!(generate_for_fishing(5).len(), 1, "the fishing table rolls");
        assert!(generate_for_container("hello:grass", 5).is_empty(), "nor is it a container table");
        assert!(register_table(Table::new("hello:chest", TableKind::Container).named("Chest")));
        assert_eq!(get_table("hello:chest").map(|table| table.name), Some("Chest".to_owned()));
        assert_eq!(generate_for_container("hello:chest", 5).len(), 0, "an empty container drops nothing");

        assert!(unregister_table("hello:grass"));
        assert!(!unregister_table("hello:grass"));
    }

    #[test]
    fn the_client_server_rules_and_world_are_refused() {
        let _serial = serial();
        assert!(get_client_table("minecraft:blocks/stone").is_none());
        assert!(get_server_table("minecraft:chests/simple_dungeon").is_none());
        assert!(get_drop_rules("minecraft:stone").is_empty());
        assert!(drop_in_world("hello:grass", (0, 64, 0)).is_empty());
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }

        assert_eq!(LootRandom::new(42).next_u32(), LootRandom::new(42).next_u32());
        assert_ne!(LootRandom::new(42).next_u32(), LootRandom::new(43).next_u32());
        let mut rng = LootRandom::new(7);
        for _ in 0..64 {
            let value = rng.next_f64();
            assert!((0.0..1.0).contains(&value), "{value}");
        }
        for _ in 0..64 {
            let value = rng.next_range(3, 7);
            assert!((3..=7).contains(&value), "{value}");
        }
    }
}
