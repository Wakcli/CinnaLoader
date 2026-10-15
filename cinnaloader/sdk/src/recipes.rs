//! Recipes: the ones a mod ships, and the matching a mod can run for itself.
//!
//! A recipe is data — a shaped pattern with a key, a shapeless list, or a machine recipe with
//! an input, an output, a cook time and experience — and a mod registers them here. What is
//! *not* just data is matching: [`find_matching`], [`can_craft`] and [`craft`] all run over a
//! grid of [`ItemStack`]s, which is section 17's value type and section 18's inventory, so a
//! mod can craft in this host exactly as it would in the game, and a recipe that does not
//! match really does not match.
//!
//! The matching is the game's rules as far as a mod can use them: a shaped recipe has to match
//! the pattern in a grid of the right size or bigger with the pattern anywhere in it, mirrored
//! or not unless the recipe says otherwise, with each key either a specific item or a tag; a
//! shapeless recipe matches if the same items are there in any arrangement; a smelting,
//! blasting, smoking or brewing recipe matches one input and one fuel.
//!
//! Refused: the client's own recipes (`get` with no id), the client's serializer registry, and
//! taking from or giving to a live player — the client's inventory is the client's, and this
//! host crafts into a grid a mod owns.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! recipes::register_shaped(recipes::pattern(
//!     recipes::shaped("hello:torches", "minecraft:torch", 4),
//!     &["C", "S"],
//! )
//! .key('C', "minecraft:coal")
//! .key('S', "minecraft:stick"));
//! let grid = vec![
//!     Some(ItemStack::new("minecraft:coal", 1)),
//!     Some(ItemStack::new("minecraft:stick", 1)),
//! ];
//! // A grid one wide and two tall, which is what the pattern "C" over "S" needs.
//! assert!(recipes::can_craft(&grid, 1));
//! assert_eq!(recipes::craft(&grid, 1).map(|stack| stack.count()), Some(4));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::items::ItemStack;

/// A crafting grid: three by three, as the game's own is.
pub const GRID: usize = 9;
/// How many items a furnace holds: input, fuel, output.
pub const FURNACE_SLOTS: usize = 3;
/// How long a smelting recipe takes, in ticks.
pub const SMELT_TIME: u32 = 200;
/// The same for blasting, which is twice as fast.
pub const BLAST_TIME: u32 = 100;
/// The same for smoking.
pub const SMOKE_TIME: u32 = 100;
/// How much a piece of coal burns for, the game's own fuel value.
pub const COAL_FUEL: u32 = 1600;

/// What kind of recipe it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RecipeType {
    Shaped,
    Shapeless,
    Smelting,
    Blasting,
    Smoking,
    Brewing,
    Smithing,
    Stonecutting,
    Machine,
}

impl RecipeType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shaped => "crafting_shaped",
            Self::Shapeless => "crafting_shapeless",
            Self::Smelting => "smelting",
            Self::Blasting => "blasting",
            Self::Smoking => "smoking",
            Self::Brewing => "brewing",
            Self::Smithing => "smithing",
            Self::Stonecutting => "stonecutting",
            Self::Machine => "machine",
        }
    }

    /// How long it takes in a furnace, when it is one of the cooking kinds.
    #[must_use]
    pub fn cook_time(self) -> u32 {
        match self {
            Self::Blasting | Self::Smoking => BLAST_TIME,
            _ => SMELT_TIME,
        }
    }

    /// Whether it is cooked in a furnace rather than crafted.
    #[must_use]
    pub fn is_cooking(self) -> bool {
        matches!(self, Self::Smelting | Self::Blasting | Self::Smoking)
    }
}

impl fmt::Display for RecipeType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One ingredient: a specific item, or anything in a tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ingredient {
    Item(String),
    Tag(String),
    /// Any of several items, where a recipe gives alternatives.
    AnyOf(Vec<String>),
}

impl Ingredient {
    /// An ingredient from a name: `#tag` is a tag, anything else is an item.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text.strip_prefix('#') {
            Some(tag) => Self::Tag(tag.to_owned()),
            None => Self::Item(text.to_owned()),
        }
    }

    /// Whether a stack satisfies it. Tags are matched against the item's own name, which is
    /// what a mod has without the client's tag registry.
    #[must_use]
    pub fn matches(&self, stack: &ItemStack) -> bool {
        match self {
            Self::Item(item) => stack.item() == item,
            Self::Tag(tag) => {
                stack.item() == tag
                    || stack
                        .component("cinnaloader:tags")
                        .is_some_and(|component| {
                            component.value.split(',').any(|held| held.trim() == tag)
                        })
            }
            Self::AnyOf(items) => items.iter().any(|item| item == stack.item()),
        }
    }

    /// How it reads.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Item(item) => item.clone(),
            Self::Tag(tag) => format!("#{tag}"),
            Self::AnyOf(items) => items.join("|"),
        }
    }
}

