//! Trading: the offers a mod declares, and the matching it can run for itself.
//!
//! An offer is data — its inputs, its outputs, how many times it may be used, what it is worth
//! in experience and how its price moves — and a mod registers them here. What is *not* just
//! data is matching: [`validate_trade`], [`find_offer`] and [`execute_trade`] run over an
//! [`Inventory`], which is section 18's value type, so a mod trades in this host exactly as
//! the game does. An offer whose inputs are missing really is refused, the use count really
//! falls, and a used-up offer really comes back after its restock cooldown has been ticked
//! down rather than being handed back early.
//!
//! The price is real too: the first input's count scaled by the offer's multiplier and cut by
//! the player's reputation, never below one, and [`set_price`] moves the multiplier so a
//! caller's number comes back out of [`get_price`].
//!
//! Refused: the client's villagers, their live offers and the trade screen — those are the
//! client's entities, the server's trades and the client's UI, none of which this host reaches.
//!
//! ```
//! use cinnaloader::inventory::Inventory;
//! use cinnaloader::items::ItemStack;
//! use cinnaloader::trading;
//!
//! trading::add_offer(trading::TradeOffer::new("hello:emeralds", "farmer")
//!     .input(ItemStack::new("minecraft:wheat", 20))
//!     .output(ItemStack::new("minecraft:emerald", 1))
//!     .max_uses(3));
//! let mut inv = Inventory::player();
//! inv.set_slot(0, Some(ItemStack::new("minecraft:wheat", 20))).unwrap();
//! assert!(trading::validate_trade("hello:emeralds", &inv).is_ok());
//! let receipt = trading::execute_trade("hello:emeralds", &mut inv).expect("the wheat is there");
//! assert_eq!(receipt.uses_left, 2);
//! assert!(!inv.contains("minecraft:wheat"), "the wheat was taken");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events;
use crate::inventory::Inventory;
use crate::items::ItemStack;

/// The trade events this host would have to receive from the client to fire.
pub const TRADE_EVENTS: [&str; 6] = [
    "trade_start",
    "trade_complete",
    "trade_offer_added",
    "trade_offer_removed",
    "trade_restock",
    "reputation_changed",
];

/// Teaches the event bus each trade event name, so a subscription is in place before the hooks
/// land.
pub fn register_event_kinds() -> usize {
    TRADE_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// The event kinds this host would need from the client to fire a trade callback.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    TRADE_EVENTS.to_vec()
}

/// Whether a trade event kind is one the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    TRADE_EVENTS.contains(&kind) && events::is_registered_kind(kind)
}

/// Why a trade question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "villager" => {
            "the client's villagers are entities it keeps to itself; a mod's own offers live \
             in this registry"
                .to_owned()
        }
        "offers" => {
            "a villager's live offers are the server's trades, held by the client's trade \
             screen, which this host cannot read"
                .to_owned()
        }
        "screen" => "the trade screen is the client's own UI, which this host can neither open nor drive".to_owned(),
        "live" => {
            "executing a trade against a live villager is the server's to accept; this host \
             trades into an inventory a mod owns"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in trading, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["villager", "offers", "screen", "live"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about a live trade: {}", why_unknown(what));
    false
}

/// One trade a mod declares: what it takes, what it gives, and how its price behaves.
#[derive(Debug, Clone, PartialEq)]
pub struct TradeOffer {
    pub id: String,
    pub profession: String,
    pub owner: String,
    pub inputs: Vec<ItemStack>,
    pub outputs: Vec<ItemStack>,
    /// How many times it may be used before it has to restock.
    pub max_uses: u32,
    /// How many uses are left, which falls as the game uses them.
    pub uses: u32,
    pub xp: u32,
    /// Scales the first input's count, above the flat reputation discount.
    pub price_multiplier: f32,
    /// How long the restock takes, in ticks, and how many are left of it.
    pub restock_ticks: u32,
    pub cooldown: u32,
}

impl TradeOffer {
    /// An offer with the defaults a caller would expect: one profession, three uses.
    #[must_use]
    pub fn new(id: &str, profession: &str) -> Self {
        Self {
            id: id.to_owned(),
            profession: profession.to_owned(),
            owner: crate::metadata::id(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            max_uses: 3,
            uses: 3,
            xp: 0,
            price_multiplier: 1.0,
            restock_ticks: 6000,
            cooldown: 0,
        }
    }

    #[must_use]
    pub fn input(mut self, stack: ItemStack) -> Self {
        self.inputs.push(stack);
        self
    }

    #[must_use]
    pub fn output(mut self, stack: ItemStack) -> Self {
        self.outputs.push(stack);
        self
    }

    #[must_use]
    pub fn max_uses(mut self, uses: u32) -> Self {
        self.max_uses = uses.max(1);
        self.uses = self.max_uses;
        self
    }

    #[must_use]
    pub fn xp(mut self, xp: u32) -> Self {
        self.xp = xp;
        self
    }

    #[must_use]
    pub fn price_multiplier(mut self, multiplier: f32) -> Self {
        self.price_multiplier = multiplier.max(0.0);
        self
    }

    #[must_use]
    pub fn restock(mut self, ticks: u32) -> Self {
        self.restock_ticks = ticks;
        self
    }

    /// Whether the offer has the uses and the cooldown to be traded right now.
    #[must_use]
    pub fn available(&self) -> bool {
        self.uses > 0 && self.cooldown == 0
    }

    /// Whether an inventory holds every input this offer wants.
    #[must_use]
    pub fn accepts(&self, inventory: &Inventory) -> bool {
        self.inputs
            .iter()
            .all(|stack| inventory.contains_amount(stack.item(), stack.count()))
    }
}

impl fmt::Display for TradeOffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {} uses left, {} -> {})",
            self.id,
            self.profession,
            self.uses,
            self.inputs
                .iter()
                .map(|stack| stack.display_name())
                .collect::<Vec<_>>()
                .join("+"),
            self.outputs
                .iter()
                .map(|stack| stack.display_name())
                .collect::<Vec<_>>()
                .join("+"),
        )
    }
}

