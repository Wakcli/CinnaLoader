//! Equipment: the gear a mod declares, the set it wears on its own, and the arithmetic.
//!
//! A mod declares a piece of gear — which slot it goes in, its armour points and toughness,
//! its durability, the attribute modifiers it grants, and the model and layer names it
//! ships — and every mod reads that registry back. [`Equipment`] is a value: a slot store
//! the mod owns, with equip, unequip, a slot query, the worn armour and its point arithmetic,
//! the weapon and the offhand, and a durability model where damage wears a piece down and it
//! breaks at zero rather than running negative. Attributes are real too: named values with a
//! base, a minimum and a maximum, and modifiers applied in the game's order — additive, then
//! multiply-base, then multiply-total — and clamped.
//!
//! What is out of reach is the gear the client's own player wears and the client's own item
//! stacks, both refused with a reason in [`why_unknown`].
//!
//! ```
//! use cinnaloader::equipment::{self, Equipment, EquipmentSlot, GearType};
//! use cinnaloader::items::ItemStack;
//!
//! assert!(equipment::register_gear(
//!     GearType::new("hello:vest", "Vest").slot(EquipmentSlot::Chest).armor(6.0)
//! ));
//! let mut worn = Equipment::new();
//! assert!(worn.equip(ItemStack::new("hello:vest", 1), EquipmentSlot::Chest));
//! assert_eq!(worn.get_armor_points(), 6.0);
//! assert!(equipment::get().is_none(), "the client wears its own");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::components::{Attribute, AttributeModifier, ModifierKind};
use crate::items::{Component, ItemStack};

/// Where a piece of gear sits when it is worn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EquipmentSlot {
    Head,
    Chest,
    Legs,
    Feet,
    Mainhand,
    Offhand,
}

impl EquipmentSlot {
    /// The four armour slots, in the order the client lists them.
    pub const ARMOR: [EquipmentSlot; 4] = [
        EquipmentSlot::Feet,
        EquipmentSlot::Legs,
        EquipmentSlot::Chest,
        EquipmentSlot::Head,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Chest => "chest",
            Self::Legs => "legs",
            Self::Feet => "feet",
            Self::Mainhand => "mainhand",
            Self::Offhand => "offhand",
        }
    }

    /// Whether it is one of the four armour slots.
    #[must_use]
    pub fn is_armor(self) -> bool {
        matches!(self, Self::Head | Self::Chest | Self::Legs | Self::Feet)
    }
}

impl fmt::Display for EquipmentSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A modifier a piece of gear grants to a named attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeGrant {
    /// The attribute it changes.
    pub attribute: String,
    /// The gear it comes from, which is also the modifier's name.
    pub source: String,
    pub kind: ModifierKind,
    pub amount: f64,
}

impl AttributeGrant {
    #[must_use]
    pub fn new(attribute: &str, source: &str, kind: ModifierKind, amount: f64) -> Self {
        Self {
            attribute: attribute.to_owned(),
            source: source.to_owned(),
            kind,
            amount,
        }
    }
}

/// A piece of gear a mod declares.
#[derive(Debug, Clone, PartialEq)]
pub struct GearType {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub slot: EquipmentSlot,
    pub armor: f64,
    pub toughness: f64,
    pub durability: i32,
    pub attributes: Vec<AttributeGrant>,
    pub model: String,
    pub layer: String,
    pub rules: Vec<String>,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered: bool,
}