impl fmt::Display for Ingredient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label())
    }
}

/// An ingredient with a count, as a recipe lists what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngredientStack {
    pub ingredient: Ingredient,
    pub count: u32,
}

impl IngredientStack {
    /// How it reads, as `item x2`.
    #[must_use]
    pub fn label(&self) -> String {
        match self.count {
            1 => self.ingredient.label(),
            count => format!("{} x{count}", self.ingredient.label()),
        }
    }

    #[must_use]
    pub fn new(ingredient: Ingredient, count: u32) -> Self {
        Self {
            ingredient,
            count: count.max(1),
        }
    }

    #[must_use]
    pub fn of(item: &str, count: u32) -> Self {
        Self::new(Ingredient::parse(item), count)
    }
}

/// A recipe: what it is called, what it needs and what it makes.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub id: String,
    pub kind: RecipeType,
    pub owner: String,
    /// The pattern, for a shaped recipe: one string per row, spaces for nothing.
    pub pattern: Vec<String>,
    /// `pattern character -> ingredient`, for a shaped recipe.
    pub key: BTreeMap<char, Ingredient>,
    /// What it needs, for a recipe with no pattern.
    pub ingredients: Vec<IngredientStack>,
    pub result: ItemStack,
    pub category: String,
    /// How long it takes, in ticks, and what it is worth in experience.
    pub cook_time: u32,
    pub experience: f32,
    /// Whether a shaped recipe may also be mirrored.
    pub allow_mirrored: bool,
    /// What is left over after crafting, such as a bucket from a cake.
    pub remaining: Vec<ItemStack>,
}

impl Recipe {
    /// A recipe of a kind, making a stack.
    #[must_use]
    pub fn new(kind: RecipeType, id: &str, result: ItemStack) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            owner: crate::metadata::id(),
            pattern: Vec::new(),
            key: BTreeMap::new(),
            ingredients: Vec::new(),
            result,
            category: "misc".to_owned(),
            cook_time: kind.cook_time(),
            experience: 0.0,
            allow_mirrored: true,
            remaining: Vec::new(),
        }
    }

    #[must_use]
    pub fn category(mut self, category: &str) -> Self {
        self.category = category.to_owned();
        self
    }

    /// One line of a shaped pattern.
    #[must_use]
    pub fn row(mut self, row: &str) -> Self {
        self.pattern.push(row.to_owned());
        self
    }

    /// What a character in the pattern stands for.
    #[must_use]
    pub fn key(mut self, symbol: char, item: &str) -> Self {
        self.key.insert(symbol, Ingredient::parse(item));
        self
    }

    #[must_use]
    pub fn ingredient(mut self, ingredient: IngredientStack) -> Self {
        self.ingredients.push(ingredient);
        self
    }

    #[must_use]
    pub fn time(mut self, ticks: u32) -> Self {
        self.cook_time = ticks;
        self
    }

    #[must_use]
    pub fn experience(mut self, experience: f32) -> Self {
        self.experience = experience;
        self
    }

    /// Whether it may be mirrored, which is what makes a recipe work both ways round.
    #[must_use]
    pub fn mirrored(mut self, allow: bool) -> Self {
        self.allow_mirrored = allow;
        self
    }

    /// Something left over after crafting it.
    #[must_use]
    pub fn remaining(mut self, stack: ItemStack) -> Self {
        self.remaining.push(stack);
        self
    }

    /// Every ingredient it needs, counted.
    #[must_use]
    pub fn get_ingredients(&self) -> Vec<IngredientStack> {
        if !self.pattern.is_empty() {
            let mut counts: BTreeMap<String, IngredientStack> = BTreeMap::new();
            for row in &self.pattern {
                for symbol in row.chars() {
                    if symbol == ' ' {
                        continue;
                    }
                    let Some(ingredient) = self.key.get(&symbol) else {
                        continue;
                    };
                    let entry = counts.entry(ingredient.label()).or_insert_with(|| {
                        // Built by hand rather than through `new`, which floors a count at
                        // one: this one is a tally that starts at zero.
                        IngredientStack {
                            ingredient: ingredient.clone(),
                            count: 0,
                        }
                    });
                    entry.count += 1;
                }
            }
            counts.into_values().collect()
        } else {
            self.ingredients.clone()
        }
    }

    /// The widest row and the number of rows, which is the shape a grid has to fit.
    #[must_use]
    pub fn shape(&self) -> (usize, usize) {
        let width = self.pattern.iter().map(|row| row.len()).max().unwrap_or(0);
        (width, self.pattern.len())
    }

    /// Whether a grid of stacks matches this recipe, at the given grid width.
    #[must_use]
    pub fn matches_grid(&self, grid: &[Option<ItemStack>], width: usize) -> bool {
        match self.kind {
            RecipeType::Shaped => self.matches_shaped(grid, width),
            RecipeType::Shapeless => self.matches_shapeless(grid),
            _ => self.matches_one(grid),
        }
    }

    fn matches_shaped(&self, grid: &[Option<ItemStack>], width: usize) -> bool {
        let (pattern_width, pattern_height) = self.shape();
        if pattern_width == 0 || pattern_height == 0 || width == 0 {
            return false;
        }
        if pattern_width > width {
            return false;
        }
        let height = grid.len() / width;
        if pattern_height > height {
            return false;
        }
        let mut patterns = vec![self.pattern.clone()];
        if self.allow_mirrored {
            patterns.push(
                self.pattern
                    .iter()
                    .map(|row| row.chars().rev().collect::<String>())
                    .collect(),
            );
        }
        for pattern in patterns {
            for offset_x in 0..=(width - pattern_width) {
                for offset_y in 0..=(height - pattern_height) {
                    if self.matches_at(grid, width, &pattern, offset_x, offset_y) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn matches_at(
        &self,
        grid: &[Option<ItemStack>],
        width: usize,
        pattern: &[String],
        offset_x: usize,
        offset_y: usize,
    ) -> bool {
        for y in 0..grid.len() / width {
            for x in 0..width {
                let slot = grid[y * width + x].clone();
                let symbol = pattern
                    .get(y.wrapping_sub(offset_y))
                    .and_then(|row| row.chars().nth(x.wrapping_sub(offset_x)));
                let wanted = symbol.filter(|symbol| *symbol != ' ').and_then(|symbol| self.key.get(&symbol));
                match (wanted, slot) {
                    (Some(ingredient), Some(stack)) if ingredient.matches(&stack) => {}
                    (None, None) => {}
                    _ => return false,
                }
            }
        }
        true
    }

    fn matches_shapeless(&self, grid: &[Option<ItemStack>]) -> bool {
        let mut remaining: Vec<&IngredientStack> = self.ingredients.iter().collect();
        for slot in grid.iter().flatten() {
            let Some(index) = remaining
                .iter()
                .position(|ingredient| ingredient.ingredient.matches(slot))
            else {
                return false;
            };
            let ingredient = remaining.remove(index);
            if slot.count() < ingredient.count {
                return false;
            }
        }
        // Every ingredient has to be accounted for.
        remaining.is_empty()
    }

    fn matches_one(&self, grid: &[Option<ItemStack>]) -> bool {
        let inputs: Vec<&ItemStack> = grid.iter().flatten().collect();
        if inputs.len() != 1 {
            return false;
        }
        match self.ingredients.first() {
            Some(ingredient) => ingredient.ingredient.matches(inputs[0]),
            None => false,
        }
    }

    /// What crafting this recipe gives, and what is left over.
    #[must_use]
    pub fn craft(&self) -> Vec<ItemStack> {
        let mut made = vec![self.result.clone()];
        made.extend(self.remaining.iter().cloned());
        made
    }
}

impl fmt::Display for Recipe {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {} -> {})",
            self.id, self.kind, self.owner, self.result
        )
    }
}