/// Why a trade would not go through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeRefusal {
    UnknownOffer,
    Restocking { cooldown: u32 },
    MissingInputs { item: String, needed: u32, held: u32 },
}

impl fmt::Display for TradeRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownOffer => formatter.write_str("no such offer"),
            Self::Restocking { cooldown } => write!(formatter, "restocking, {cooldown} ticks left"),
            Self::MissingInputs { item, needed, held } => {
                write!(formatter, "missing {item}: need {needed}, hold {held}")
            }
        }
    }
}

/// What actually moved in a trade, so a caller can log the effect rather than a boolean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeReceipt {
    pub offer: String,
    pub uses_left: u32,
    pub price: u32,
    pub xp: u32,
    pub given: Vec<ItemStack>,
    pub got: Vec<ItemStack>,
}

fn offers() -> &'static Mutex<BTreeMap<String, TradeOffer>> {
    static OFFERS: OnceLock<Mutex<BTreeMap<String, TradeOffer>>> = OnceLock::new();
    OFFERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, TradeOffer>) -> T) -> T {
    let mut guard = offers().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn reputation() -> &'static Mutex<i32> {
    static REPUTATION: OnceLock<Mutex<i32>> = OnceLock::new();
    REPUTATION.get_or_init(|| Mutex::new(0))
}

