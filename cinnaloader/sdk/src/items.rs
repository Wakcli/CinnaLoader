//! Items: the stacks a mod can build here, and the item registry the client owns.
//!
//! A **stack** is a value, and this section is real where it concerns one: an identifier, a
//! count, an aux value, damage, a custom name, lore, enchantments and components, with the
//! operations a mod needs — create, clone, copy, split, merge, count, name, lore, damage.
//! All of it happens in this process, on data the mod already has.
//!
//! What is not reachable is the client's **item registry** — what an identifier means, its
//! max stack, its durability, its tags, its icon — and the live stack in the player's hand.
//! Those answer `None` with the reason in [`why_unknown`]. A mod may still *declare* an item
//! type, exactly as it declares a block; the declaration is real data, and
//! [`is_registered_with_the_client`] is honest that the client has not been told.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let mut stack = ItemStack::new("minecraft:stone", 16);
//! assert_eq!(stack.split(4).map(|split| split.count()), Some(4));
//! assert_eq!(stack.count(), 12);
//! stack.set_name("Cobble");
//! assert_eq!(stack.name(), Some("Cobble"));
//! assert!(items::get_stack().is_none(), "the hand is the client's");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events::{self, kind as event_kind};

/// Why an item question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "registry" => {
            "item definitions are the client's own registry (session::items); reading them \
             needs a hook this host does not have"
                .to_owned()
        }
        "hand" | "stack" => {
            "the stack in the player's hand is the client's player state, which needs the \
             hooks section 10 lists as never"
                .to_owned()
        }
        "icon" => "an item's icon is the client's asset, from its own item definitions".to_owned(),
        "model" => {
            "a Bedrock item draws an icon rather than a model graph; there is no model to \
             return, and the Forge model this name comes from has no counterpart"
                .to_owned()
        }
        "components" => {
            "item components are decoded by the client from the wire; this host cannot build \
             one the client would honour"
                .to_owned()
        }
        "enchantments" => "enchantment definitions are the client's registry data".to_owned(),
        "tags" => "item tags come from the client's registry, from the server's data".to_owned(),
        "events" => {
            "item events come from the client as it runs; nothing here sees them, so they are \
             registered for the day the hooks land"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The item events this host would have to receive from the client to fire.
pub const ITEM_EVENTS: [&str; 19] = [
    "item_use",
    "item_use_finish",
    "item_use_stop",
    "item_use_tick",
    "item_click",
    "item_craft",
    "item_smelt",
    "item_consume",
    "item_equip",
    "item_unequip",
    "item_damage",
    "item_break",
    "item_repair",
    "item_enchant",
    "item_drop",
    "item_pickup",
    "item_merge",
    "item_split",
    "item_component_change",
];

/// Teaches the event bus each item event name, for a mod that wants one of them.
pub fn register_event_kinds() -> usize {
    ITEM_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// One thing an item stack carries beyond its identifier and count, as Bedrock spells it:
/// `minecraft:damage`, `minecraft:custom_name`, and so on. `items::ItemComponent` is the same
/// type, because `Component` is a name several sections want.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    /// The value as text: a component is the client's to decode.
    pub value: String,
}

impl Component {
    #[must_use]
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }

    /// The damage component, which is what a tool's wear and tear is.
    #[must_use]
    pub fn damage(damage: i32) -> Self {
        Self::new("minecraft:damage", &damage.to_string())
    }

    /// The custom-name component.
    #[must_use]
    pub fn custom_name(name: &str) -> Self {
        Self::new("minecraft:custom_name", name)
    }

    /// The lore component, lines joined the way the client writes them.
    #[must_use]
    pub fn lore(lines: &[&str]) -> Self {
        Self::new("minecraft:lore", &lines.join("\n"))
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self.value.as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        self.value.parse().ok()
    }
}

/// A stack of items: a value this host owns, and the closest thing here to the client's own
/// `item-stack` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemStack {
    item: String,
    count: u32,
    /// The legacy data value, for the items that still have one.
    aux: i32,
    max_count: u32,
    components: BTreeMap<String, Component>,
    enchantments: BTreeMap<String, i32>,
    /// Whether the stack carries nothing beyond its identifier and count.
    plain: bool,
}

