//! Inventory and containers: a slot store this host owns, and the live one it does not.
//!
//! [`Inventory`] is a value: a slot vector using the client's own indices, with everything a
//! mod needs to work over one — get, set, clear, contents, add, remove, contains, count,
//! find, insert, extract, swap, move, split, merge, transactions, a revision counter and a
//! lock. It is *the mod's copy*, never the player's: binding the live inventory is
//! [`get`], which answers with the reason from [`why_unknown`] rather than handing back
//! something that is not the player's.
//!
//! The slot layout is the client's, because a mod that has to talk about a slot should use
//! the same numbers the game does: `0..=8` hotbar, `9..=35` main, `36` offhand, `37..=40`
//! armour.
//!
//! `containers::*` describes and refuses: a mod declares a container type (its slots,
//! properties, title, distance limit) and every mod can read it, while opening, closing,
//! reading or writing a *live* container is the client's screen and inventory session, which
//! needs hooks. `on_slot_click` and `on_property_change` are recorded and never called here,
//! like a block's behaviour callbacks.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let mut bag = Inventory::player();
//! bag.set_slot(0, Some(ItemStack::new("minecraft:stone", 64)));
//! assert_eq!(bag.count("minecraft:stone"), 64);
//! let moved = bag.move_stack(0, 9).expect("an empty slot takes it");
//! assert_eq!(moved.count(), 64);
//! assert!(inventory::get().is_none(), "the player's inventory is the client's");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::items::ItemStack;

/// How many slots a player's inventory has, counting the armour and the offhand.
pub const PLAYER_SLOTS: usize = 41;
/// The first hotbar slot.
pub const HOTBAR_FIRST: usize = 0;
/// The last hotbar slot.
pub const HOTBAR_LAST: usize = 8;
/// The first main-inventory slot.
pub const MAIN_FIRST: usize = 9;
/// The last main-inventory slot.
pub const MAIN_LAST: usize = 35;
/// The offhand.
pub const OFFHAND: usize = 36;
/// The first armour slot: feet, legs, chest, head.
pub const ARMOUR_FIRST: usize = 37;
/// The last armour slot.
pub const ARMOUR_LAST: usize = 40;

/// Why a live-inventory question has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "inventory" | "get" => {
            "the player's inventory is the client's own state; binding it needs the hooks \
             section 10 lists as never"
                .to_owned()
        }
        "slot" => "a live slot is the client's; a mod's copy is what this host can hold".to_owned(),
        "selection" => "the selected hotbar slot is the client's player state".to_owned(),
        "container" => {
            "the open container is the client's inventory session, which this host cannot reach"
                .to_owned()
        }
        "screen" => "what the client is showing is its own screen state".to_owned(),
        "viewers" => {
            "a client has no viewer list; that is a server idea, and the local player is the \
             only one here"
                .to_owned()
        }
        "click" => {
            "slot clicks and property changes come from the client's screens; callbacks \
             recorded here are never called by this host"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What kind of slot a position is, using the client's layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotKind {
    Hotbar,
    Main,
    Offhand,
    Armour,
    /// Outside the player's own inventory, in a container that was opened.
    Container,
}

impl SlotKind {
    /// Which kind a player-inventory index is.
    #[must_use]
    pub fn of(index: usize) -> Self {
        match index {
            HOTBAR_FIRST..=HOTBAR_LAST => Self::Hotbar,
            MAIN_FIRST..=MAIN_LAST => Self::Main,
            OFFHAND => Self::Offhand,
            ARMOUR_FIRST..=ARMOUR_LAST => Self::Armour,
            _ => Self::Container,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hotbar => "hotbar",
            Self::Main => "main",
            Self::Offhand => "offhand",
            Self::Armour => "armour",
            Self::Container => "container",
        }
    }
}

/// What went wrong in a slot operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryError {
    /// The index is past the end of the inventory.
    NoSuchSlot(usize),
    /// The inventory is locked.
    Locked,
    /// Nothing is in that slot.
    Empty(usize),
    /// The stack does not fit, and this many were left over.
    Full(u32),
    /// The two slots hold items that cannot be merged.
    Mismatch,
}

impl fmt::Display for InventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchSlot(index) => write!(formatter, "there is no slot {index}"),
            Self::Locked => formatter.write_str("the inventory is locked"),
            Self::Empty(index) => write!(formatter, "slot {index} is empty"),
            Self::Full(left) => write!(formatter, "{left} did not fit"),
            Self::Mismatch => formatter.write_str("those slots hold different items"),
        }
    }
}

impl std::error::Error for InventoryError {}

/// Somewhere one or more stacks can go: a slot store with the client's indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    slots: Vec<Option<ItemStack>>,
    revision: u64,
    locked: bool,
    /// A snapshot a transaction put aside, if one is open.
    transaction: Option<Vec<Option<ItemStack>>>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::with_size(PLAYER_SLOTS)
    }
}