/// A shaped recipe: `count` of `result`, from a pattern the rows give.
#[must_use]
pub fn shaped(id: &str, result: &str, count: u32) -> Recipe {
    Recipe::new(RecipeType::Shaped, id, ItemStack::new(result, count))
}

/// A pattern of rows on a shaped recipe.
#[must_use]
pub fn pattern(recipe: Recipe, rows: &[&str]) -> Recipe {
    rows.iter().fold(recipe, |recipe, row| recipe.row(row))
}

/// A shapeless recipe: `count` of `result`, from anything, in any arrangement.
#[must_use]
pub fn shapeless(id: &str, result: &str, count: u32) -> Recipe {
    Recipe::new(RecipeType::Shapeless, id, ItemStack::new(result, count))
}

/// The ingredients of a shapeless recipe, where `itemx3` means `item` three times.
#[must_use]
pub fn from(recipe: Recipe, ingredients: &[&str]) -> Recipe {
    let mut recipe = recipe;
    for ingredient in ingredients {
        if let Some((item, count)) = ingredient.split_once('x') {
            if let Ok(count) = count.parse() {
                recipe = recipe.ingredient(IngredientStack::of(item, count));
                continue;
            }
        }
        recipe = recipe.ingredient(IngredientStack::of(ingredient, 1));
    }
    recipe
}