/// How many of one item fit in a stack when a mod does not say.
pub const DEFAULT_MAX_COUNT: u32 = 64;

impl ItemStack {
    /// A stack of `count` of `item`, with the default maximum.
    #[must_use]
    pub fn new(item: &str, count: u32) -> Self {
        Self {
            item: item.to_owned(),
            count,
            aux: 0,
            max_count: DEFAULT_MAX_COUNT,
            components: BTreeMap::new(),
            enchantments: BTreeMap::new(),
            plain: true,
        }
    }

    /// The same, with a maximum count, which is what an item definition would say.
    #[must_use]
    pub fn with_max_count(mut self, max_count: u32) -> Self {
        self.max_count = max_count.max(1);
        self
    }

    /// The same, with an aux value.
    #[must_use]
    pub fn with_aux(mut self, aux: i32) -> Self {
        self.aux = aux;
        self
    }

    /// The same as [`ItemStack::clone`], under the name the wishlist uses.
    #[must_use]
    pub fn copy(&self) -> Self {
        self.clone()
    }

    /// The same as [`ItemStack::copy`]: three names for one operation in the wishlist.
    #[must_use]
    pub fn cloned(&self) -> Self {
        self.clone()
    }

    #[must_use]
    pub fn item(&self) -> &str {
        &self.item
    }

    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    #[must_use]
    pub fn aux(&self) -> i32 {
        self.aux
    }

    #[must_use]
    pub fn max_count(&self) -> u32 {
        self.max_count
    }

    /// Whether it carries nothing beyond its identifier and count, which is what the client
    /// calls a plain stack.
    #[must_use]
    pub fn is_plain(&self) -> bool {
        self.plain
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether more than one fits in a stack.
    #[must_use]
    pub fn is_stackable(&self) -> bool {
        self.max_count > 1
    }

    /// Whether it takes damage: it has a damage component, or it was given a durability.
    #[must_use]
    pub fn is_damageable(&self) -> bool {
        self.damage().is_some() || self.components.contains_key("minecraft:durability")
    }

    /// Whether it is enchanted.
    #[must_use]
    pub fn is_enchanted(&self) -> bool {
        !self.enchantments.is_empty()
    }

    /// Sets the count, and answers the stack.
    pub fn set_count(&mut self, count: u32) -> &mut Self {
        self.count = count.min(self.max_count.max(count));
        self
    }

    /// The stack with another count.
    #[must_use]
    pub fn with_count(mut self, count: u32) -> Self {
        let clamped = count.min(self.max_count);
        self.count = clamped;
        self
    }

    /// Takes `count` off the top as a new stack, or `None` when there are not that many.
    pub fn split(&mut self, count: u32) -> Option<Self> {
        if count == 0 || count > self.count {
            return None;
        }
        self.count -= count;
        let mut split = self.clone();
        split.count = count;
        Some(split)
    }

    /// Merges another stack into this one as far as it fits, and answers how many were left.
    pub fn merge(&mut self, other: &Self) -> u32 {
        if other.item != self.item || self.max_count <= 1 {
            return other.count;
        }
        let room = self.max_count.saturating_sub(self.count);
        let moved = room.min(other.count);
        self.count += moved;
        if moved > 0 {
            self.plain = false;
        }
        other.count - moved
    }

    /// Merges what fits and hands back whatever did not, as a stack or nothing.
    #[must_use]
    pub fn merge_into(mut self, other: &Self) -> (Self, Option<Self>) {
        let left = self.merge(other);
        let rest = (left > 0).then(|| {
            let mut overflow = other.clone();
            overflow.count = left;
            overflow
        });
        (self, rest)
    }

    /// The damage a stack has taken.
    #[must_use]
    pub fn damage(&self) -> Option<i32> {
        self.components
            .get("minecraft:damage")
            .and_then(Component::as_integer)
            .map(|damage| damage as i32)
    }

    /// Sets the damage, which is what wearing a tool down does.
    pub fn set_damage(&mut self, damage: i32) -> &mut Self {
        self.set_component(Component::damage(damage.max(0)));
        self
    }

    /// How much damage it can take before it breaks, when the mod declared it.
    #[must_use]
    pub fn max_damage(&self) -> Option<i32> {
        self.components
            .get("minecraft:durability")
            .and_then(Component::as_integer)
            .map(|durability| durability as i32)
    }

    /// How much use is left, when it can be damaged at all.
    #[must_use]
    pub fn durability(&self) -> Option<i32> {
        let max = self.max_damage()?;
        Some((max - self.damage().unwrap_or(0)).max(0))
    }

    /// The custom name, if it was given one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.components
            .get("minecraft:custom_name")
            .map(|component| component.value.as_str())
    }