impl Inventory {
    /// A player's inventory: hotbar, main, offhand and armour.
    #[must_use]
    pub fn player() -> Self {
        Self::default()
    }

    /// A container with `size` slots and the client's indices underneath them.
    #[must_use]
    pub fn with_size(size: usize) -> Self {
        Self {
            slots: vec![None; size],
            revision: 0,
            locked: false,
            transaction: None,
        }
    }

    /// A hotbar with nothing in it.
    #[must_use]
    pub fn hotbar() -> Self {
        Self::with_size(HOTBAR_LAST + 1)
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.slots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// How many slots hold something.
    #[must_use]
    pub fn used_slots(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    /// Whether the inventory is locked against changes.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    /// A counter that changes whenever a slot does, for a caller that caches.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn touched(&mut self) {
        self.revision += 1;
    }

    fn slot(&self, index: usize) -> Result<&Option<ItemStack>, InventoryError> {
        self.slots
            .get(index)
            .ok_or(InventoryError::NoSuchSlot(index))
    }

    /// What is in a slot.
    pub fn get_slot(&self, index: usize) -> Result<Option<ItemStack>, InventoryError> {
        Ok(self.slot(index)?.clone())
    }

    /// Puts a stack in a slot.
    pub fn set_slot(
        &mut self,
        index: usize,
        stack: Option<ItemStack>,
    ) -> Result<(), InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        if index >= self.slots.len() {
            return Err(InventoryError::NoSuchSlot(index));
        }
        self.slots[index] = stack.filter(|stack| !stack.is_empty());
        self.touched();
        Ok(())
    }