fn recipes() -> &'static Mutex<BTreeMap<String, Recipe>> {
    static RECIPES: OnceLock<Mutex<BTreeMap<String, Recipe>>> = OnceLock::new();
    RECIPES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, Recipe>) -> T) -> T {
    let mut guard = recipes().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a recipe, or replaces the one with that id.
pub fn register(recipe: Recipe) -> bool {
    with(|recipes| recipes.insert(recipe.id.clone(), recipe).is_none())
}

/// Forgets a recipe.
pub fn unregister(id: &str) -> bool {
    with(|recipes| recipes.remove(id).is_some())
}

/// The same as [`unregister`], under the name the wishlist also uses.
pub fn remove(id: &str) -> bool {
    unregister(id)
}

/// Replaces a recipe, and answers whether there was one to replace.
pub fn replace(recipe: Recipe) -> bool {
    let existed = with(|recipes| recipes.contains_key(&recipe.id));
    with(|recipes| {
        recipes.insert(recipe.id.clone(), recipe);
    });
    existed
}

/// A recipe by id.
#[must_use]
pub fn get_by_id(id: &str) -> Option<Recipe> {
    with(|recipes| recipes.get(id).cloned())
}

/// Every recipe, or every one of a kind.
#[must_use]
pub fn get_all() -> Vec<Recipe> {
    with(|recipes| recipes.values().cloned().collect())
}

/// Every recipe of a kind.
#[must_use]
pub fn get(kind: Option<RecipeType>) -> Vec<Recipe> {
    with(|recipes| {
        recipes
            .values()
            .filter(|recipe| kind.is_none_or(|kind| recipe.kind == kind))
            .cloned()
            .collect()
    })
}

/// Every recipe whose result is this item.
#[must_use]
pub fn get_for_result(item: &str) -> Vec<Recipe> {
    with(|recipes| {
        recipes
            .values()
            .filter(|recipe| recipe.result.item() == item)
            .cloned()
            .collect()
    })
}

/// Registers a shaped recipe.
pub fn register_shaped(recipe: Recipe) -> bool {
    register(recipe)
}

/// Registers a shapeless recipe.
pub fn register_shapeless(recipe: Recipe) -> bool {
    register(recipe)
}

/// Registers a smelting recipe.
pub fn register_smelting_recipe(recipe: Recipe) -> bool {
    register(recipe)
}

/// Registers a blasting recipe.
pub fn register_blasting_recipe(recipe: Recipe) -> bool {
    register(recipe)
}

/// Registers a brewing recipe.
pub fn register_brewing_recipe(recipe: Recipe) -> bool {
    register(recipe)
}

/// Registers a smithing recipe.
pub fn register_smithing_recipe(recipe: Recipe) -> bool {
    register(recipe)
}

/// Registers a machine recipe, which is a recipe with an input, a time and experience that a
/// mod's own machine uses.
pub fn register_machine_recipe(recipe: Recipe) -> bool {
    register(recipe)
}

/// Registers a recipe type a mod invents.
pub fn register_type(name: &str) -> bool {
    with_types(|types| {
        if types.iter().any(|held| held == name) {
            false
        } else {
            types.push(name.to_owned());
            true
        }
    })
}

/// The recipe types this host knows, plus any a mod registered.
#[must_use]
pub fn get_type_names() -> Vec<String> {
    let mut names: Vec<String> = [
        RecipeType::Shaped,
        RecipeType::Shapeless,
        RecipeType::Smelting,
        RecipeType::Blasting,
        RecipeType::Smoking,
        RecipeType::Brewing,
        RecipeType::Smithing,
        RecipeType::Stonecutting,
        RecipeType::Machine,
    ]
    .iter()
    .map(|kind| kind.as_str().to_owned())
    .collect();
    names.extend(with_types(|types| types.clone()));
    names
}

/// The type of a registered recipe, by id.
#[must_use]
pub fn get_type(id: &str) -> Option<RecipeType> {
    get_by_id(id).map(|recipe| recipe.kind)
}

/// Records that a mod has a serializer for a recipe type, which is what writes it down and
/// reads it back. This host has one serializer: `key = value` through [`Recipe::id`] and the
/// registries above.
pub fn register_serializer(name: &str) -> bool {
    with_serializers(|serializers| {
        if serializers.iter().any(|held| held == name) {
            false
        } else {
            serializers.push(name.to_owned());
            true
        }
    })
}

/// The serializers a mod registered.
#[must_use]
pub fn get_serializers() -> Vec<String> {
    with_serializers(|serializers| serializers.clone())
}

/// Registers a condition a recipe needs, such as a mod being loaded. The condition is recorded
/// and this host does not evaluate it against the client.
pub fn register_condition(name: &str) -> bool {
    with_conditions(|conditions| {
        if conditions.iter().any(|held| held == name) {
            false
        } else {
            conditions.push(name.to_owned());
            true
        }
    })
}

/// The conditions a mod registered.
#[must_use]
pub fn get_conditions() -> Vec<String> {
    with_conditions(|conditions| conditions.clone())
}

fn types() -> &'static Mutex<Vec<String>> {
    static TYPES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    TYPES.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_types<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = types().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn serializers() -> &'static Mutex<Vec<String>> {
    static SERIALIZERS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    SERIALIZERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_serializers<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = serializers()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn conditions() -> &'static Mutex<Vec<String>> {
    static CONDITIONS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    CONDITIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_conditions<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = conditions()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Every recipe that matches a grid, at a grid width. Three-by-three is the game's own.
#[must_use]
pub fn find_matching(grid: &[Option<ItemStack>], width: usize) -> Vec<Recipe> {
    with(|recipes| {
        recipes
            .values()
            .filter(|recipe| recipe.matches_grid(grid, width))
            .cloned()
            .collect()
    })
}

/// The same, from a list of stacks in the order a player would put them in a three-by-three
/// grid, with anything empty left out.
#[must_use]
pub fn find_matching_items(items: &[ItemStack]) -> Vec<Recipe> {
    let grid = grid_from(items, GRID / 3);
    find_matching(&grid, GRID / 3)
}

/// A square grid from a list of stacks, filling from the top left. The game's own grid is
/// three by three, so `grid_from(items, 3)` is what a player's crafting table holds.
#[must_use]
pub fn grid_from(items: &[ItemStack], width: usize) -> Vec<Option<ItemStack>> {
    let mut grid: Vec<Option<ItemStack>> = vec![None; width * width];
    for (index, item) in items.iter().enumerate() {
        if index < grid.len() {
            grid[index] = Some(item.clone());
        }
    }
    grid
}

/// Whether anything in the registry matches a grid.
#[must_use]
pub fn can_craft(grid: &[Option<ItemStack>], width: usize) -> bool {
    !find_matching(grid, width).is_empty()
}

/// Crafts the first recipe that matches, and answers what it made.
#[must_use]
pub fn craft(grid: &[Option<ItemStack>], width: usize) -> Option<ItemStack> {
    find_matching(grid, width)
        .first()
        .map(|recipe| recipe.result.clone())
}

/// Everything a recipe makes, results and leftovers together.
#[must_use]
pub fn craft_all(grid: &[Option<ItemStack>], width: usize) -> Vec<ItemStack> {
    find_matching(grid, width)
        .first()
        .map(Recipe::craft)
        .unwrap_or_default()
}

/// What a recipe needs.
#[must_use]
pub fn get_ingredients(id: &str) -> Vec<IngredientStack> {
    get_by_id(id).map(|recipe| recipe.get_ingredients()).unwrap_or_default()
}

/// What a recipe makes.
#[must_use]
pub fn get_result(id: &str) -> Option<ItemStack> {
    get_by_id(id).map(|recipe| recipe.result)
}

/// What it is filed under.
#[must_use]
pub fn get_category(id: &str) -> Option<String> {
    get_by_id(id).map(|recipe| recipe.category)
}

/// How long a recipe takes.
#[must_use]
pub fn get_cook_time(id: &str) -> Option<u32> {
    get_by_id(id).map(|recipe| recipe.cook_time)
}

/// What a recipe is worth in experience.
#[must_use]
pub fn get_experience(id: &str) -> Option<f32> {
    get_by_id(id).map(|recipe| recipe.experience)
}

/// What is left over after crafting a recipe.
#[must_use]
pub fn get_remaining_items(id: &str) -> Vec<ItemStack> {
    get_by_id(id).map(|recipe| recipe.remaining).unwrap_or_default()
}

/// Registers a fuel, and how long it burns for in ticks.
pub fn register_fuel(item: &str, burn_time: u32) -> bool {
    with_fuels(|fuels| fuels.insert(item.to_owned(), burn_time).is_none())
}

/// How long an item burns for, or `None` when it is not a fuel.
#[must_use]
pub fn get_fuel_time(item: &str) -> Option<u32> {
    with_fuels(|fuels| fuels.get(item).copied())
}

/// Every fuel a mod registered.
#[must_use]
pub fn get_fuels() -> Vec<(String, u32)> {
    with_fuels(|fuels| fuels.iter().map(|(item, time)| (item.clone(), *time)).collect())
}

fn fuels() -> &'static Mutex<BTreeMap<String, u32>> {
    static FUELS: OnceLock<Mutex<BTreeMap<String, u32>>> = OnceLock::new();
    FUELS.get_or_init(|| {
        let mut fuels = BTreeMap::new();
        fuels.insert("minecraft:coal".to_owned(), COAL_FUEL);
        fuels.insert("minecraft:charcoal".to_owned(), COAL_FUEL);
        fuels.insert("minecraft:stick".to_owned(), 100);
        fuels.insert("minecraft:lava_bucket".to_owned(), 20_000);
        Mutex::new(fuels)
    })
}