    /// Gives it a name.
    pub fn set_name(&mut self, name: &str) -> &mut Self {
        self.plain = false;
        self.components
            .insert("minecraft:custom_name".to_owned(), Component::custom_name(name));
        self
    }

    /// The name to show: the custom one, or the identifier.
    #[must_use]
    pub fn display_name(&self) -> String {
        self.name().map(str::to_owned).unwrap_or_else(|| self.item.clone())
    }

    /// The lore lines.
    #[must_use]
    pub fn lore(&self) -> Vec<String> {
        self.components
            .get("minecraft:lore")
            .map(|component| component.value.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// Sets the lore.
    pub fn set_lore(&mut self, lines: &[&str]) -> &mut Self {
        self.plain = lines.is_empty();
        self.components
            .insert("minecraft:lore".to_owned(), Component::lore(lines));
        self
    }

    /// Every component it carries.
    #[must_use]
    pub fn components(&self) -> Vec<Component> {
        self.components.values().cloned().collect()
    }

    /// One component by name.
    #[must_use]
    pub fn component(&self, name: &str) -> Option<Component> {
        self.components.get(name).cloned()
    }

    /// Adds or replaces a component, and answers the stack.
    pub fn set_component(&mut self, component: Component) -> &mut Self {
        self.plain = false;
        self.components.insert(component.name.clone(), component);
        self
    }

    /// Removes a component.
    pub fn remove_component(&mut self, name: &str) -> bool {
        self.components.remove(name).is_some()
    }

    /// The enchantments, by name.
    #[must_use]
    pub fn enchantments(&self) -> BTreeMap<String, i32> {
        self.enchantments.clone()
    }

    /// Adds an enchantment at a level.
    pub fn add_enchantment(&mut self, name: &str, level: i32) -> &mut Self {
        self.plain = false;
        self.enchantments.insert(name.to_owned(), level);
        self
    }

    /// Removes an enchantment, and answers whether it was there.
    pub fn remove_enchantment(&mut self, name: &str) -> bool {
        self.enchantments.remove(name).is_some()
    }
}

impl fmt::Display for ItemStack {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}x {} ({})",
            self.count,
            self.display_name(),
            self.item
        )
    }
}

/// What a mod declares an item to be, which the client cannot be told about here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemType {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub max_count: u32,
    pub durability: Option<i32>,
    /// The icon name a mod ships; the client's own icon is not reachable.
    pub icon: String,
    pub tags: Vec<String>,
    pub components: Vec<Component>,
    /// How it is used, such as `eat` or `drink`, and for how long.
    pub use_action: String,
    pub use_duration: u32,
    pub cooldown: u32,
    /// The item components a mod calls its food, armour or tool grouping. The grouping is
    /// Forge's; Bedrock carries the same data as components.
    pub food: Vec<Component>,
    pub armour: Vec<Component>,
    pub tool: Vec<Component>,
    pub registered: bool,
}