impl GearType {
    /// A piece of gear with the defaults a caller would expect.
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            slot: EquipmentSlot::Mainhand,
            armor: 0.0,
            toughness: 0.0,
            durability: 0,
            attributes: Vec::new(),
            model: String::new(),
            layer: String::new(),
            rules: Vec::new(),
            registered: false,
        }
    }

    #[must_use]
    pub fn slot(mut self, slot: EquipmentSlot) -> Self {
        self.slot = slot;
        self
    }

    #[must_use]
    pub fn armor(mut self, armor: f64) -> Self {
        self.armor = armor.max(0.0);
        self
    }

    #[must_use]
    pub fn toughness(mut self, toughness: f64) -> Self {
        self.toughness = toughness.max(0.0);
        self
    }

    #[must_use]
    pub fn durability(mut self, durability: i32) -> Self {
        self.durability = durability.max(0);
        self
    }

    #[must_use]
    pub fn attribute(mut self, grant: AttributeGrant) -> Self {
        self.attributes.retain(|held| held.source != grant.source);
        self.attributes.push(grant);
        self
    }

    #[must_use]
    pub fn model(mut self, model: &str) -> Self {
        self.model = model.to_owned();
        self
    }

    #[must_use]
    pub fn layer(mut self, layer: &str) -> Self {
        self.layer = layer.to_owned();
        self
    }

    /// A stack of this gear, carrying its declared durability.
    #[must_use]
    pub fn stack(&self, count: u32) -> ItemStack {
        let mut stack = ItemStack::new(&self.id, count);
        if self.durability > 0 {
            stack.set_component(Component::new("minecraft:durability", &self.durability.to_string()));
        }
        stack
    }
}

impl fmt::Display for GearType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}, {:.1} armour)",
            self.name,
            self.id,
            self.slot.as_str(),
            self.armor
        )
    }
}

fn gear() -> &'static Mutex<BTreeMap<String, GearType>> {
    static GEAR: OnceLock<Mutex<BTreeMap<String, GearType>>> = OnceLock::new();
    GEAR.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_gear<T>(body: impl FnOnce(&mut BTreeMap<String, GearType>) -> T) -> T {
    let mut guard = gear().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a piece of gear, or replaces the one with that id.
pub fn register_gear(piece: GearType) -> bool {
    with_gear(|gear| gear.insert(piece.id.clone(), piece).is_none())
}

/// Forgets a declared piece of gear.
pub fn unregister_gear(id: &str) -> bool {
    with_gear(|gear| gear.remove(id).is_some())
}

/// A declared piece of gear, by id.
#[must_use]
pub fn get_gear(id: &str) -> Option<GearType> {
    with_gear(|gear| gear.get(id).cloned())
}

/// Every declared piece of gear, in id order.
#[must_use]
pub fn get_all_gear() -> Vec<GearType> {
    with_gear(|gear| gear.values().cloned().collect())
}

/// Records which slot a declared piece of gear goes in.
pub fn register_slot(id: &str, slot: EquipmentSlot) -> bool {
    with_gear(|gear| match gear.get_mut(id) {
        Some(piece) => {
            piece.slot = slot;
            true
        }
        None => false,
    })
}

/// Records an equip rule on a declared piece of gear, which a mod applies itself.
pub fn register_rule(id: &str, rule: &str) -> bool {
    with_gear(|gear| match gear.get_mut(id) {
        Some(piece) => {
            piece.rules.push(rule.to_owned());
            true
        }
        None => false,
    })
}

/// Records the model name a declared piece of gear ships. The client's own model is not
/// reachable.
pub fn register_model(id: &str, model: &str) -> bool {
    with_gear(|gear| match gear.get_mut(id) {
        Some(piece) => {
            piece.model = model.to_owned();
            true
        }
        None => false,
    })
}

/// Records the armour layer name a declared piece of gear ships.
pub fn register_layer(id: &str, layer: &str) -> bool {
    with_gear(|gear| match gear.get_mut(id) {
        Some(piece) => {
            piece.layer = layer.to_owned();
            true
        }
        None => false,
    })
}

/// A set of gear a mod owns: a value, never the player's.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Equipment {
    slots: BTreeMap<EquipmentSlot, ItemStack>,
}

impl Equipment {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many slots hold something.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// What is worn in a slot.
    #[must_use]
    pub fn get_slot(&self, slot: EquipmentSlot) -> Option<ItemStack> {
        self.slots.get(&slot).cloned()
    }

    /// Whether a stack may go in a slot: declared gear only goes in its own slot, and an
    /// undeclared item may be held in a hand.
    #[must_use]
    pub fn can_equip(&self, stack: &ItemStack, slot: EquipmentSlot) -> bool {
        if stack.is_empty() {
            return false;
        }
        match get_gear(stack.item()) {
            Some(piece) => piece.slot == slot,
            None => matches!(slot, EquipmentSlot::Mainhand | EquipmentSlot::Offhand),
        }
    }