    /// Empties a slot, and answers what was in it.
    pub fn clear_slot(&mut self, index: usize) -> Result<Option<ItemStack>, InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        if index >= self.slots.len() {
            return Err(InventoryError::NoSuchSlot(index));
        }
        let taken = self.slots[index].take();
        if taken.is_some() {
            self.touched();
        }
        Ok(taken)
    }

    /// Empties every slot, and answers how many had something.
    pub fn clear(&mut self) -> Result<usize, InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        let taken = self.used_slots();
        self.slots.fill(None);
        if taken > 0 {
            self.touched();
        }
        Ok(taken)
    }

    /// Every slot, in index order, with nothing in the empty ones.
    #[must_use]
    pub fn get_contents(&self) -> Vec<(usize, ItemStack)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.clone().map(|stack| (index, stack)))
            .collect()
    }

    /// Replaces the contents from a list of slots.
    pub fn set_contents(&mut self, contents: &[(usize, ItemStack)]) -> Result<(), InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        self.slots.fill(None);
        for (index, stack) in contents {
            if *index >= self.slots.len() {
                return Err(InventoryError::NoSuchSlot(*index));
            }
            self.slots[*index] = Some(stack.clone());
        }
        self.touched();
        Ok(())
    }

    /// Adds a stack wherever it fits, and answers how many were left over.
    pub fn add_item(&mut self, stack: &ItemStack) -> Result<u32, InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        let mut left = stack.count();
        let mut added = false;
        // First, top up stacks of the same item.
        for slot in &mut self.slots {
            if left == 0 {
                break;
            }
            if let Some(held) = slot {
                if held.item() == stack.item() {
                    left = held.merge(&ItemStack::new(stack.item(), left).with_max_count(stack.max_count()));
                    added = true;
                }
            }
        }
        // Then, an empty slot, which takes at most what its own maximum allows.
        for slot in &mut self.slots {
            if left == 0 {
                break;
            }
            if slot.is_none() {
                let mut fresh = stack.clone();
                let room = fresh.max_count();
                fresh.set_count(left.min(room));
                left -= fresh.count();
                *slot = Some(fresh);
                added = true;
            }
        }
        if added {
            self.touched();
        }
        Ok(left)
    }

    /// Takes `count` of an item out of the inventory, and answers how many went.
    pub fn remove_item(&mut self, item: &str, count: u32) -> Result<u32, InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        let mut left = count;
        let mut removed = 0;
        for slot in &mut self.slots {
            if left == 0 {
                break;
            }
            let Some(held) = slot else {
                continue;
            };
            if held.item() != item {
                continue;
            }
            let take = held.count().min(left);
            left -= take;
            removed += take;
            if held.count() == take {
                *slot = None;
            } else {
                held.set_count(held.count() - take);
            }
        }
        if removed > 0 {
            self.touched();
        }
        Ok(removed)
    }

    /// Whether any slot holds this item.
    #[must_use]
    pub fn contains(&self, item: &str) -> bool {
        self.count(item) > 0
    }

    /// Whether at least this many of an item are held.
    #[must_use]
    pub fn contains_amount(&self, item: &str, count: u32) -> bool {
        self.count(item) >= count
    }

    /// How many of an item are held, in total.
    #[must_use]
    pub fn count(&self, item: &str) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|stack| stack.item() == item)
            .map(ItemStack::count)
            .sum()
    }

    /// The first slot holding an item.
    #[must_use]
    pub fn find(&self, item: &str) -> Option<usize> {
        self.first_matching(|stack| stack.item() == item)
    }

    /// Every slot holding an item.
    #[must_use]
    pub fn find_all(&self, item: &str) -> Vec<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.as_ref()
                    .filter(|stack| stack.item() == item)
                    .map(|_| index)
            })
            .collect()
    }

    /// The first slot a stack could go into, taking room into account.
    #[must_use]
    pub fn first_empty_slot(&self) -> Option<usize> {
        self.slots.iter().position(Option::is_none)
    }

    fn first_matching(&self, wanted: impl Fn(&ItemStack) -> bool) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(&wanted))
    }

    /// Whether a stack would fit, either topped onto one of the same item or into an empty
    /// slot.
    #[must_use]
    pub fn can_insert(&self, stack: &ItemStack) -> bool {
        self.slots.iter().any(|slot| match slot {
            Some(held) => held.item() == stack.item() && held.count() < held.max_count(),
            None => true,
        })
    }

    /// Whether a slot can be emptied.
    #[must_use]
    pub fn can_extract(&self, index: usize) -> bool {
        self.slots.get(index).is_some_and(Option::is_some)
    }

    /// Puts a stack in a slot, and answers how many did not fit.
    pub fn insert(&mut self, index: usize, stack: &ItemStack) -> Result<u32, InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        let current = self.get_slot(index)?;
        match current {
            None => {
                self.set_slot(index, Some(stack.clone()))?;
                Ok(0)
            }
            Some(mut held) => {
                if held.item() != stack.item() {
                    return Err(InventoryError::Mismatch);
                }
                let leftover = held.merge(stack);
                let held_count = held.count();
                let _ = held_count;
                self.set_slot(index, Some(held))?;
                Ok(leftover)
            }
        }
    }

    /// Takes a stack out of a slot.
    pub fn extract(&mut self, index: usize) -> Result<ItemStack, InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        self.clear_slot(index)?
            .ok_or(InventoryError::Empty(index))
    }

    /// Swaps two slots, and answers the pair as they are now.
    pub fn swap(&mut self, left: usize, right: usize) -> Result<(), InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        if left >= self.slots.len() {
            return Err(InventoryError::NoSuchSlot(left));
        }
        if right >= self.slots.len() {
            return Err(InventoryError::NoSuchSlot(right));
        }
        self.slots.swap(left, right);
        self.touched();
        Ok(())
    }

    /// Moves a whole stack from one slot to another, which fails when the target holds a
    /// different item.
    pub fn move_stack(&mut self, from: usize, to: usize) -> Result<ItemStack, InventoryError> {
        let stack = self.extract(from)?;
        match self.insert(to, &stack) {
            Ok(0) => Ok(stack),
            Ok(left) => {
                // Put back what did not fit, rather than losing it.
                let mut partial = stack.clone();
                partial.set_count(left);
                self.set_slot(from, Some(partial))?;
                Ok(stack)
            }
            Err(error) => {
                self.set_slot(from, Some(stack))?;
                Err(error)
            }
        }
    }

    /// Takes `count` off the top of a slot, and answers the new stack.
    pub fn split_stack(
        &mut self,
        index: usize,
        count: u32,
    ) -> Result<ItemStack, InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        let held = self.get_slot(index)?.ok_or(InventoryError::Empty(index))?;
        let mut held = held;
        let split = held.split(count).ok_or(InventoryError::Full(count))?;
        self.set_slot(index, Some(held))?;
        Ok(split)
    }

    /// Merges one slot into another, and answers how many did not fit.
    pub fn merge_stacks(&mut self, from: usize, to: usize) -> Result<u32, InventoryError> {
        if self.locked {
            return Err(InventoryError::Locked);
        }
        let source = self.get_slot(from)?.ok_or(InventoryError::Empty(from))?;
        let target = self.get_slot(to)?.ok_or(InventoryError::Empty(to))?;
        if source.item() != target.item() {
            return Err(InventoryError::Mismatch);
        }
        let mut target = target;
        let left = target.merge(&source);
        self.set_slot(to, Some(target))?;
        if left == 0 {
            self.clear_slot(from)?;
        } else {
            // Whatever did not fit stays where it was.
            let mut rest = source;
            rest.set_count(left);
            self.set_slot(from, Some(rest))?;
        }
        Ok(left)
    }

    /// A snapshot a transaction can put back.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Option<ItemStack>> {
        self.slots.clone()
    }

    /// Puts a snapshot back.
    pub fn restore(&mut self, snapshot: Vec<Option<ItemStack>>) {
        self.slots = snapshot;
        self.touched();
    }

    /// Starts a transaction, and answers whether one was already open.
    pub fn begin_transaction(&mut self) -> bool {
        if self.transaction.is_some() {
            return false;
        }
        self.transaction = Some(self.snapshot());
        true
    }

    /// Keeps everything a transaction changed.
    pub fn commit_transaction(&mut self) -> bool {
        self.transaction.take().is_some()
    }

    /// Puts everything back as it was when the transaction began.
    pub fn rollback_transaction(&mut self) -> bool {
        match self.transaction.take() {
            Some(snapshot) => {
                self.restore(snapshot);
                true
            }
            None => false,
        }
    }

    /// Whether a transaction is open.
    #[must_use]
    pub fn has_transaction(&self) -> bool {
        self.transaction.is_some()
    }

    /// Locking stops changes until it is unlocked, for a mod that handes the inventory to a
    /// worker while it reasons about it.
    pub fn lock(&mut self) {
        self.locked = true;
    }

    pub fn unlock(&mut self) {
        self.locked = false;
    }
}