fn with_reputation<T>(body: impl FnOnce(&mut i32) -> T) -> T {
    let mut guard = reputation().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers an offer, or replaces the one with that id.
pub fn add_offer(offer: TradeOffer) -> bool {
    with(|offers| offers.insert(offer.id.clone(), offer).is_none())
}

/// The same as [`add_offer`], under the name the wishlist also uses.
pub fn register_offer(offer: TradeOffer) -> bool {
    add_offer(offer)
}

/// Forgets an offer.
pub fn remove_offer(id: &str) -> bool {
    with(|offers| offers.remove(id).is_some())
}

/// Everything a mod declared, in id order.
#[must_use]
pub fn get_offers() -> Vec<TradeOffer> {
    with(|offers| offers.values().cloned().collect())
}

/// Everything one profession offers, in id order.
#[must_use]
pub fn get_offers_for_profession(profession: &str) -> Vec<TradeOffer> {
    with(|offers| {
        offers
            .values()
            .filter(|offer| offer.profession == profession)
            .cloned()
            .collect()
    })
}

/// One offer by id.
#[must_use]
pub fn get_offer(id: &str) -> Option<TradeOffer> {
    with(|offers| offers.get(id).cloned())
}

/// Changes an offer in place, and answers whether there was one.
pub fn modify_offer(id: &str, change: impl FnOnce(&mut TradeOffer)) -> bool {
    with(|offers| match offers.get_mut(id) {
        Some(offer) => {
            change(offer);
            true
        }
        None => false,
    })
}

/// The first offer an inventory can pay for and use, in id order.
#[must_use]
pub fn find_offer(inventory: &Inventory) -> Option<TradeOffer> {
    with(|offers| {
        offers
            .values()
            .find(|offer| offer.available() && offer.accepts(inventory))
            .cloned()
    })
}

/// Whether a trade would go through, and what stopped it when it would not.
pub fn validate_trade(id: &str, inventory: &Inventory) -> Result<(), TradeRefusal> {
    let Some(offer) = get_offer(id) else {
        return Err(TradeRefusal::UnknownOffer);
    };
    if offer.uses == 0 || offer.cooldown > 0 {
        return Err(TradeRefusal::Restocking {
            cooldown: offer.cooldown,
        });
    }
    for stack in &offer.inputs {
        let held = inventory.count(stack.item());
        if held < stack.count() {
            return Err(TradeRefusal::MissingInputs {
                item: stack.item().to_owned(),
                needed: stack.count(),
                held,
            });
        }
    }
    Ok(())
}

/// Runs a trade: takes the inputs from the inventory, gives the outputs back, and spends one
/// use. `None` when [`validate_trade`] would have refused.
pub fn execute_trade(id: &str, inventory: &mut Inventory) -> Option<TradeReceipt> {
    validate_trade(id, inventory).ok()?;
    let mut offer = get_offer(id)?;
    let price = get_price(id)?;
    for stack in &offer.inputs {
        inventory.remove_item(stack.item(), stack.count()).ok()?;
    }
    for stack in &offer.outputs {
        inventory.add_item(stack).ok()?;
    }
    offer.uses -= 1;
    if offer.uses == 0 {
        offer.cooldown = offer.restock_ticks;
    }
    let uses_left = offer.uses;
    let xp = offer.xp;
    let given = offer.inputs.clone();
    let got = offer.outputs.clone();
    with(|offers| offers.insert(id.to_owned(), offer));
    Some(TradeReceipt {
        offer: id.to_owned(),
        uses_left,
        price,
        xp,
        given,
        got,
    })
}

/// Advances every restock cooldown by `ticks`, and answers how many offers came back.
pub fn tick_trades(ticks: u32) -> usize {
    let mut restocked = 0;
    with(|offers| {
        for offer in offers.values_mut() {
            if offer.cooldown == 0 {
                continue;
            }
            offer.cooldown = offer.cooldown.saturating_sub(ticks);
            if offer.cooldown == 0 {
                offer.uses = offer.max_uses;
                restocked += 1;
            }
        }
    });
    restocked
}

fn adjusted(offer: &TradeOffer, reputation: i32) -> u32 {
    let base = offer.inputs.first().map_or(0, ItemStack::count) as f32;
    let scaled = (base * offer.price_multiplier).round();
    let discount = (reputation.max(0) / 10) as f32;
    (scaled - discount).max(1.0) as u32
}

/// What the first input really costs right now, after the multiplier and reputation.
#[must_use]
pub fn get_price(id: &str) -> Option<u32> {
    let offer = get_offer(id)?;
    Some(adjusted(&offer, get_reputation()))
}

/// Moves an offer's multiplier so [`get_price`] answers exactly `price` at today's reputation.
pub fn set_price(id: &str, price: u32) -> bool {
    let reputation = get_reputation();
    modify_offer(id, |offer| {
        let discount = (reputation.max(0) / 10) as f32;
        let base = offer.inputs.first().map_or(1, ItemStack::count).max(1) as f32;
        offer.price_multiplier = (price.max(1) as f32 + discount) / base;
    })
}

/// The player's reputation, which is what the reputation discount is read from.
#[must_use]
pub fn get_reputation() -> i32 {
    with_reputation(|reputation| *reputation)
}

/// Sets the reputation, and answers what it was.
pub fn set_reputation(value: i32) -> i32 {
    with_reputation(|reputation| std::mem::replace(reputation, value))
}

/// Adds to the reputation, and answers the new value.
pub fn modify_reputation(delta: i32) -> i32 {
    with_reputation(|reputation| {
        *reputation += delta;
        *reputation
    })
}

fn factories() -> &'static Mutex<Vec<String>> {
    static FACTORIES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    FACTORIES.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_factories<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = factories().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Records an offer factory a mod ships, so a caller can see what can be built later.
pub fn register_offer_factory(name: &str) -> bool {
    with_factories(|factories| {
        if factories.iter().any(|held| held == name) {
            false
        } else {
            factories.push(name.to_owned());
            true
        }
    })
}

/// The offer factories a mod registered.
#[must_use]
pub fn get_offer_factories() -> Vec<String> {
    with_factories(|factories| factories.clone())
}

fn professions() -> &'static Mutex<Vec<String>> {
    static PROFESSIONS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    PROFESSIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_professions<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = professions().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Records a profession a mod ships, so its offers have a category to sit under.
pub fn register_profession(name: &str) -> bool {
    with_professions(|professions| {
        if professions.iter().any(|held| held == name) {
            false
        } else {
            professions.push(name.to_owned());
            true
        }
    })
}

/// The professions a mod registered.
#[must_use]
pub fn get_professions() -> Vec<String> {
    with_professions(|professions| professions.clone())
}

/// A villager's live offers, which only the client has. Empty.
#[must_use]
pub fn get_villager_offers() -> Vec<TradeOffer> {
    Vec::new()
}

/// Opens the trade screen, which is the client's. `false`.
pub fn open_trade_screen() -> bool {
    refused("screen")
}

/// Executes a trade against a live villager, which is the server's. `false`.
pub fn execute_live_trade(_id: &str) -> bool {
    refused("live")
}

/// A live villager. `None`, always: [`why_unknown`] says why.
pub type VillagerHandle = ();

/// A live villager, under the name a trade would need. `None`.
#[must_use]
pub fn get_villager() -> Option<VillagerHandle> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|offers| offers.clear());
        with_factories(|factories| factories.clear());
        with_professions(|professions| professions.clear());
        with_reputation(|reputation| *reputation = 0);
    }

    fn emeralds() -> TradeOffer {
        TradeOffer::new("hello:emeralds", "farmer")
            .input(ItemStack::new("minecraft:wheat", 20))
            .output(ItemStack::new("minecraft:emerald", 1))
            .max_uses(3)
            .xp(2)
    }

    fn stocked() -> Inventory {
        let mut inventory = Inventory::player();
        inventory
            .set_slot(0, Some(ItemStack::new("minecraft:wheat", 64)))
            .expect("a player slot");
        inventory
    }

    #[test]
    fn an_offer_registers_and_reads_back() {
        let _serial = serial();
        fresh();
        assert!(add_offer(emeralds()));
        assert!(!add_offer(emeralds()), "the second replaces the first");
        let offer = get_offer("hello:emeralds").expect("declared");
        assert_eq!(offer.profession, "farmer");
        assert_eq!(offer.max_uses, 3);
        assert_eq!(offer.uses, 3);
        assert_eq!(offer.xp, 2);
        assert_eq!(offer.inputs.len(), 1);
        assert_eq!(offer.outputs.len(), 1);
        assert_eq!(get_offers().len(), 1);
        assert_eq!(get_offers_for_profession("farmer").len(), 1);
        assert!(remove_offer("hello:emeralds"));
        assert!(!remove_offer("hello:emeralds"));
        let text = emeralds().to_string();
        assert!(text.contains("hello:emeralds"), "{text}");
        assert!(text.contains("farmer"), "{text}");
        assert!(text.contains("3 uses left"), "{text}");
    }

    #[test]
    fn matching_picks_an_offer_the_inventory_can_pay_for() {
        let _serial = serial();
        fresh();
        add_offer(emeralds());
        add_offer(
            TradeOffer::new("hello:bread", "farmer")
                .input(ItemStack::new("minecraft:emerald", 1))
                .output(ItemStack::new("minecraft:bread", 6)),
        );
        let mut empty = Inventory::player();
        assert_eq!(find_offer(&empty).map(|offer| offer.id), None, "nothing to trade with");
        assert_eq!(
            validate_trade("hello:emeralds", &empty),
            Err(TradeRefusal::MissingInputs {
                item: "minecraft:wheat".to_owned(),
                needed: 20,
                held: 0,
            })
        );
        empty
            .set_slot(0, Some(ItemStack::new("minecraft:emerald", 1)))
            .expect("a slot");
        assert_eq!(find_offer(&empty).map(|offer| offer.id), Some("hello:bread".to_owned()));
        let stocked = stocked();
        assert_eq!(find_offer(&stocked).map(|offer| offer.id), Some("hello:emeralds".to_owned()));
        assert_eq!(validate_trade("nothing", &stocked), Err(TradeRefusal::UnknownOffer));
    }

    #[test]
    fn a_trade_takes_the_inputs_and_spends_a_use() {
        let _serial = serial();
        fresh();
        add_offer(emeralds());
        let mut inventory = stocked();
        let receipt = execute_trade("hello:emeralds", &mut inventory).expect("the wheat is there");
        assert_eq!(receipt.uses_left, 2, "one use spent");
        assert_eq!(receipt.price, 20);
        assert_eq!(receipt.xp, 2);
        assert_eq!(inventory.count("minecraft:wheat"), 44, "20 taken");
        assert_eq!(inventory.count("minecraft:emerald"), 1, "one given");
        assert_eq!(get_offer("hello:emeralds").map(|offer| offer.uses), Some(2));
        // A second trade leaves 24, and a third leaves 4 with the offer used up.
        assert!(execute_trade("hello:emeralds", &mut inventory).is_some());
        assert!(execute_trade("hello:emeralds", &mut inventory).is_some());
        assert_eq!(get_offer("hello:emeralds").map(|offer| offer.uses), Some(0));
        assert_eq!(
            validate_trade("hello:emeralds", &inventory),
            Err(TradeRefusal::Restocking { cooldown: 6000 })
        );
        assert!(execute_trade("hello:emeralds", &mut inventory).is_none());
        assert_eq!(inventory.count("minecraft:wheat"), 4, "exactly three trades of twenty");
    }

    #[test]
    fn a_used_offer_restocks_only_after_its_cooldown_ticks_down() {
        let _serial = serial();
        fresh();
        add_offer(emeralds().restock(100));
        let mut inventory = stocked();
        for _ in 0..3 {
            assert!(execute_trade("hello:emeralds", &mut inventory).is_some());
        }
        assert_eq!(get_offer("hello:emeralds").map(|offer| offer.cooldown), Some(100));
        assert_eq!(tick_trades(40), 0, "not yet");
        assert_eq!(get_offer("hello:emeralds").map(|offer| offer.cooldown), Some(60));
        assert_eq!(
            validate_trade("hello:emeralds", &inventory),
            Err(TradeRefusal::Restocking { cooldown: 60 })
        );
        assert_eq!(tick_trades(60), 1, "the cooldown reached zero");
        assert_eq!(get_offer("hello:emeralds").map(|offer| offer.uses), Some(3), "uses are back");
        assert_eq!(get_offer("hello:emeralds").map(|offer| offer.cooldown), Some(0));
        assert_eq!(
            validate_trade("hello:emeralds", &inventory),
            Err(TradeRefusal::MissingInputs {
                item: "minecraft:wheat".to_owned(),
                needed: 20,
                held: 4,
            }),
            "restocked, so only the missing wheat stops it now"
        );
    }

    #[test]
    fn the_price_is_multiplied_and_discounted() {
        let _serial = serial();
        fresh();
        add_offer(emeralds());
        assert_eq!(get_price("hello:emeralds"), Some(20), "20 wheat at multiplier one");
        assert!(set_price("hello:emeralds", 30));
        assert_eq!(get_price("hello:emeralds"), Some(30), "the multiplier moved to match");
        modify_offer("hello:emeralds", |offer| offer.price_multiplier = 2.0);
        assert_eq!(get_price("hello:emeralds"), Some(40));
        assert_eq!(set_reputation(0), 0);
        assert_eq!(modify_reputation(25), 25);
        assert_eq!(get_price("hello:emeralds"), Some(38), "25 reputation is a 2 discount");
        assert_eq!(modify_reputation(-100), -75);
        assert_eq!(get_price("hello:emeralds"), Some(40), "a negative reputation discounts nothing");
        assert!(set_price("hello:emeralds", 3));
        assert_eq!(get_price("hello:emeralds"), Some(3));
        assert!(!set_price("nothing", 3));
        assert_eq!(get_price("nothing"), None);
    }

    #[test]
    fn the_factories_and_professions_a_mod_registers_are_recorded() {
        let _serial = serial();
        fresh();
        assert!(register_offer_factory("hello:farmer"));
        assert!(!register_offer_factory("hello:farmer"), "once");
        assert_eq!(get_offer_factories(), vec!["hello:farmer".to_owned()]);
        assert!(register_profession("farmer"));
        assert!(!register_profession("farmer"), "once");
        assert_eq!(get_professions(), vec!["farmer".to_owned()]);
    }

    #[test]
    fn the_clients_villagers_offers_and_screen_are_refused() {
        let _serial = serial();
        assert!(get_villager_offers().is_empty());
        assert!(get_villager().is_none());
        assert!(!open_trade_screen());
        assert!(!execute_live_trade("hello:emeralds"));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_trade_event_kinds_can_be_subscribed_to_before_they_fire() {
        let _serial = serial();
        assert!(register_event_kinds() > 0);
        assert!(has_event_kind("trade_complete"));
        assert!(!has_event_kind("nothing_at_all"));
        assert_eq!(event_kinds().len(), 6);
    }
}
