//! Section 53 demo.

use cinnaloader::breeding;
use cinnaloader::farming;
use cinnaloader::inventory::Inventory;
use cinnaloader::items::ItemStack;
use cinnaloader::trading;

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    trading::add_offer(
        trading::TradeOffer::new("hello:emeralds", "farmer")
            .input(ItemStack::new("minecraft:wheat", 20))
            .output(ItemStack::new("minecraft:emerald", 1))
            .max_uses(3)
            .xp(2)
            .restock(100),
    );
    let mut bag = Inventory::player();
    let start_wheat = 64;
    bag.set_slot(0, Some(ItemStack::new("minecraft:wheat", start_wheat)))
        .ok();
    let matched = trading::find_offer(&bag)
        .map(|offer| offer.id)
        .unwrap_or_default();
    let uses_before = trading::get_offer("hello:emeralds")
        .map(|offer| offer.uses)
        .unwrap_or(0);
    let receipt = trading::execute_trade("hello:emeralds", &mut bag);
    let uses_after = trading::get_offer("hello:emeralds")
        .map(|offer| offer.uses)
        .unwrap_or(0);
    lines.push(format!(
        "trading: matched {} | uses {} -> {} | wheat {} -> {} | emerald {} | price {} xp {}",
        matched,
        uses_before,
        uses_after,
        start_wheat,
        bag.count("minecraft:wheat"),
        bag.count("minecraft:emerald"),
        receipt.as_ref().map_or(0, |r| r.price),
        receipt.as_ref().map_or(0, |r| r.xp),
    ));

    trading::execute_trade("hello:emeralds", &mut bag);
    trading::execute_trade("hello:emeralds", &mut bag);
    let spent = trading::get_offer("hello:emeralds")
        .map(|offer| offer.uses)
        .unwrap_or(0);
    let cooldown = trading::get_offer("hello:emeralds")
        .map(|offer| offer.cooldown)
        .unwrap_or(0);
    let restocked = trading::tick_trades(100);
    let back = trading::get_offer("hello:emeralds")
        .map(|offer| offer.uses)
        .unwrap_or(0);
    lines.push(format!(
        "trading: uses {} at cooldown {} | tick 100 -> restocked {} offers, uses {}",
        spent, cooldown, restocked, back
    ));

    let price_before = trading::get_price("hello:emeralds").unwrap_or(0);
    trading::modify_reputation(25);
    let price_after = trading::get_price("hello:emeralds").unwrap_or(0);
    lines.push(format!(
        "trading: price {} -> {} at reputation {} (25 is a 2 discount)",
        price_before,
        price_after,
        trading::get_reputation()
    ));

    breeding::register_parent(breeding::Animal::new("cow-a", "cow").trait_of("speed", 0.25));
    breeding::register_parent(breeding::Animal::new("cow-b", "cow").trait_of("speed", 0.3));
    breeding::register_rule(
        breeding::BreedingRule::new("cow", "cow", "calf")
            .cooldown(100)
            .mature_ticks(60),
    );
    let pair = breeding::start("cow-a", "cow-b").unwrap_or_default();
    let child = breeding::breed(&pair, 7);
    let speed = child
        .as_ref()
        .and_then(|child| child.traits.get("speed").copied())
        .unwrap_or(-1.0);
    let from = if speed == 0.3 {
        "B"
    } else if speed == 0.25 {
        "A"
    } else {
        "?"
    };
    lines.push(format!(
        "breeding: pair {} | child {} speed {} from parent {} (A 0.25 B 0.30) | growth {}",
        pair,
        child.as_ref().map(|child| child.kind.clone()).unwrap_or_default(),
        speed,
        from,
        child.as_ref().map_or(0, |child| child.growth),
    ));

    let matured = breeding::advance(60);
    let grown = breeding::get_child(&pair);
    lines.push(format!(
        "breeding: advance 60 matured {} | mature {} age {} growth {} | cooldown {}",
        matured,
        grown.as_ref().map(|child| child.mature).unwrap_or(false),
        grown.as_ref().map_or(0, |child| child.age),
        grown.as_ref().map_or(0, |child| child.growth),
        breeding::get_cooldown(&pair).unwrap_or(0),
    ));

    farming::register_crop(
        farming::Crop::new("wheat", "Wheat")
            .stages(4)
            .ticks_per_stage(20)
            .yield_range(1, 3)
            .drop_item(ItemStack::new("minecraft:wheat", 1)),
    );
    farming::register_growth_rule(farming::GrowthRule::new("wheat").chance(1.0));
    farming::plant("plot", "wheat", 1);
    let stage_before = farming::get_growth_stage("plot").unwrap_or(99);
    let stage_after = farming::grow("plot", 60).unwrap_or(99);
    lines.push(format!(
        "farming: stage {} -> {} after 60 ticks (3000 ms) | can_grow {}",
        stage_before,
        stage_after,
        farming::can_grow("plot").unwrap_or(true)
    ));

    farming::apply_bone_meal("plot");
    let ripe = farming::get_harvest_drops("plot");
    lines.push(format!(
        "farming: bone meal -> stage {} | yield {} x{}",
        farming::get_growth_stage("plot").unwrap_or(99),
        ripe.first().map(|stack| stack.item().to_owned()).unwrap_or_default(),
        ripe.first().map_or(0, ItemStack::count),
    ));
    let harvested = farming::harvest("plot").unwrap_or_default();
    lines.push(format!(
        "farming: harvested {} x{} | plot reset to stage {}",
        harvested.first().map(|stack| stack.item().to_owned()).unwrap_or_default(),
        harvested.first().map_or(0, ItemStack::count),
        farming::get_growth_stage("plot").unwrap_or(99),
    ));

    farming::register_growth_rule(farming::GrowthRule::new("wheat").chance(0.0));
    farming::plant("dry", "wheat", 2);
    lines.push(format!(
        "farming: chance 0 -> stage {} after 100 ticks (a zero chance stalls)",
        farming::grow("dry", 100).unwrap_or(99)
    ));

    lines.push(format!(
        "refusals: trading {} breeding {} farming {} | events t{} b{} f{}",
        trading::refusals().len(),
        breeding::refusals().len(),
        farming::refusals().len(),
        trading::event_kinds().len(),
        breeding::event_kinds().len(),
        farming::event_kinds().len(),
    ));

    lines
}