impl fmt::Display for Inventory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} slots, {} used, revision {}",
            self.size(),
            self.used_slots(),
            self.revision
        )
    }
}

/// The inventory this mod owns, which the module-level functions work on.
fn local() -> &'static Mutex<Inventory> {
    static LOCAL: OnceLock<Mutex<Inventory>> = OnceLock::new();
    LOCAL.get_or_init(|| Mutex::new(Inventory::player()))
}

fn with<T>(body: impl FnOnce(&mut Inventory) -> T) -> T {
    let mut guard: MutexGuard<'_, Inventory> = local().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The mod's own inventory, for a caller that wants to hold it.
pub fn local_inventory() -> Inventory {
    with(|inventory| inventory.clone())
}

/// Replaces the mod's inventory with another, which a load or a save would do.
pub fn set_local_inventory(inventory: Inventory) {
    with(|held| *held = inventory);
}

/// Binds the player's live inventory. This host cannot: the client owns it.
#[must_use]
pub fn get() -> Option<Inventory> {
    None
}

/// How many slots the mod's own inventory has.
#[must_use]
pub fn get_size() -> usize {
    with(|inventory| inventory.size())
}

/// What is in a slot of the mod's own inventory.
pub fn get_slot(index: usize) -> Result<Option<ItemStack>, InventoryError> {
    with(|inventory| inventory.get_slot(index))
}

/// Puts a stack in a slot.
pub fn set_slot(index: usize, stack: Option<ItemStack>) -> Result<(), InventoryError> {
    with(|inventory| inventory.set_slot(index, stack))
}

/// Empties a slot.
pub fn clear_slot(index: usize) -> Result<Option<ItemStack>, InventoryError> {
    with(|inventory| inventory.clear_slot(index))
}

/// Empties the inventory.
pub fn clear() -> Result<usize, InventoryError> {
    with(|inventory| inventory.clear())
}

/// Every slot that holds something.
#[must_use]
pub fn get_contents() -> Vec<(usize, ItemStack)> {
    with(|inventory| inventory.get_contents())
}

/// Replaces the contents.
pub fn set_contents(contents: &[(usize, ItemStack)]) -> Result<(), InventoryError> {
    with(|inventory| inventory.set_contents(contents))
}

/// Adds a stack wherever it fits, and answers what was left over.
pub fn add_item(stack: &ItemStack) -> Result<u32, InventoryError> {
    with(|inventory| inventory.add_item(stack))
}

/// Takes an item out, and answers how many went.
pub fn remove_item(item: &str, count: u32) -> Result<u32, InventoryError> {
    with(|inventory| inventory.remove_item(item, count))
}

#[must_use]
pub fn contains(item: &str) -> bool {
    with(|inventory| inventory.contains(item))
}

#[must_use]
pub fn contains_amount(item: &str, count: u32) -> bool {
    with(|inventory| inventory.contains_amount(item, count))
}

#[must_use]
pub fn count(item: &str) -> u32 {
    with(|inventory| inventory.count(item))
}

#[must_use]
pub fn find(item: &str) -> Option<usize> {
    with(|inventory| inventory.find(item))
}

#[must_use]
pub fn find_all(item: &str) -> Vec<usize> {
    with(|inventory| inventory.find_all(item))
}

#[must_use]
pub fn first_empty_slot() -> Option<usize> {
    with(|inventory| inventory.first_empty_slot())
}

#[must_use]
pub fn can_insert(stack: &ItemStack) -> bool {
    with(|inventory| inventory.can_insert(stack))
}

#[must_use]
pub fn can_extract(index: usize) -> bool {
    with(|inventory| inventory.can_extract(index))
}

/// Puts a stack in a slot, and answers what did not fit.
pub fn insert(index: usize, stack: &ItemStack) -> Result<u32, InventoryError> {
    with(|inventory| inventory.insert(index, stack))
}

/// Takes a stack out of a slot.
pub fn extract(index: usize) -> Result<ItemStack, InventoryError> {
    with(|inventory| inventory.extract(index))
}

/// Swaps two slots.
pub fn swap(left: usize, right: usize) -> Result<(), InventoryError> {
    with(|inventory| inventory.swap(left, right))
}

/// Moves a stack from one slot to another.
pub fn move_stack(from: usize, to: usize) -> Result<ItemStack, InventoryError> {
    with(|inventory| inventory.move_stack(from, to))
}

/// Takes part of a stack.
pub fn split_stack(index: usize, count: u32) -> Result<ItemStack, InventoryError> {
    with(|inventory| inventory.split_stack(index, count))
}

/// Merges two slots of the same item.
pub fn merge_stacks(from: usize, to: usize) -> Result<u32, InventoryError> {
    with(|inventory| inventory.merge_stacks(from, to))
}

/// The selected hotbar slot. The client owns the real one.
#[must_use]
pub fn get_selected_slot() -> Option<usize> {
    None
}

/// Asks the client to select a hotbar slot. This host cannot.
pub fn set_selected_slot(_index: usize) -> bool {
    false
}

/// The armour the player is wearing. The client owns it.
#[must_use]
pub fn get_armor() -> Vec<Option<ItemStack>> {
    Vec::new()
}

/// The offhand. The client owns it.
#[must_use]
pub fn get_offhand() -> Option<ItemStack> {
    None
}

/// The stack on the cursor. The client owns it.
#[must_use]
pub fn get_cursor_stack() -> Option<ItemStack> {
    None
}

/// Asks the client to put something on the cursor, which this host cannot do.
pub fn set_cursor_stack(_stack: Option<ItemStack>) -> bool {
    false
}

/// The open transaction, if one is.
#[must_use]
pub fn get_transaction() -> bool {
    with(|inventory| inventory.has_transaction())
}

/// Starts a transaction on the mod's own inventory.
pub fn begin_transaction() -> bool {
    with(|inventory| inventory.begin_transaction())
}

/// Keeps what a transaction changed.
pub fn commit_transaction() -> bool {
    with(|inventory| inventory.commit_transaction())
}

/// Puts back what a transaction changed.
pub fn rollback_transaction() -> bool {
    with(|inventory| inventory.rollback_transaction())
}

/// The revision counter, which changes whenever a slot does.
#[must_use]
pub fn get_revision() -> u64 {
    with(|inventory| inventory.revision())
}

/// Stops changes to the mod's own inventory.
pub fn lock() {
    with(|inventory| inventory.lock());
}

/// Lets changes through again.
pub fn unlock() {
    with(|inventory| inventory.unlock());
}

/// What kind of slot an index is, in the client's layout.
#[must_use]
pub fn slot_kind(index: usize) -> SlotKind {
    SlotKind::of(index)
}

/// A container a mod declares: what it is called, how many slots, what it shows and how far
/// away it can be used.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerType {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub slots: usize,
    /// Properties a screen shows, such as a furnace's burn and cook progress.
    pub properties: Vec<String>,
    pub title: String,
    pub distance_limit: f64,
    /// Handlers a mod would like the client to call, recorded and never called here.
    pub on_slot_click: bool,
    pub on_property_change: bool,
    pub registered: bool,
}