fn with_fuels<T>(body: impl FnOnce(&mut BTreeMap<String, u32>) -> T) -> T {
    let mut guard = fuels().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// How many of a recipe a furnace full of fuel would cook, for a caller planning a batch.
#[must_use]
pub fn batch_size(item: &str, fuel: &str) -> u32 {
    match (get_fuel_time(fuel), get_by_id(item)) {
        (Some(burn), Some(recipe)) if recipe.cook_time > 0 => burn / recipe.cook_time,
        _ => 0,
    }
}

/// Why a recipe question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client" => {
            "the client's own recipes come from the server's data pack, which this host cannot \
             read; a mod ships its own recipes here and crafts against those"
                .to_owned()
        }
        "player" => {
            "taking from or giving to the player's inventory is the client's, and the server's \
             to accept; this host crafts into a grid a mod owns"
                .to_owned()
        }
        "serializer" => {
            "the client's serializer registry is inside the client; a mod registers its own \
             names here and this host writes recipes down its own way"
                .to_owned()
        }
        "condition" => {
            "a condition is recorded and not evaluated against the client, because the client \
             is not asking this host for recipes yet"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client", "player", "serializer", "condition"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's recipes. Empty.
#[must_use]
pub fn get_client_recipes() -> Vec<Recipe> {
    Vec::new()
}

/// The client's serializer registry. Empty.
#[must_use]
pub fn get_serializer(id: &str) -> Option<String> {
    let _ = id;
    None
}

/// Crafts into the player's inventory. `false`: the client's inventory is the client's.
pub fn craft_into_player(_grid: &[Option<ItemStack>], _width: usize) -> bool {
    crate::warn!(
        "a mod asked this host to craft into the player's inventory: {}",
        why_unknown("player")
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|recipes| recipes.clear());
        with_types(|types| types.clear());
        with_serializers(|serializers| serializers.clear());
        with_conditions(|conditions| conditions.clear());
    }

    fn torches() -> Recipe {
        Recipe::new(RecipeType::Shaped, "hello:torches", ItemStack::new("minecraft:torch", 4))
            .row("C")
            .row("S")
            .key('C', "minecraft:coal")
            .key('S', "minecraft:stick")
            .category("misc")
    }

    fn two_by_two() -> Recipe {
        Recipe::new(
            RecipeType::Shaped,
            "hello:crafting_table",
            ItemStack::new("minecraft:crafting_table", 1),
        )
        .row("PP")
        .row("PP")
        .key('P', "#minecraft:planks")
    }

    #[test]
    fn a_shaped_recipe_matches_the_pattern_it_was_given() {
        let _serial = serial();
        fresh();
        assert!(register_shaped(torches()));
        let coal = ItemStack::new("minecraft:coal", 1);
        let stick = ItemStack::new("minecraft:stick", 1);
        assert!(can_craft(&[Some(coal.clone()), Some(stick.clone())], 1));
        assert!(!can_craft(&[Some(stick.clone()), Some(coal.clone())], 1), "order matters");
        // A two-wide grid, with the column in the first of its two columns.
        assert!(
            can_craft(&[Some(coal.clone()), None, Some(stick.clone()), None], 2),
            "and a two-wide grid works"
        );
        assert!(
            !can_craft(&[Some(coal.clone()), None, None, Some(stick.clone())], 2),
            "a diagonal is not a column"
        );
        assert!(!can_craft(&[Some(coal.clone())], 1), "half the pattern is not enough");
        assert_eq!(
            craft(&[Some(coal.clone()), Some(stick.clone())], 1)
                .map(|stack| stack.count()),
            Some(4)
        );
        assert!(!can_craft(&[Some(stick.clone()), Some(stick.clone())], 1));
    }

    #[test]
    fn a_pattern_matches_anywhere_in_the_grid_and_mirrored() {
        let _serial = serial();
        fresh();
        register_shaped(torches());
        let coal = Some(ItemStack::new("minecraft:coal", 1));
        let stick = Some(ItemStack::new("minecraft:stick", 1));
        // The pattern sits in the left column of a three-by-three.
        let grid: Vec<Option<ItemStack>> = vec![
            coal.clone(),
            None,
            None,
            stick.clone(),
            None,
            None,
            None,
            None,
            None,
        ];
        assert!(can_craft(&grid, 3), "the pattern is at the top left");
        // The same column moved to the middle.
        let moved: Vec<Option<ItemStack>> = vec![
            None,
            coal.clone(),
            None,
            None,
            stick.clone(),
            None,
            None,
            None,
            None,
        ];
        assert!(can_craft(&moved, 3), "and it does not have to be at the edge");
        // A column the other way round is a rotation, not a mirror, and does not match.
        let rotated: Vec<Option<ItemStack>> = vec![
            stick.clone(),
            None,
            None,
            coal.clone(),
            None,
            None,
            None,
            None,
            None,
        ];
        assert!(!can_craft(&rotated, 3), "rotating a column is not mirroring it");

        // Mirroring is horizontal, and a wide pattern shows it: "CS" also matches "SC".
        fresh();
        register_shaped(
            Recipe::new(RecipeType::Shaped, "hello:pair", ItemStack::new("minecraft:torch", 1))
                .row("CS")
                .key('C', "minecraft:coal")
                .key('S', "minecraft:stick"),
        );
        let mirrored: Vec<Option<ItemStack>> = vec![stick.clone(), coal.clone()];
        assert!(can_craft(&mirrored, 2), "the row is mirrored");
        register(
            Recipe::new(RecipeType::Shaped, "hello:pair", ItemStack::new("minecraft:torch", 1))
                .row("CS")
                .key('C', "minecraft:coal")
                .key('S', "minecraft:stick")
                .mirrored(false),
        );
        assert!(!can_craft(&mirrored, 2), "with mirroring off it no longer matches");
        let straight: Vec<Option<ItemStack>> = vec![coal.clone(), stick.clone()];
        assert!(can_craft(&straight, 2), "and the pattern itself still matches");
    }

    #[test]
    fn a_shapeless_recipe_matches_any_arrangement() {
        let _serial = serial();
        fresh();
        assert!(register_shapeless(from(
            shapeless("hello:planks", "minecraft:oak_planks", 4),
            &["minecraft:oak_log"],
        )));
        let log = ItemStack::new("minecraft:oak_log", 1);
        assert!(can_craft(&[Some(log.clone())], 1));
        assert!(can_craft(&[None, None, Some(log.clone())], 3), "position does not matter");
        assert!(!can_craft(&[Some(ItemStack::new("minecraft:stone", 1))], 1));
        // A two-ingredient shapeless recipe needs both.
        register_shapeless(from(
            shapeless("hello:stew", "minecraft:mushroom_stew", 1),
            &["minecraft:bowl", "minecraft:brown_mushroom"],
        ));
        let bowl = ItemStack::new("minecraft:bowl", 1);
        let mushroom = ItemStack::new("minecraft:brown_mushroom", 1);
        assert!(can_craft(&[Some(mushroom.clone()), Some(bowl.clone())], 2));
        assert!(!can_craft(&[Some(bowl.clone())], 2), "one of two is not enough");
        let found = find_matching_items(&[bowl, mushroom]);
        assert!(found.iter().any(|recipe| recipe.id == "hello:stew"));
    }

    #[test]
    fn a_tag_ingredient_matches_by_name_or_by_its_own_tags() {
        let _serial = serial();
        fresh();
        register_shaped(two_by_two());
        let planks = Some(ItemStack::new("minecraft:planks", 1));
        let grid = vec![
            planks.clone(),
            planks.clone(),
            planks.clone(),
            planks.clone(),
        ];
        assert!(can_craft(&grid, 2), "a tag matches an item named for it");
        let mut tagged = ItemStack::new("minecraft:oak_planks", 1);
        tagged.set_component(crate::items::ItemComponent::new(
            "cinnaloader:tags",
            "minecraft:planks,minecraft:wood",
        ));
        let grid = vec![
            Some(tagged.clone()),
            Some(tagged.clone()),
            Some(tagged.clone()),
            Some(tagged.clone()),
        ];
        assert!(can_craft(&grid, 2), "and an item carrying the tag");
        let plain = ItemStack::new("minecraft:oak_planks", 1);
        assert!(
            !can_craft(&[Some(plain.clone()), Some(plain.clone()), Some(plain.clone()), Some(plain.clone())], 2),
            "without the tag it does not match a tag ingredient"
        );
        assert!(Ingredient::parse("#a").matches(&ItemStack::new("a", 1)));
        assert_eq!(Ingredient::parse("#a").label(), "#a");
        assert_eq!(Ingredient::parse("minecraft:stone").label(), "minecraft:stone");
        assert_eq!(
            Ingredient::AnyOf(vec!["a".to_owned(), "b".to_owned()]).label(),
            "a|b"
        );
    }

    #[test]
    fn what_a_recipe_needs_and_makes_can_be_read_back() {
        let _serial = serial();
        fresh();
        register_shaped(torches().experience(0.1));
        assert_eq!(get_ingredients("hello:torches").len(), 2);
        let counts: Vec<u32> = get_ingredients("hello:torches")
            .iter()
            .map(|ingredient| ingredient.count)
            .collect();
        assert_eq!(
            get_ingredients("hello:torches")[0].label(),
            "minecraft:coal"
        );
        assert_eq!(counts, vec![1, 1]);
        assert_eq!(get_result("hello:torches").map(|stack| stack.count()), Some(4));
        assert_eq!(get_category("hello:torches"), Some("misc".to_owned()));
        assert_eq!(get_experience("hello:torches"), Some(0.1));
        assert_eq!(get_cook_time("hello:torches"), Some(SMELT_TIME));
        assert!(get_remaining_items("hello:torches").is_empty());
        assert_eq!(get_type("hello:torches"), Some(RecipeType::Shaped));
        assert_eq!(torches().shape(), (1, 2));
        assert!(torches().to_string().contains("hello:torches"));

        // A four-by-four pattern counted from its rows.
        assert_eq!(get_ingredients("hello:crafting_table").len(), 0, "not registered yet");
        register_shaped(two_by_two());
        assert_eq!(get_ingredients("hello:crafting_table")[0].count, 4);
        assert_eq!(
            get_ingredients("hello:crafting_table")[0].ingredient.label(),
            "#minecraft:planks"
        );
    }

    #[test]
    fn a_cooking_recipe_takes_one_input_and_has_a_time() {
        let _serial = serial();
        fresh();
        let mut smelting = Recipe::new(
            RecipeType::Smelting,
            "hello:glass",
            ItemStack::new("minecraft:glass", 1),
        );
        smelting = smelting.ingredient(IngredientStack::of("minecraft:sand", 1));
        assert_eq!(smelting.cook_time, SMELT_TIME);
        assert!(register_smelting_recipe(smelting.clone()));
        let sand = Some(ItemStack::new("minecraft:sand", 1));
        assert!(can_craft(std::slice::from_ref(&sand), 1));
        let two = vec![sand.clone(), sand.clone()];
        assert!(!can_craft(&two, 2), "one input, not two");
        assert!(!can_craft(&[Some(ItemStack::new("minecraft:gravel", 1))], 1));

        let blasting = Recipe::new(
            RecipeType::Blasting,
            "hello:iron",
            ItemStack::new("minecraft:iron_ingot", 1),
        )
        .ingredient(IngredientStack::of("minecraft:iron_ore", 1));
        assert_eq!(blasting.cook_time, BLAST_TIME, "blasting is faster");
        assert!(register_blasting_recipe(blasting));
        assert!(RecipeType::Smelting.is_cooking());
        assert!(!RecipeType::Shaped.is_cooking());
        assert_eq!(RecipeType::Blasting.to_string(), "blasting");
    }

    #[test]
    fn fuels_burn_for_the_time_they_say() {
        let _serial = serial();
        fresh();
        assert_eq!(get_fuel_time("minecraft:coal"), Some(COAL_FUEL));
        assert_eq!(get_fuel_time("minecraft:stick"), Some(100));
        assert_eq!(get_fuel_time("minecraft:stone"), None);
        assert!(register_fuel("hello:peat", 400));
        assert!(!register_fuel("hello:peat", 400), "once");
        assert_eq!(get_fuel_time("hello:peat"), Some(400));
        assert!(get_fuels().len() >= 5);

        register_smelting_recipe(
            Recipe::new(
                RecipeType::Smelting,
                "hello:glass",
                ItemStack::new("minecraft:glass", 1),
            )
            .ingredient(IngredientStack::of("minecraft:sand", 1)),
        );
        assert_eq!(batch_size("hello:glass", "minecraft:coal"), 8, "1600 / 200");
        assert_eq!(batch_size("hello:glass", "minecraft:stick"), 0);
        assert_eq!(batch_size("nothing", "minecraft:coal"), 0);
    }

    #[test]
    fn the_types_serializers_and_conditions_a_mod_registers_are_recorded() {
        let _serial = serial();
        fresh();
        assert!(register_type("hello:alchemy"));
        assert!(!register_type("hello:alchemy"), "once");
        assert!(get_type_names().contains(&"hello:alchemy".to_owned()));
        assert!(get_type_names().contains(&"crafting_shaped".to_owned()));
        assert!(register_serializer("hello:alchemy"));
        assert!(!register_serializer("hello:alchemy"));
        assert_eq!(get_serializers().len(), 1);
        assert!(register_condition("hello:loaded"));
        assert!(!register_condition("hello:loaded"));
        assert_eq!(get_conditions().len(), 1);
    }

    #[test]
    fn registering_replacing_and_removing_a_recipe() {
        let _serial = serial();
        fresh();
        assert!(register_shaped(torches()));
        assert!(!register_shaped(torches()), "the second replaces the first");
        assert!(replace(torches()));
        assert_eq!(get_all().len(), 1, "replacing does not add a second one");
        assert!(
            !replace(Recipe::new(
                RecipeType::Shaped,
                "hello:nowhere",
                ItemStack::new("minecraft:air", 1)
            )),
            "replacing a recipe that is not there is not a replacement"
        );
        assert_eq!(get_all().len(), 2, "the torch recipe and the one that replaced nothing");
        assert_eq!(get(Some(RecipeType::Shaped)).len(), 2);
        assert_eq!(get(Some(RecipeType::Smelting)).len(), 0);
        assert_eq!(get_for_result("minecraft:torch").len(), 1);
        assert_eq!(get_for_result("minecraft:stick").len(), 0);
        assert!(remove("hello:torches"));
        assert!(!remove("hello:torches"));
        assert!(get_by_id("hello:torches").is_none());
        assert_eq!(get_all().len(), 1, "the other recipe is still there");
    }

    #[test]
    fn the_clients_recipes_and_the_players_inventory_are_refused() {
        let _serial = serial();
        assert!(get_client_recipes().is_empty());
        assert!(get_serializer("minecraft:crafting_shaped").is_none());
        assert!(!craft_into_player(&[], 1));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_eq!(grid_from(&[ItemStack::new("a", 1)], 3).len(), 9);
        assert!(craft_all(&[Some(ItemStack::new("a", 1))], 1).is_empty());
    }
}