impl ItemType {
    /// An item with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            max_count: DEFAULT_MAX_COUNT,
            durability: None,
            icon: String::new(),
            tags: Vec::new(),
            components: Vec::new(),
            use_action: String::new(),
            use_duration: 0,
            cooldown: 0,
            food: Vec::new(),
            armour: Vec::new(),
            tool: Vec::new(),
            registered: false,
        }
    }

    #[must_use]
    pub fn max_count(mut self, max_count: u32) -> Self {
        self.max_count = max_count.max(1);
        self
    }

    #[must_use]
    pub fn durability(mut self, durability: i32) -> Self {
        self.durability = Some(durability);
        self
    }

    #[must_use]
    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = icon.to_owned();
        self
    }

    #[must_use]
    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_owned());
        self
    }

    #[must_use]
    pub fn component(mut self, component: Component) -> Self {
        self.components.retain(|held| held.name != component.name);
        self.components.push(component);
        self
    }

    #[must_use]
    pub fn use_action(mut self, action: &str, duration: u32) -> Self {
        self.use_action = action.to_owned();
        self.use_duration = duration;
        self
    }

    #[must_use]
    pub fn cooldown(mut self, ticks: u32) -> Self {
        self.cooldown = ticks;
        self
    }

    #[must_use]
    pub fn food(mut self, components: &[Component]) -> Self {
        self.food = components.to_vec();
        self
    }

    #[must_use]
    pub fn armour(mut self, components: &[Component]) -> Self {
        self.armour = components.to_vec();
        self
    }

    #[must_use]
    pub fn tool(mut self, components: &[Component]) -> Self {
        self.tool = components.to_vec();
        self
    }

    /// A stack of this item, with its own maximum count and durability.
    #[must_use]
    pub fn stack(&self, count: u32) -> ItemStack {
        let mut stack = ItemStack::new(&self.id, count).with_max_count(self.max_count);
        if let Some(durability) = self.durability {
            stack.set_component(Component::new("minecraft:durability", &durability.to_string()));
        }
        for component in &self.components {
            stack.set_component(component.clone());
        }
        stack
    }

    #[must_use]
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|held| held == tag)
    }
}

impl fmt::Display for ItemType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, max {}, {})",
            self.name,
            self.id,
            self.max_count,
            if self.durability.is_some() {
                "damageable"
            } else {
                "not damageable"
            }
        )
    }
}

/// The same type as [`Component`], under a name that cannot collide when several section
/// modules are in scope at once.
pub type ItemComponent = Component;

fn registry() -> &'static Mutex<BTreeMap<String, ItemType>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, ItemType>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, ItemType>) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares an item type. The declaration is real data; the client is not told, and
/// [`is_registered_with_the_client`] says so.
pub fn register(item: ItemType) -> bool {
    crate::warn!(
        "{} declared the item {}: this host records it, but {}",
        crate::metadata::id(),
        item.id,
        why_unknown("registry")
    );
    with(|registry| registry.insert(item.id.clone(), item).is_none())
}

/// Forgets a declared item type.
pub fn unregister(id: &str) -> bool {
    with(|registry| registry.remove(id).is_some())
}

/// Declares a component on an item that is already declared.
pub fn register_component(id: &str, component: Component) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(item) => {
            item.components.retain(|held| held.name != component.name);
            item.components.push(component);
            true
        }
        None => false,
    })
}

/// A declared item type, by id. Everything the client's own registry would answer for is
/// `None` here, with the reason in [`why_unknown`].
#[must_use]
pub fn get(id: &str) -> Option<ItemType> {
    with(|registry| registry.get(id).cloned())
}

/// The same, by the name a mod gave it.
#[must_use]
pub fn get_by_name(name: &str) -> Option<ItemType> {
    with(|registry| {
        registry
            .values()
            .find(|item| item.name == name)
            .cloned()
    })
}

/// Every declared item type, in id order.
#[must_use]
pub fn get_all() -> Vec<ItemType> {
    with(|registry| registry.values().cloned().collect())
}

/// A declared item's id.
#[must_use]
pub fn get_id_of(id: &str) -> Option<String> {
    get(id).map(|item| item.id)
}