    /// Puts a stack in a slot, or empties it with `None`, refusing one the slot will not take.
    pub fn set_slot(&mut self, slot: EquipmentSlot, stack: Option<ItemStack>) -> bool {
        match stack.filter(|stack| !stack.is_empty()) {
            Some(stack) => {
                if !self.can_equip(&stack, slot) {
                    return false;
                }
                self.slots.insert(slot, with_declared_durability(stack));
                true
            }
            None => {
                self.slots.remove(&slot);
                true
            }
        }
    }

    /// Equips a stack, when the slot will take it.
    pub fn equip(&mut self, stack: ItemStack, slot: EquipmentSlot) -> bool {
        self.set_slot(slot, Some(stack))
    }

    /// Takes a stack off, and answers what it was.
    pub fn unequip(&mut self, slot: EquipmentSlot) -> Option<ItemStack> {
        self.slots.remove(&slot)
    }

    /// Every worn piece, in slot order.
    #[must_use]
    pub fn get_equipped_items(&self) -> Vec<(EquipmentSlot, ItemStack)> {
        self.slots
            .iter()
            .map(|(slot, stack)| (*slot, stack.clone()))
            .collect()
    }

    /// The armour pieces, in the client's armour order.
    #[must_use]
    pub fn get_armor(&self) -> Vec<ItemStack> {
        EquipmentSlot::ARMOR
            .iter()
            .filter_map(|slot| self.slots.get(slot).cloned())
            .collect()
    }

    /// The armour points the worn armour adds up to.
    #[must_use]
    pub fn get_armor_points(&self) -> f64 {
        // std seeds a float sum with -0.0, which is a poor answer to show for no armour
        let points: f64 = EquipmentSlot::ARMOR
            .iter()
            .filter_map(|slot| self.slots.get(slot))
            .filter_map(|stack| get_gear(stack.item()).map(|piece| piece.armor))
            .sum();
        if points == 0.0 {
            0.0
        } else {
            points
        }
    }

    /// The weapon in the main hand.
    #[must_use]
    pub fn get_weapon(&self) -> Option<ItemStack> {
        self.slots.get(&EquipmentSlot::Mainhand).cloned()
    }

    /// What is in the offhand.
    #[must_use]
    pub fn get_offhand(&self) -> Option<ItemStack> {
        self.slots.get(&EquipmentSlot::Offhand).cloned()
    }

    /// How much use is left in a slot's piece.
    #[must_use]
    pub fn durability(&self, slot: EquipmentSlot) -> Option<i32> {
        self.slots.get(&slot).and_then(ItemStack::durability)
    }

    /// Wears a piece down and answers whether it broke. At zero it is removed rather than
    /// left at a negative durability.
    pub fn damage(&mut self, slot: EquipmentSlot, amount: i32) -> bool {
        let Some(held) = self.slots.get_mut(&slot) else {
            return false;
        };
        let Some(max) = held.max_damage() else {
            return false;
        };
        let worn = held.damage().unwrap_or(0).saturating_add(amount.max(0));
        if worn >= max {
            self.slots.remove(&slot);
            return true;
        }
        held.set_damage(worn);
        false
    }

    /// Every attribute modifier the worn gear grants.
    #[must_use]
    pub fn get_attribute_modifiers(&self) -> Vec<AttributeGrant> {
        self.slots
            .values()
            .filter_map(|stack| get_gear(stack.item()))
            .flat_map(|piece| piece.attributes)
            .collect()
    }

    /// An attribute's value with the worn gear's modifiers applied in the game's order and
    /// clamped between the bounds.
    #[must_use]
    pub fn attribute_value(
        &self,
        attribute: &str,
        base: f64,
        minimum: f64,
        maximum: f64,
    ) -> f64 {
        let mut held = Attribute::new(attribute, base, minimum, maximum);
        for grant in self.get_attribute_modifiers() {
            if grant.attribute == attribute {
                held.add_modifier(AttributeModifier {
                    name: grant.source,
                    kind: grant.kind,
                    amount: grant.amount,
                });
            }
        }
        held.current()
    }
}