impl ContainerType {
    /// A container with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str, slots: usize) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            slots,
            properties: Vec::new(),
            title: name.to_owned(),
            distance_limit: 3.0,
            on_slot_click: false,
            on_property_change: false,
            registered: false,
        }
    }

    #[must_use]
    pub fn property(mut self, property: &str) -> Self {
        self.properties.push(property.to_owned());
        self
    }

    #[must_use]
    pub fn title(mut self, title: &str) -> Self {
        self.title = title.to_owned();
        self
    }

    #[must_use]
    pub fn distance_limit(mut self, distance: f64) -> Self {
        self.distance_limit = distance;
        self
    }

    /// Records that the mod has a slot-click handler. This host never calls it.
    #[must_use]
    pub fn on_slot_click(mut self) -> Self {
        self.on_slot_click = true;
        self
    }

    /// Records that the mod has a property-change handler. This host never calls it.
    #[must_use]
    pub fn on_property_change(mut self) -> Self {
        self.on_property_change = true;
        self
    }
}

impl fmt::Display for ContainerType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {} slots, {})",
            self.title, self.id, self.slots, self.name
        )
    }
}

fn types() -> &'static Mutex<BTreeMap<String, ContainerType>> {
    static TYPES: OnceLock<Mutex<BTreeMap<String, ContainerType>>> = OnceLock::new();
    TYPES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_types<T>(body: impl FnOnce(&mut BTreeMap<String, ContainerType>) -> T) -> T {
    let mut guard = types().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a container type, which every mod can read and the client cannot be told about.
pub fn register_type(container: ContainerType) -> bool {
    crate::warn!(
        "{} declared the container {}: this host records it, but {}",
        crate::metadata::id(),
        container.id,
        why_unknown("container")
    );
    with_types(|types| types.insert(container.id.clone(), container).is_none())
}

/// Forgets a declared container type.
pub fn unregister_type(id: &str) -> bool {
    with_types(|types| types.remove(id).is_some())
}

/// A declared container type.
#[must_use]
pub fn get_type(id: &str) -> Option<ContainerType> {
    with_types(|types| types.get(id).cloned())
}

/// Every declared container type.
#[must_use]
pub fn get_all_types() -> Vec<ContainerType> {
    with_types(|types| types.values().cloned().collect())
}

/// Records that a mod has a screen handler for a container. This host never calls it.
pub fn register_screen_handler(id: &str) -> bool {
    with_types(|types| match types.get_mut(id) {
        Some(container) => {
            container.on_slot_click = true;
            true
        }
        None => false,
    })
}

/// Records a slot rule for a declared container, pairing a slot with what may go in it. The
/// rule is real data; nothing here enforces it against the client.
pub fn register_slot_rule(id: &str, slot: usize, accepts: &str) -> bool {
    with_types(|types| match types.get_mut(id) {
        Some(container) => {
            container
                .properties
                .push(format!("slot {slot} accepts {accepts}"));
            true
        }
        None => false,
    })
}

/// Opens a live container. This host cannot: the client's screen is its own.
#[must_use]
pub fn open(_id: &str) -> Option<()> {
    None
}

/// Closes whatever is open on the client, which this host cannot ask for.
pub fn close() -> bool {
    crate::warn!("a mod asked this host to close a container: {}", why_unknown("screen"));
    false
}

/// A live container. `None`: the client owns it.
#[must_use]
pub fn container_of(_id: &str) -> Option<()> {
    None
}

/// The atomic::get on a live container's slots. `None`: the client owns it.
#[must_use]
pub fn get_slots() -> Vec<(usize, ItemStack)> {
    Vec::new()
}

/// A live container's properties. Empty: the client owns it.
#[must_use]
pub fn get_properties() -> Vec<(String, i32)> {
    Vec::new()
}

/// Who has the container open. A client has no viewer list; that is a server idea.
#[must_use]
pub fn get_viewers() -> Vec<String> {
    Vec::new()
}

/// Whether a container can be interacted with from where the player is. The client decides.
#[must_use]
pub fn can_interact() -> Option<bool> {
    None
}

/// How far a live container can be used from. The client decides.
#[must_use]
pub fn get_distance_limit() -> Option<f64> {
    None
}

/// A live container's title. The client draws it.
#[must_use]
pub fn get_title() -> Option<String> {
    None
}

/// A slot click, which the client would report. Nothing here calls it.
pub fn on_slot_click(_slot: usize, _button: u8) -> bool {
    crate::warn!("a mod asked this host to deliver a slot click: {}", why_unknown("click"));
    false
}

/// A property change, which the client would report. Nothing here calls it.
pub fn on_property_change(_property: &str, _value: i32) -> bool {
    crate::warn!(
        "a mod asked this host to deliver a container property change: {}",
        why_unknown("click")
    );
    false
}

/// What this host cannot reach about inventories and containers.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["inventory", "slot", "selection", "container", "screen", "viewers", "click"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn stone(count: u32) -> ItemStack {
        ItemStack::new("minecraft:stone", count)
    }

    #[test]
    fn the_slot_layout_is_the_clients() {
        assert_eq!(SlotKind::of(0), SlotKind::Hotbar);
        assert_eq!(SlotKind::of(8), SlotKind::Hotbar);
        assert_eq!(SlotKind::of(9), SlotKind::Main);
        assert_eq!(SlotKind::of(35), SlotKind::Main);
        assert_eq!(SlotKind::of(36), SlotKind::Offhand);
        assert_eq!(SlotKind::of(37), SlotKind::Armour);
        assert_eq!(SlotKind::of(40), SlotKind::Armour);
        assert_eq!(SlotKind::of(41), SlotKind::Container);
        assert_eq!(SlotKind::Hotbar.as_str(), "hotbar");
        assert_eq!(Inventory::player().size(), PLAYER_SLOTS);
        assert_eq!(Inventory::hotbar().size(), 9);
    }

    #[test]
    fn a_slot_can_be_set_read_and_cleared() {
        let mut bag = Inventory::player();
        assert!(bag.is_empty());
        assert_eq!(bag.revision(), 0);
        bag.set_slot(0, Some(stone(64))).unwrap();
        assert_eq!(bag.count("minecraft:stone"), 64);
        assert_eq!(bag.used_slots(), 1);
        assert_eq!(bag.revision(), 1);
        assert_eq!(
            bag.get_slot(0).unwrap().map(|stack| stack.count()),
            Some(64)
        );
        assert_eq!(bag.clear_slot(0).unwrap().map(|stack| stack.count()), Some(64));
        assert!(bag.get_slot(0).unwrap().is_none());
        assert!(bag.get_slot(99).is_err());
        assert!(bag.set_slot(99, None).is_err());
        assert_eq!(slot_kind(0).as_str(), "hotbar");
    }

    #[test]
    fn contents_read_and_write_as_a_list_of_slots() {
        let mut bag = Inventory::player();
        bag.set_contents(&[(0, stone(1)), (40, stone(2))]).unwrap();
        assert_eq!(bag.used_slots(), 2);
        let contents = bag.get_contents();
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0].0, 0);
        assert_eq!(contents[1].0, 40);
        assert_eq!(bag.clear().unwrap(), 2);
        assert!(bag.is_empty());
        assert!(bag.set_contents(&[(99, stone(1))]).is_err());
    }

    #[test]
    fn adding_and_removing_an_item_works_the_way_a_mod_expects() {
        let mut bag = Inventory::hotbar();
        assert_eq!(bag.add_item(&stone(64)).unwrap(), 0, "it fits in one slot");
        assert_eq!(bag.add_item(&stone(100)).unwrap(), 0, "and tops up, then fills another");
        assert_eq!(bag.count("minecraft:stone"), 164);
        assert!(bag.contains("minecraft:stone"));
        assert!(bag.contains_amount("minecraft:stone", 164));
        assert!(!bag.contains_amount("minecraft:stone", 165));
        assert_eq!(bag.find("minecraft:stone"), Some(0));
        assert_eq!(bag.find_all("minecraft:stone"), vec![0, 1, 2]);
        assert_eq!(bag.first_empty_slot(), Some(3));
        assert_eq!(bag.remove_item("minecraft:stone", 70).unwrap(), 70);
        assert_eq!(bag.count("minecraft:stone"), 94);
        assert_eq!(bag.remove_item("minecraft:stone", 1000).unwrap(), 94);
        assert!(!bag.contains("minecraft:stone"));

        // A full inventory leaves the rest over rather than losing it.
        let mut small = Inventory::with_size(1);
        small.set_slot(0, Some(stone(64))).unwrap();
        assert_eq!(small.add_item(&stone(9)).unwrap(), 9);
        assert!(!small.can_insert(&stone(1)));
    }

    #[test]
    fn inserting_extracting_swapping_moving_and_merging_respect_the_slots() {
        let mut bag = Inventory::hotbar();
        assert_eq!(bag.insert(0, &stone(64)).unwrap(), 0);
        assert_eq!(bag.insert(0, &stone(8)).unwrap(), 8, "the slot is full");
        assert_eq!(bag.insert(1, &stone(1)).unwrap(), 0);
        assert_eq!(bag.merge_stacks(1, 0).unwrap(), 1, "the target is full");
        assert_eq!(
            bag.insert(0, &ItemStack::new("minecraft:dirt", 1)).unwrap_err(),
            InventoryError::Mismatch,
            "a slot holding one item does not take another"
        );
        assert_eq!(
            bag.insert(2, &ItemStack::new("minecraft:dirt", 1)).unwrap(),
            0,
            "an empty slot takes anything"
        );
        let extracted = bag.extract(1).expect("slot one holds one stone");
        assert_eq!(extracted.count(), 1);
        assert_eq!(bag.extract(1).unwrap_err(), InventoryError::Empty(1));
        bag.set_slot(1, Some(stone(1))).unwrap();
        bag.swap(1, 3).unwrap();
        assert!(bag.get_slot(3).unwrap().is_some());
        let moved = bag.move_stack(3, 4).expect("an empty slot takes it");
        assert_eq!(moved.count(), 1);
        assert!(bag.get_slot(3).unwrap().is_none());
        let split = bag.split_stack(0, 20).expect("sixty-four holds twenty");
        assert_eq!(split.count(), 20);
        assert_eq!(bag.get_slot(0).unwrap().map(|stack| stack.count()), Some(44));
        assert!(bag.can_extract(0));
        assert!(!bag.can_extract(8));
    }

    #[test]
    fn a_transaction_keeps_or_puts_back_the_slots() {
        let mut bag = Inventory::player();
        bag.set_slot(0, Some(stone(64))).unwrap();
        let before = bag.revision();
        assert!(bag.begin_transaction());
        assert!(!bag.begin_transaction(), "one at a time");
        assert!(bag.has_transaction());
        bag.clear_slot(0).unwrap();
        bag.add_item(&stone(5)).unwrap();
        assert!(bag.rollback_transaction());
        assert_eq!(bag.count("minecraft:stone"), 64);
        assert!(bag.revision() > before);
        assert!(!bag.rollback_transaction(), "nothing is open now");

        bag.begin_transaction();
        bag.clear().unwrap();
        assert!(bag.commit_transaction());
        assert!(!bag.has_transaction());
        assert!(bag.is_empty());
        assert!(!bag.commit_transaction());
    }

    #[test]
    fn locking_stops_changes_until_it_is_unlocked() {
        let mut bag = Inventory::player();
        bag.set_slot(0, Some(stone(1))).unwrap();
        bag.lock();
        assert!(bag.is_locked());
        assert_eq!(bag.set_slot(0, None).unwrap_err(), InventoryError::Locked);
        assert_eq!(bag.clear().unwrap_err(), InventoryError::Locked);
        assert_eq!(bag.extract(0).unwrap_err(), InventoryError::Locked);
        bag.unlock();
        assert!(bag.set_slot(0, None).is_ok());
    }

    #[test]
    fn the_module_functions_work_on_the_mods_own_inventory() {
        let _serial = serial();
        set_local_inventory(Inventory::hotbar());
        assert_eq!(get_size(), 9);
        set_slot(0, Some(stone(10))).unwrap();
        assert!(contains("minecraft:stone"));
        assert_eq!(count("minecraft:stone"), 10);
        assert_eq!(find("minecraft:stone"), Some(0));
        assert_eq!(add_item(&stone(4)).unwrap(), 0);
        assert_eq!(get_contents().len(), 1);
        assert_eq!(extract(0).unwrap().count(), 14);
        assert!(can_insert(&stone(1)));
        assert!(!can_extract(0));
        set_contents(&[(1, stone(2))]).unwrap();
        assert_eq!(move_stack(1, 2).unwrap().count(), 2);
        set_slot(1, Some(stone(3))).unwrap();
        assert_eq!(merge_stacks(1, 2).unwrap(), 0, "both fit in one slot");
        assert_eq!(count("minecraft:stone"), 5);
        assert_eq!(find_all("minecraft:stone"), vec![2]);
        assert!(begin_transaction());
        clear().unwrap();
        assert!(rollback_transaction());
        assert_eq!(count("minecraft:stone"), 5);
        lock();
        assert!(set_slot(0, None).is_err());
        unlock();
        let revision = get_revision();
        assert!(revision > 0);
        assert!(!get_transaction());
        assert_eq!(remove_item("minecraft:stone", 1).unwrap(), 1);
        let snapshot = local_inventory();
        set_local_inventory(Inventory::player());
        assert_eq!(get_size(), PLAYER_SLOTS);
        set_local_inventory(snapshot);
    }

    #[test]
    fn a_live_inventory_and_container_are_refused_with_a_reason() {
        let _serial = serial();
        assert!(get().is_none());
        assert!(get_selected_slot().is_none());
        assert!(!set_selected_slot(3));
        assert!(get_armor().is_empty());
        assert!(get_offhand().is_none());
        assert!(get_cursor_stack().is_none());
        assert!(!set_cursor_stack(None));
        assert!(open("chest").is_none());
        assert!(!close());
        assert!(container_of("chest").is_none());
        assert!(get_slots().is_empty());
        assert!(get_properties().is_empty());
        assert!(get_viewers().is_empty());
        assert_eq!(can_interact(), None);
        assert_eq!(get_distance_limit(), None);
        assert!(get_title().is_none());
        assert!(!on_slot_click(0, 0));
        assert!(!on_property_change("burn", 1));
        assert_eq!(refusals().len(), 7);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn a_declared_container_can_be_read_back() {
        let _serial = serial();
        with_types(|types| types.clear());
        assert!(register_type(
            ContainerType::new("crate", "Crate", 27)
                .property("burn_time")
                .title("A crate")
                .distance_limit(4.0)
                .on_slot_click()
                .on_property_change()
        ));
        assert!(!register_type(ContainerType::new("crate", "Crate again", 9)));
        let declared = get_type("crate").expect("declared");
        assert_eq!(declared.slots, 9, "the second declaration replaced the first");
        assert_eq!(declared.name, "Crate again");
        assert!(!declared.on_slot_click, "and took its callbacks with it");
        assert!(!declared.registered, "the client was never told");
        assert!(register_screen_handler("crate"));
        assert!(!register_screen_handler("nothing"));
        assert!(register_slot_rule("crate", 0, "cinnaloader:lights"));
        assert!(!register_slot_rule("nothing", 0, "anything"));
        assert_eq!(get_all_types().len(), 1);
        assert!(unregister_type("crate"));
        assert!(!unregister_type("crate"));
    }

    #[test]
    fn an_inventory_renders_for_a_report() {
        let mut bag = Inventory::player();
        bag.set_slot(0, Some(stone(1))).unwrap();
        let text = bag.to_string();
        assert!(text.contains("41 slots"), "{text}");
        assert!(text.contains("1 used"), "{text}");
    }
}