/// A declared item's registry id, with the owning mod in front.
#[must_use]
pub fn get_registry_id(id: &str) -> Option<String> {
    get(id).map(|item| format!("{}:{}", item.owner, item.id))
}

/// A declared item's maximum stack size.
#[must_use]
pub fn get_max_stack_size(id: &str) -> Option<u32> {
    get(id).map(|item| item.max_count)
}

/// A declared item's durability.
#[must_use]
pub fn get_durability(id: &str) -> Option<i32> {
    get(id).and_then(|item| item.durability)
}

/// A declared item's components.
#[must_use]
pub fn get_components(id: &str) -> Vec<Component> {
    get(id).map(|item| item.components).unwrap_or_default()
}

/// One component of a declared item.
#[must_use]
pub fn get_component(id: &str, name: &str) -> Option<Component> {
    get(id).and_then(|item| {
        item.components
            .into_iter()
            .find(|component| component.name == name)
    })
}

/// A declared item's tags.
#[must_use]
pub fn get_tags(id: &str) -> Vec<String> {
    get(id).map(|item| item.tags).unwrap_or_default()
}

/// Whether a declared item has a tag.
#[must_use]
pub fn has_tag(id: &str, tag: &str) -> bool {
    get(id).is_some_and(|item| item.has_tag(tag))
}

/// Whether a declared item can be damaged.
#[must_use]
pub fn is_damageable(id: &str) -> Option<bool> {
    get(id).map(|item| item.durability.is_some())
}

/// Whether a declared item stacks.
#[must_use]
pub fn is_stackable(id: &str) -> Option<bool> {
    get(id).map(|item| item.max_count > 1)
}

/// Whether a declared item can be eaten.
#[must_use]
pub fn is_edible(id: &str) -> Option<bool> {
    get(id).map(|item| !item.food.is_empty() || item.use_action == "eat")
}

/// Whether a declared item can be equipped.
#[must_use]
pub fn is_equippable(id: &str) -> Option<bool> {
    get(id).map(|item| !item.armour.is_empty())
}

/// A declared item's food components. The grouping is Forge's; Bedrock carries the same data
/// as components.
#[must_use]
pub fn get_food_properties(id: &str) -> Vec<Component> {
    get(id).map(|item| item.food).unwrap_or_default()
}

/// A declared item's armour components.
#[must_use]
pub fn get_armour_properties(id: &str) -> Vec<Component> {
    get(id).map(|item| item.armour).unwrap_or_default()
}

/// A declared item's tool components.
#[must_use]
pub fn get_tool_properties(id: &str) -> Vec<Component> {
    get(id).map(|item| item.tool).unwrap_or_default()
}

/// How a declared item is used.
#[must_use]
pub fn get_use_action(id: &str) -> Option<String> {
    get(id).map(|item| item.use_action)
}

/// How long using it takes.
#[must_use]
pub fn get_use_duration(id: &str) -> Option<u32> {
    get(id).map(|item| item.use_duration)
}

/// How long its cooldown is.
#[must_use]
pub fn get_cooldown(id: &str) -> Option<u32> {
    get(id).map(|item| item.cooldown)
}

/// The icon name a declared item ships. The client's own icon is not reachable.
#[must_use]
pub fn get_texture(id: &str) -> Option<String> {
    get(id).map(|item| item.icon)
}

/// Whether the client has been told about a declared item, which it never has.
#[must_use]
pub fn is_registered_with_the_client(id: &str) -> bool {
    get(id).is_some_and(|item| item.registered)
}

/// There is no model graph to return: a Bedrock item draws an icon, and the Forge model this
/// name comes from has no counterpart. `None`, always, with that reason.
#[must_use]
pub fn get_model(_id: &str) -> Option<()> {
    None
}

/// The stack in the player's hand, which is the client's player state.
#[must_use]
pub fn get_stack() -> Option<ItemStack> {
    None
}

/// A stack built from an identifier and a count, for a caller that does not have a
/// declaration.
#[must_use]
pub fn create_stack(item: &str, count: u32) -> ItemStack {
    ItemStack::new(item, count)
}