impl fmt::Display for Equipment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} piece(s), {:.1} armour",
            self.len(),
            self.get_armor_points()
        )
    }
}

fn with_declared_durability(mut stack: ItemStack) -> ItemStack {
    let declared = get_gear(stack.item()).map_or(0, |piece| piece.durability);
    if stack.max_damage().is_none() && declared > 0 {
        stack.set_component(Component::new("minecraft:durability", &declared.to_string()));
    }
    stack
}

fn local() -> &'static Mutex<Equipment> {
    static LOCAL: OnceLock<Mutex<Equipment>> = OnceLock::new();
    LOCAL.get_or_init(|| Mutex::new(Equipment::new()))
}

fn with<T>(body: impl FnOnce(&mut Equipment) -> T) -> T {
    let mut guard = local().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The mod's own equipment, for a caller that wants to hold it.
#[must_use]
pub fn local_equipment() -> Equipment {
    with(|equipment| equipment.clone())
}

/// Replaces the mod's equipment with another.
pub fn set_local_equipment(equipment: Equipment) {
    with(|held| *held = equipment);
}

/// Binds the player's live equipment. This host cannot: the client wears it.
#[must_use]
pub fn get() -> Option<Equipment> {
    None
}

/// What is worn in a slot of the mod's own equipment.
#[must_use]
pub fn get_slot(slot: EquipmentSlot) -> Option<ItemStack> {
    with(|equipment| equipment.get_slot(slot))
}

/// Puts a stack in a slot of the mod's own equipment.
pub fn set_slot(slot: EquipmentSlot, stack: Option<ItemStack>) -> bool {
    with(|equipment| equipment.set_slot(slot, stack))
}

/// Equips a stack on the mod's own equipment.
pub fn equip(stack: ItemStack, slot: EquipmentSlot) -> bool {
    with(|equipment| equipment.equip(stack, slot))
}

/// Takes a stack off the mod's own equipment.
pub fn unequip(slot: EquipmentSlot) -> Option<ItemStack> {
    with(|equipment| equipment.unequip(slot))
}

/// The armour on the mod's own equipment.
#[must_use]
pub fn get_armor() -> Vec<ItemStack> {
    with(|equipment| equipment.get_armor())
}

/// The armour points of the mod's own equipment.
#[must_use]
pub fn get_armor_points() -> f64 {
    with(|equipment| equipment.get_armor_points())
}

/// The weapon on the mod's own equipment.
#[must_use]
pub fn get_weapon() -> Option<ItemStack> {
    with(|equipment| equipment.get_weapon())
}

/// The offhand of the mod's own equipment.
#[must_use]
pub fn get_offhand() -> Option<ItemStack> {
    with(|equipment| equipment.get_offhand())
}

/// Every worn piece of the mod's own equipment.
#[must_use]
pub fn get_equipped_items() -> Vec<(EquipmentSlot, ItemStack)> {
    with(|equipment| equipment.get_equipped_items())
}

/// Whether the mod's own equipment will take a stack in a slot.
#[must_use]
pub fn can_equip(stack: &ItemStack, slot: EquipmentSlot) -> bool {
    with(|equipment| equipment.can_equip(stack, slot))
}

/// Wears a piece of the mod's own equipment down, and answers whether it broke.
pub fn damage(slot: EquipmentSlot, amount: i32) -> bool {
    with(|equipment| equipment.damage(slot, amount))
}

/// How much use is left in a slot of the mod's own equipment.
#[must_use]
pub fn durability(slot: EquipmentSlot) -> Option<i32> {
    with(|equipment| equipment.durability(slot))
}

/// Every attribute modifier the mod's own equipment grants.
#[must_use]
pub fn get_attribute_modifiers() -> Vec<AttributeGrant> {
    with(|equipment| equipment.get_attribute_modifiers())
}

/// An attribute's value from the mod's own equipment, applied in order and clamped.
#[must_use]
pub fn attribute_value(attribute: &str, base: f64, minimum: f64, maximum: f64) -> f64 {
    with(|equipment| equipment.attribute_value(attribute, base, minimum, maximum))
}

/// Why an equipment question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "slot" => {
            "the gear the client's player wears is the client's own player state, which this \
             host cannot read"
                .to_owned()
        }
        "stack" => {
            "an item stack the client or the server owns is not this host's to equip"
                .to_owned()
        }
        "model" => {
            "a Bedrock armour model is the client's own asset; a layer name a mod ships is \
             not one"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The gear the client's player wears in a slot. `None`: the client owns it.
#[must_use]
pub fn read_client_slot(_slot: EquipmentSlot) -> Option<ItemStack> {
    crate::warn!(
        "a mod asked this host for the client's equipment: {}",
        why_unknown("slot")
    );
    None
}

/// Equips a stack on the client's player. This host cannot.
pub fn equip_on_client(_stack: &ItemStack, _slot: EquipmentSlot) -> bool {
    crate::warn!(
        "a mod asked this host to equip on the client: {}",
        why_unknown("stack")
    );
    false
}

/// The model the client draws for a piece of gear. `None`: that is the client's asset.
#[must_use]
pub fn get_client_model(_id: &str) -> Option<String> {
    crate::warn!(
        "a mod asked this host for the client's gear model: {}",
        why_unknown("model")
    );
    None
}

/// What this host cannot reach about equipment.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["slot", "stack", "model"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[test]
fn no_armour_reports_a_positive_zero() {
    let _serial = crate::test_lock::serial();
    let bare = Equipment::new();
    assert!(
        bare.get_armor_points().is_sign_positive(),
        "an empty sum must not answer with a negative zero"
    );
    assert_eq!(bare.get_armor_points(), 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh_gear() {
        with_gear(|gear| gear.clear());
    }

    fn grant(source: &str, attribute: &str, kind: ModifierKind, amount: f64) -> AttributeGrant {
        AttributeGrant::new(attribute, source, kind, amount)
    }

    #[test]
    fn a_gear_piece_is_declared_and_read_back() {
        let _serial = serial();
        fresh_gear();
        assert!(register_gear(
            GearType::new("hello:plate", "Plate")
                .slot(EquipmentSlot::Chest)
                .armor(8.0)
                .toughness(2.0)
                .durability(80)
                .model("hello:plate_model")
                .layer("hello:plate_layer")
        ));
        let piece = get_gear("hello:plate").expect("declared");
        assert_eq!(piece.name, "Plate");
        assert_eq!(piece.slot, EquipmentSlot::Chest);
        assert_eq!(piece.armor, 8.0);
        assert_eq!(piece.toughness, 2.0);
        assert_eq!(piece.durability, 80);
        assert_eq!(piece.model, "hello:plate_model");
        assert_eq!(piece.layer, "hello:plate_layer");
        assert!(!piece.registered, "the client was never told");
        assert_eq!(get_all_gear().len(), 1);
        assert!(piece.to_string().contains("Plate"), "{piece}");

        assert!(register_slot("hello:plate", EquipmentSlot::Head));
        assert_eq!(get_gear("hello:plate").map(|held| held.slot), Some(EquipmentSlot::Head));
        assert!(register_rule("hello:plate", "requires:hello:focus"));
        assert!(register_model("hello:plate", "hello:other_model"));
        assert!(register_layer("hello:plate", "hello:other_layer"));
        assert!(!register_slot("nothing", EquipmentSlot::Head));
        assert!(!register_rule("nothing", "x"));
        assert!(!register_model("nothing", "x"));
        assert!(!register_layer("nothing", "x"));
        assert_eq!(get_gear("hello:plate").map(|held| held.rules.len()), Some(1));
        assert!(!register_gear(GearType::new("hello:plate", "Plate again")), "it replaces");
        assert!(unregister_gear("hello:plate"));
        assert!(!unregister_gear("hello:plate"));
    }

    #[test]
    fn gear_equips_and_unequips_by_slot() {
        let _serial = serial();
        fresh_gear();
        register_gear(GearType::new("hello:plate", "Plate").slot(EquipmentSlot::Chest));
        register_gear(GearType::new("hello:sword", "Sword").slot(EquipmentSlot::Mainhand));

        let mut worn = Equipment::new();
        assert!(worn.is_empty());
        assert!(worn.equip(ItemStack::new("hello:plate", 1), EquipmentSlot::Chest));
        assert!(!worn.equip(ItemStack::new("hello:plate", 1), EquipmentSlot::Head), "wrong slot");
        assert_eq!(worn.len(), 1);
        assert_eq!(worn.get_slot(EquipmentSlot::Chest).map(|stack| stack.item().to_owned()), Some("hello:plate".to_owned()));
        assert!(worn.can_equip(&ItemStack::new("hello:sword", 1), EquipmentSlot::Mainhand));
        assert!(!worn.can_equip(&ItemStack::new("minecraft:dirt", 1), EquipmentSlot::Head), "dirt is not headgear");
        assert!(worn.can_equip(&ItemStack::new("minecraft:dirt", 1), EquipmentSlot::Offhand), "a hand takes anything");
        assert_eq!(worn.get_equipped_items().len(), 1);
        assert_eq!(
            worn.unequip(EquipmentSlot::Chest).map(|stack| stack.item().to_owned()),
            Some("hello:plate".to_owned())
        );
        assert!(worn.unequip(EquipmentSlot::Chest).is_none());
        assert!(worn.set_slot(EquipmentSlot::Mainhand, None));
        assert!(worn.is_empty());
    }

    #[test]
    fn armor_points_add_up() {
        let _serial = serial();
        fresh_gear();
        register_gear(GearType::new("hello:helm", "Helm").slot(EquipmentSlot::Head).armor(3.0));
        register_gear(GearType::new("hello:plate", "Plate").slot(EquipmentSlot::Chest).armor(8.0).toughness(2.0));
        register_gear(GearType::new("hello:legs", "Legs").slot(EquipmentSlot::Legs).armor(6.0));
        register_gear(GearType::new("hello:boots", "Boots").slot(EquipmentSlot::Feet).armor(3.0));

        let mut worn = Equipment::new();
        assert_eq!(worn.get_armor_points(), 0.0);
        assert!(worn.equip(ItemStack::new("hello:helm", 1), EquipmentSlot::Head));
        assert_eq!(worn.get_armor_points(), 3.0);
        assert!(worn.equip(ItemStack::new("hello:plate", 1), EquipmentSlot::Chest));
        assert!(worn.equip(ItemStack::new("hello:legs", 1), EquipmentSlot::Legs));
        assert!(worn.equip(ItemStack::new("hello:boots", 1), EquipmentSlot::Feet));
        assert_eq!(worn.get_armor_points(), 20.0, "3 + 8 + 6 + 3");
        assert_eq!(worn.get_armor().len(), 4);
        assert!(worn.unequip(EquipmentSlot::Legs).is_some());
        assert_eq!(worn.get_armor_points(), 14.0);
    }

    #[test]
    fn damage_breaks_a_piece_at_zero_rather_than_going_negative() {
        let _serial = serial();
        fresh_gear();
        register_gear(GearType::new("hello:pick", "Pick").slot(EquipmentSlot::Mainhand).durability(10));
        let mut worn = Equipment::new();
        assert!(worn.equip(ItemStack::new("hello:pick", 1), EquipmentSlot::Mainhand));
        assert_eq!(worn.durability(EquipmentSlot::Mainhand), Some(10));
        assert!(!worn.damage(EquipmentSlot::Mainhand, 4), "not broken yet");
        assert_eq!(worn.durability(EquipmentSlot::Mainhand), Some(6));
        assert_eq!(worn.get_slot(EquipmentSlot::Mainhand).and_then(|stack| stack.damage()), Some(4));
        assert!(worn.damage(EquipmentSlot::Mainhand, 6), "ten of ten is zero");
        assert!(worn.get_slot(EquipmentSlot::Mainhand).is_none(), "it is gone, not negative");
        assert_eq!(worn.durability(EquipmentSlot::Mainhand), None);
        assert!(!worn.damage(EquipmentSlot::Mainhand, 1), "nothing left to break");
        assert!(!worn.damage(EquipmentSlot::Offhand, 1), "nothing there");
    }

    #[test]
    fn attribute_modifiers_apply_in_order_and_clamp() {
        let _serial = serial();
        fresh_gear();
        register_gear(
            GearType::new("hello:add", "Add")
                .slot(EquipmentSlot::Mainhand)
                .attribute(grant("hello:add", "hello:power", ModifierKind::Add, 5.0)),
        );
        register_gear(
            GearType::new("hello:mul", "Mul")
                .slot(EquipmentSlot::Offhand)
                .attribute(grant("hello:mul", "hello:power", ModifierKind::Multiply, 0.5)),
        );
        register_gear(
            GearType::new("hello:total", "Total")
                .slot(EquipmentSlot::Chest)
                .attribute(grant("hello:total", "hello:power", ModifierKind::MultiplyTotal, 1.0)),
        );

        let mut worn = Equipment::new();
        assert!(worn.equip(ItemStack::new("hello:add", 1), EquipmentSlot::Mainhand));
        assert_eq!(worn.attribute_value("hello:power", 10.0, 0.0, 100.0), 15.0, "additive first");
        assert!(worn.equip(ItemStack::new("hello:mul", 1), EquipmentSlot::Offhand));
        assert_eq!(worn.attribute_value("hello:power", 10.0, 0.0, 100.0), 22.5, "then multiply-base");
        assert!(worn.equip(ItemStack::new("hello:total", 1), EquipmentSlot::Chest));
        assert_eq!(worn.attribute_value("hello:power", 10.0, 0.0, 100.0), 45.0, "then multiply-total");
        assert_eq!(worn.attribute_value("hello:power", 10.0, 0.0, 20.0), 20.0, "clamped to the maximum");
        assert_eq!(worn.attribute_value("hello:power", 10.0, 0.0, 100.0), 45.0, "unrelated bounds change nothing");
        assert_eq!(worn.get_attribute_modifiers().len(), 3);
        assert_eq!(worn.attribute_value("hello:haste", 1.0, 0.0, 10.0), 1.0, "no gear touches it");
    }

    #[test]
    fn the_module_functions_work_on_the_mods_own_gear() {
        let _serial = serial();
        fresh_gear();
        set_local_equipment(Equipment::new());
        register_gear(GearType::new("hello:sword", "Sword").slot(EquipmentSlot::Mainhand).durability(5));
        register_gear(GearType::new("hello:shield", "Shield").slot(EquipmentSlot::Offhand));
        assert!(equip(ItemStack::new("hello:sword", 1), EquipmentSlot::Mainhand));
        assert!(equip(ItemStack::new("hello:shield", 1), EquipmentSlot::Offhand));
        assert_eq!(get_weapon().map(|stack| stack.item().to_owned()), Some("hello:sword".to_owned()));
        assert_eq!(get_offhand().map(|stack| stack.item().to_owned()), Some("hello:shield".to_owned()));
        assert_eq!(get_equipped_items().len(), 2);
        assert_eq!(durability(EquipmentSlot::Mainhand), Some(5));
        assert!(can_equip(&ItemStack::new("hello:shield", 1), EquipmentSlot::Offhand));
        assert!(damage(EquipmentSlot::Mainhand, 5), "five of five breaks it");
        assert!(get_slot(EquipmentSlot::Mainhand).is_none());
        assert_eq!(
            unequip(EquipmentSlot::Offhand).map(|stack| stack.item().to_owned()),
            Some("hello:shield".to_owned())
        );
        assert!(get_equipped_items().is_empty());
        assert!(!set_slot(EquipmentSlot::Head, Some(ItemStack::new("minecraft:dirt", 1))));
        assert!(get_slot(EquipmentSlot::Head).is_none(), "dirt is not headgear");
        assert!(get_armor().is_empty());
        let snapshot = local_equipment();
        assert_eq!(get_armor_points(), 0.0);
        set_local_equipment(Equipment::new());
        assert!(snapshot.is_empty());
        set_local_equipment(snapshot);
    }

    #[test]
    fn what_the_client_wears_is_refused_with_a_reason() {
        let _serial = serial();
        assert!(get().is_none());
        assert!(read_client_slot(EquipmentSlot::Chest).is_none());
        assert!(!equip_on_client(
            &ItemStack::new("minecraft:diamond_chestplate", 1),
            EquipmentSlot::Chest
        ));
        assert!(get_client_model("hello:plate").is_none());
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