/// The same as [`ItemStack::clone`].
#[must_use]
pub fn clone_stack(stack: &ItemStack) -> ItemStack {
    stack.clone()
}

/// The same as [`ItemStack::copy`].
#[must_use]
pub fn copy_stack(stack: &ItemStack) -> ItemStack {
    stack.copy()
}

/// Takes `count` off the top of a stack, and answers the new one.
pub fn split_stack(stack: &mut ItemStack, count: u32) -> Option<ItemStack> {
    stack.split(count)
}

/// Merges one stack into another, and answers how many were left over.
pub fn merge_stacks(into: &mut ItemStack, from: &ItemStack) -> u32 {
    into.merge(from)
}

/// Sets a stack's count.
pub fn set_count(stack: &mut ItemStack, count: u32) {
    stack.set_count(count);
}

/// Sets a stack's damage.
pub fn set_damage(stack: &mut ItemStack, damage: i32) {
    stack.set_damage(damage);
}

/// A stack's damage.
#[must_use]
pub fn get_damage(stack: &ItemStack) -> Option<i32> {
    stack.damage()
}

/// A stack's maximum damage, when it was given one.
#[must_use]
pub fn get_max_damage(stack: &ItemStack) -> Option<i32> {
    stack.max_damage()
}

/// A stack's metadata, which is its aux value.
#[must_use]
pub fn get_metadata(stack: &ItemStack) -> i32 {
    stack.aux()
}

/// A stack's own name.
#[must_use]
pub fn get_name(stack: &ItemStack) -> String {
    stack.display_name()
}

/// A stack's display name.
#[must_use]
pub fn get_display_name(stack: &ItemStack) -> String {
    stack.display_name()
}

/// A stack's lore.
#[must_use]
pub fn get_lore(stack: &ItemStack) -> Vec<String> {
    stack.lore()
}

/// Gives a stack a name.
pub fn set_name(stack: &mut ItemStack, name: &str) {
    stack.set_name(name);
}

/// Sets a stack's lore.
pub fn set_lore(stack: &mut ItemStack, lines: &[&str]) {
    stack.set_lore(lines);
}

/// A stack's enchantments.
#[must_use]
pub fn get_enchantments(stack: &ItemStack) -> BTreeMap<String, i32> {
    stack.enchantments()
}

/// Adds an enchantment to a stack.
pub fn add_enchantment(stack: &mut ItemStack, name: &str, level: i32) {
    stack.add_enchantment(name, level);
}

/// Removes an enchantment from a stack.
pub fn remove_enchantment(stack: &mut ItemStack, name: &str) -> bool {
    stack.remove_enchantment(name)
}

/// Whether a stack is enchanted.
#[must_use]
pub fn is_enchanted(stack: &ItemStack) -> bool {
    stack.is_enchanted()
}

/// Whether a stack stacks.
#[must_use]
pub fn is_stackable_stack(stack: &ItemStack) -> bool {
    stack.is_stackable()
}

/// Adds a component to a stack.
pub fn set_component(stack: &mut ItemStack, component: Component) {
    stack.set_component(component);
}

/// One component of a stack.
#[must_use]
pub fn get_component_of(stack: &ItemStack, name: &str) -> Option<Component> {
    stack.component(name)
}

/// Removes a component from a stack.
pub fn remove_component(stack: &mut ItemStack, name: &str) -> bool {
    stack.remove_component(name)
}

/// Every component of a stack.
#[must_use]
pub fn components_of(stack: &ItemStack) -> Vec<Component> {
    stack.components()
}

/// What this host cannot reach about items, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["registry", "hand", "icon", "model", "components", "enchantments", "tags", "events"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The event kind a mod would subscribe to for an item being used, spelled the way the bus
/// spells it.
#[must_use]
pub fn use_event_kind() -> &'static str {
    event_kind::REGISTRY_CHANGED
}

/// The event kinds this host would need from the client to fire an item callback.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    ITEM_EVENTS.to_vec()
}

/// Whether an item event kind is one the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    ITEM_EVENTS.contains(&kind) && events::is_registered_kind(kind)
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

    #[test]
    fn a_stack_is_a_value_and_the_operations_work_on_it() {
        let mut stack = ItemStack::new("minecraft:stone", 16);
        assert_eq!(stack.count(), 16);
        assert!(stack.is_stackable());
        assert!(stack.is_plain());
        assert!(!stack.is_empty());

        let split = stack.split(4).expect("sixteen holds four");
        assert_eq!(split.count(), 4);
        assert_eq!(stack.count(), 12);
        assert!(stack.split(0).is_none());
        assert!(stack.split(99).is_none());

        let whole = stack.copy();
        assert_eq!(whole.count(), 12);
        assert_eq!(stack.cloned().item(), "minecraft:stone");

        let other = ItemStack::new("minecraft:stone", 60);
        assert_eq!(stack.merge(&other), 8, "only twelve fit");
        assert_eq!(stack.count(), 64);
        assert_eq!(other.count(), 60, "merging does not take from the source");
        assert_eq!(ItemStack::new("minecraft:dirt", 1).merge(&other), 60, "different items do not merge");
    }

    #[test]
    fn a_stack_carries_a_name_lore_damage_and_components() {
        let mut stack = ItemStack::new("minecraft:iron_pickaxe", 1).with_max_count(1);
        assert!(!stack.is_stackable());
        assert!(!stack.is_damageable(), "nothing says it takes damage yet");
        stack.set_component(Component::new("minecraft:durability", "250"));
        assert!(stack.is_damageable());
        assert_eq!(stack.durability(), Some(250));
        stack.set_damage(50);
        assert_eq!(stack.damage(), Some(50));
        assert_eq!(stack.durability(), Some(200));
        assert_eq!(stack.max_damage(), Some(250));

        stack.set_name("Digger");
        stack.set_lore(&["made in a test", "still sharp"]);
        assert_eq!(stack.name(), Some("Digger"));
        assert_eq!(stack.display_name(), "Digger");
        assert_eq!(stack.lore(), vec!["made in a test".to_owned(), "still sharp".to_owned()]);
        assert!(!stack.is_plain());

        stack.add_enchantment("minecraft:efficiency", 3);
        assert!(stack.is_enchanted());
        assert_eq!(stack.enchantments().get("minecraft:efficiency"), Some(&3));
        assert!(stack.remove_enchantment("minecraft:efficiency"));
        assert!(!stack.remove_enchantment("minecraft:efficiency"));

        assert_eq!(stack.component("minecraft:durability").and_then(|c| c.as_integer()), Some(250));
        assert!(stack.remove_component("minecraft:lore"));
        assert!(stack.lore().is_empty());
        assert!(stack.to_string().contains("Digger"), "{stack}");
    }

    #[test]
    fn a_declared_item_can_be_read_back_and_is_honest_about_the_client() {
        let _serial = serial();
        fresh();
        assert!(register(
            ItemType::new("lantern", "Lantern")
                .max_count(16)
                .durability(64)
                .icon("hello:lantern")
                .tag("cinnaloader:lights")
                .component(Component::custom_name("Lantern"))
                .use_action("place", 10)
                .cooldown(5)
                .food(&[Component::new("minecraft:nutrition", "4")])
                .armour(&[Component::new("minecraft:armor_slot", "head")])
                .tool(&[Component::new("minecraft:mining_speed", "2")])
        ));
        assert_eq!(get("lantern").map(|item| item.name), Some("Lantern".to_owned()));
        assert_eq!(get_by_name("Lantern").map(|item| item.id), Some("lantern".to_owned()));
        assert_eq!(get_all().len(), 1);
        assert_eq!(get_id_of("lantern"), Some("lantern".to_owned()));
        assert!(get_registry_id("lantern").is_some_and(|id| id.ends_with(":lantern")));
        assert_eq!(get_max_stack_size("lantern"), Some(16));
        assert_eq!(get_durability("lantern"), Some(64));
        assert_eq!(is_damageable("lantern"), Some(true));
        assert_eq!(is_stackable("lantern"), Some(true));
        assert_eq!(is_edible("lantern"), Some(true));
        assert_eq!(is_equippable("lantern"), Some(true));
        assert_eq!(get_tags("lantern"), vec!["cinnaloader:lights".to_owned()]);
        assert!(has_tag("lantern", "cinnaloader:lights"));
        assert!(!has_tag("lantern", "nothing"));
        assert_eq!(get_use_action("lantern"), Some("place".to_owned()));
        assert_eq!(get_use_duration("lantern"), Some(10));
        assert_eq!(get_cooldown("lantern"), Some(5));
        assert_eq!(get_texture("lantern"), Some("hello:lantern".to_owned()));
        assert_eq!(get_food_properties("lantern").len(), 1);
        assert_eq!(get_armour_properties("lantern").len(), 1);
        assert_eq!(get_tool_properties("lantern").len(), 1);
        assert_eq!(get_components("lantern").len(), 1);
        assert!(get_component("lantern", "minecraft:custom_name").is_some());
        assert!(register_component("lantern", Component::damage(0)));
        assert_eq!(get_components("lantern").len(), 2);
        assert!(!register_component("nothing", Component::damage(0)));
        assert!(!is_registered_with_the_client("lantern"));
        let stack = get("lantern").expect("declared").stack(3);
        assert_eq!(stack.count(), 3);
        assert_eq!(stack.max_count(), 16);
        assert_eq!(stack.max_damage(), Some(64));
        assert!(unregister("lantern"));
        assert!(!unregister("lantern"));
    }

    #[test]
    fn what_the_client_owns_is_refused_with_a_reason() {
        let _serial = serial();
        fresh();
        assert!(get_stack().is_none());
        assert!(get_model("minecraft:stone").is_none());
        assert!(get("minecraft:stone").is_none(), "the client's registry is out of reach");
        assert_eq!(refusals().len(), 8);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_free_functions_are_the_stack_methods_under_the_wishlists_names() {
        let mut stack = create_stack("minecraft:apple", 8);
        assert_eq!(get_metadata(&stack), 0);
        assert_eq!(get_name(&stack), "minecraft:apple");
        set_name(&mut stack, "Apple");
        assert_eq!(get_display_name(&stack), "Apple");
        set_lore(&mut stack, &["one a day"]);
        assert_eq!(get_lore(&stack), vec!["one a day".to_owned()]);
        set_count(&mut stack, 3);
        assert_eq!(stack.count(), 3);
        let split = split_stack(&mut stack, 1).expect("three holds one");
        assert_eq!(split.count(), 1);
        set_damage(&mut stack, 2);
        assert_eq!(get_damage(&stack), Some(2));
        assert_eq!(get_max_damage(&stack), None);
        add_enchantment(&mut stack, "minecraft:sharpness", 1);
        assert!(is_enchanted(&stack));
        assert_eq!(get_enchantments(&stack).len(), 1);
        assert!(remove_enchantment(&mut stack, "minecraft:sharpness"));
        set_component(&mut stack, Component::new("minecraft:foil", "true"));
        assert!(get_component_of(&stack, "minecraft:foil").is_some());
        // damage, the custom name, the lore and the foil: four components.
        assert_eq!(components_of(&stack).len(), 4);
        assert!(remove_component(&mut stack, "minecraft:foil"));
        assert!(is_stackable_stack(&stack));
        assert_eq!(copy_stack(&stack).count(), stack.count());
        let mut into = ItemStack::new("minecraft:apple", 62);
        assert_eq!(merge_stacks(&mut into, &stack), 0);
        assert_eq!(into.count(), 64);
    }

    #[test]
    fn the_item_event_kinds_can_be_subscribed_to_before_they_fire() {
        let _serial = serial();
        assert!(register_event_kinds() > 0);
        assert!(has_event_kind("item_use"));
        assert!(!has_event_kind("nothing_at_all"));
        assert_eq!(ITEM_EVENTS.len(), 19);
    }
}
