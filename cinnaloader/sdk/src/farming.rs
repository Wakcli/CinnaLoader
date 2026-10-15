//! Farming: the crops a mod declares, and growth that really advances against the clock.
//!
//! A crop is data — its growth stages, how long each takes, what it drops and the yield it can
//! answer — and a mod registers them here. What is *not* just data is growth: [`grow`] adds the
//! ticks a caller reports, scaled by a rate a mod may set, and rolls the crop's seeded growth
//! chance at every stage boundary, so a chance of zero really does stall and a chance of one
//! really does advance on the exact tick. A planting's stage, its progress and its seed are all
//! kept, so [`get_growth_stage`] and [`get_harvest_drops`] answer the same thing twice, and
//! [`harvest`] answers a real yield and resets the plot.
//!
//! Refused: the client's crops and its block growth — a live block's stage is the client's
//! state and growing it over the client's ticks is the client's world, neither of which this
//! host reaches.
//!
//! ```
//! use cinnaloader::farming;
//! use cinnaloader::items::ItemStack;
//!
//! farming::register_crop(farming::Crop::new("wheat", "Wheat")
//!     .stages(4)
//!     .ticks_per_stage(20)
//!     .drop_item(ItemStack::new("minecraft:wheat", 1)));
//! assert!(farming::plant("plot", "wheat", 1));
//! assert_eq!(farming::grow("plot", 20), Some(1));
//! assert!(farming::set_growth_stage("plot", 3));
//! assert_eq!(farming::get_harvest_drops("plot").len(), 1, "ripe wheat drops");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::events;
use crate::items::ItemStack;
use crate::worldgen::Random;

/// The farming events this host would have to receive from the client to fire.
pub const FARM_EVENTS: [&str; 6] = [
    "crop_planted",
    "crop_grown",
    "crop_harvested",
    "growth_rule_added",
    "bone_meal_applied",
    "growth_boost",
];

/// Teaches the event bus each farming event name, so a subscription is in place before the
/// hooks land.
pub fn register_event_kinds() -> usize {
    FARM_EVENTS
        .iter()
        .filter(|name| events::register_event_type(name))
        .count()
}

/// The event kinds this host would need from the client to fire a farming callback.
#[must_use]
pub fn event_kinds() -> Vec<&'static str> {
    FARM_EVENTS.to_vec()
}

/// Whether a farming event kind is one the bus knows.
#[must_use]
pub fn has_event_kind(kind: &str) -> bool {
    FARM_EVENTS.contains(&kind) && events::is_registered_kind(kind)
}

/// Why a growth question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "crop" => {
            "the client's crops are blocks it keeps in its world; a mod's own crops and \
             plantings live in this registry"
                .to_owned()
        }
        "growth" => "growing a crop over the client's ticks is the client's world, which this host cannot advance".to_owned(),
        "state" => "a live block's growth stage is the client's block state, which is not synced to it".to_owned(),
        "harvest" => "breaking a crop in the live world is the client's, and the server's to accept".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in farming, for a panel or a report.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["crop", "growth", "state", "harvest"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about a live crop: {}", why_unknown(what));
    false
}

/// A crop a mod declares: how it grows, what it gives, and how much.
#[derive(Debug, Clone, PartialEq)]
pub struct Crop {
    pub id: String,
    pub name: String,
    pub owner: String,
    /// How many growth stages it has, including the ripe one.
    pub stages: u32,
    pub ticks_per_stage: u32,
    pub drops: Vec<ItemStack>,
    pub min_yield: u32,
    pub max_yield: u32,
    /// How many stages a bone meal moves it.
    pub bone_meal_stages: u32,
}

impl Crop {
    #[must_use]
    pub fn new(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            stages: 8,
            ticks_per_stage: 20,
            drops: Vec::new(),
            min_yield: 1,
            max_yield: 1,
            bone_meal_stages: 1,
        }
    }

    #[must_use]
    pub fn stages(mut self, stages: u32) -> Self {
        self.stages = stages.max(1);
        self
    }

    #[must_use]
    pub fn ticks_per_stage(mut self, ticks: u32) -> Self {
        self.ticks_per_stage = ticks.max(1);
        self
    }

    #[must_use]
    pub fn drop_item(mut self, stack: ItemStack) -> Self {
        self.drops.push(stack);
        self
    }

    #[must_use]
    pub fn yield_range(mut self, min: u32, max: u32) -> Self {
        self.min_yield = min.max(1);
        self.max_yield = max.max(self.min_yield);
        self
    }

    #[must_use]
    pub fn bone_meal(mut self, stages: u32) -> Self {
        self.bone_meal_stages = stages.max(1);
        self
    }

    /// The last growth stage, which is the ripe one.
    #[must_use]
    pub fn last_stage(&self) -> u32 {
        self.stages - 1
    }
}

impl fmt::Display for Crop {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {} stages, {} ticks each)",
            self.name, self.id, self.stages, self.ticks_per_stage
        )
    }
}

/// How likely a crop is to take a stage when its time is up.
#[derive(Debug, Clone, PartialEq)]
pub struct GrowthRule {
    pub crop: String,
    pub chance: f32,
}

impl GrowthRule {
    #[must_use]
    pub fn new(crop: &str) -> Self {
        Self {
            crop: crop.to_owned(),
            chance: 1.0,
        }
    }

    #[must_use]
    pub fn chance(mut self, chance: f32) -> Self {
        self.chance = chance.clamp(0.0, 1.0);
        self
    }
}

/// A crop planted in a plot, with how far it has come.
#[derive(Debug, Clone, PartialEq)]
pub struct Planting {
    pub crop: String,
    pub stage: u32,
    pub progress: u32,
    /// The seed its growth chances and its yield are drawn from.
    pub seed: u64,
    /// How many draws have been taken, so a failed chance is retried on the next roll.
    pub rolls: u64,
}

fn crops() -> &'static Mutex<BTreeMap<String, Crop>> {
    static CROPS: OnceLock<Mutex<BTreeMap<String, Crop>>> = OnceLock::new();
    CROPS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_crops<T>(body: impl FnOnce(&mut BTreeMap<String, Crop>) -> T) -> T {
    let mut guard = crops().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn plantings() -> &'static Mutex<BTreeMap<String, Planting>> {
    static PLANTINGS: OnceLock<Mutex<BTreeMap<String, Planting>>> = OnceLock::new();
    PLANTINGS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_plantings<T>(body: impl FnOnce(&mut BTreeMap<String, Planting>) -> T) -> T {
    let mut guard = plantings().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn rules() -> &'static Mutex<BTreeMap<String, GrowthRule>> {
    static RULES: OnceLock<Mutex<BTreeMap<String, GrowthRule>>> = OnceLock::new();
    RULES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_rules<T>(body: impl FnOnce(&mut BTreeMap<String, GrowthRule>) -> T) -> T {
    let mut guard = rules().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn rates() -> &'static Mutex<BTreeMap<String, f32>> {
    static RATES: OnceLock<Mutex<BTreeMap<String, f32>>> = OnceLock::new();
    RATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_rates<T>(body: impl FnOnce(&mut BTreeMap<String, f32>) -> T) -> T {
    let mut guard = rates().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn handlers() -> &'static Mutex<Vec<String>> {
    static HANDLERS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    HANDLERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_handlers<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = handlers().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a crop, or replaces the one with that id.
pub fn register_crop(crop: Crop) -> bool {
    with_crops(|crops| crops.insert(crop.id.clone(), crop).is_none())
}

/// Forgets a declared crop.
pub fn unregister_crop(id: &str) -> bool {
    with_crops(|crops| crops.remove(id).is_some())
}

/// A declared crop by id.
#[must_use]
pub fn get_crop(id: &str) -> Option<Crop> {
    with_crops(|crops| crops.get(id).cloned())
}

/// Every declared crop, in id order.
#[must_use]
pub fn get_crops() -> Vec<Crop> {
    with_crops(|crops| crops.values().cloned().collect())
}

/// Declares how likely a crop is to grow, replacing any rule it already had.
pub fn register_growth_rule(rule: GrowthRule) -> bool {
    with_rules(|rules| rules.insert(rule.crop.clone(), rule).is_none())
}

/// A crop's growth rule, if a mod set one.
#[must_use]
pub fn get_growth_rule(crop: &str) -> Option<GrowthRule> {
    with_rules(|rules| rules.get(crop).cloned())
}

fn chance_of(crop: &str) -> f32 {
    get_growth_rule(crop).map_or(1.0, |rule| rule.chance)
}

/// Multiplies a crop's growth rate by `factor`. The rate starts at one.
pub fn modify_growth_rate(crop: &str, factor: f32) -> bool {
    if get_crop(crop).is_none() {
        return false;
    }
    with_rates(|rates| {
        rates.insert(crop.to_owned(), factor.max(0.0));
    });
    true
}

/// A crop's growth rate, one unless a mod changed it.
#[must_use]
pub fn get_growth_rate(crop: &str) -> f32 {
    with_rates(|rates| rates.get(crop).copied().unwrap_or(1.0))
}

/// Plants a crop in a plot, and answers whether the crop exists.
pub fn plant(plot: &str, crop: &str, seed: u64) -> bool {
    if get_crop(crop).is_none() {
        return false;
    }
    with_plantings(|plantings| {
        plantings.insert(
            plot.to_owned(),
            Planting {
                crop: crop.to_owned(),
                stage: 0,
                progress: 0,
                seed,
                rolls: 0,
            },
        );
    });
    true
}

/// A plot's planting.
#[must_use]
pub fn get_planting(plot: &str) -> Option<Planting> {
    with_plantings(|plantings| plantings.get(plot).cloned())
}

/// A plot's growth stage.
#[must_use]
pub fn get_growth_stage(plot: &str) -> Option<u32> {
    get_planting(plot).map(|planting| planting.stage)
}

/// Sets a plot's growth stage, clamped to the crop's stages, and answers whether it exists.
pub fn set_growth_stage(plot: &str, stage: u32) -> bool {
    let Some(planting) = get_planting(plot) else {
        return false;
    };
    let last = get_crop(&planting.crop).map_or(stage, |crop| crop.last_stage());
    with_plantings(|plantings| {
        if let Some(planting) = plantings.get_mut(plot) {
            planting.stage = stage.min(last);
            planting.progress = 0;
            true
        } else {
            false
        }
    })
}

/// Whether a plot's crop still has a stage to reach.
#[must_use]
pub fn can_grow(plot: &str) -> Option<bool> {
    let planting = get_planting(plot)?;
    let crop = get_crop(&planting.crop)?;
    Some(planting.stage < crop.last_stage())
}

/// Adds `ticks` of growth to a plot, applying the crop's rate and its seeded chance at every
/// stage boundary, and answers the stage it reached.
pub fn grow(plot: &str, ticks: u32) -> Option<u32> {
    let planting = get_planting(plot)?;
    let crop = get_crop(&planting.crop)?;
    if planting.stage >= crop.last_stage() {
        return Some(planting.stage);
    }
    let rate = get_growth_rate(&crop.id);
    let chance = f64::from(chance_of(&crop.id));
    let per_stage = u64::from(crop.ticks_per_stage);
    let added = (ticks as f32 * rate).round() as u32;
    let mut stage = planting.stage;
    let mut progress = planting.progress + added;
    let mut rolls = planting.rolls;
    while stage < crop.last_stage() && u64::from(progress) >= per_stage {
        let mut rng = Random::new(planting.seed ^ rolls.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        rolls += 1;
        progress -= crop.ticks_per_stage;
        if rng.chance(chance) {
            stage += 1;
        } else {
            progress = 0;
            break;
        }
    }
    if stage >= crop.last_stage() {
        progress = 0;
    }
    with_plantings(|plantings| {
        if let Some(planting) = plantings.get_mut(plot) {
            planting.stage = stage;
            planting.progress = progress;
            planting.rolls = rolls;
        }
    });
    Some(stage)
}

/// A bone meal boost: moves a plot up by the crop's bone-meal stages, up to ripe.
pub fn apply_bone_meal(plot: &str) -> Option<u32> {
    let planting = get_planting(plot)?;
    let crop = get_crop(&planting.crop)?;
    let stage = (planting.stage + crop.bone_meal_stages).min(crop.last_stage());
    set_growth_stage(plot, stage);
    Some(stage)
}

fn yield_of(planting: &Planting, crop: &Crop) -> Vec<ItemStack> {
    let mut rng = Random::new(planting.seed);
    crop.drops
        .iter()
        .map(|stack| {
            let count = rng.next_range(i64::from(crop.min_yield), i64::from(crop.max_yield)) as u32;
            ItemStack::new(stack.item(), count)
        })
        .collect()
}

/// What a plot would give if harvested: the crop's real yield when ripe, and nothing when not.
#[must_use]
pub fn get_harvest_drops(plot: &str) -> Vec<ItemStack> {
    let Some(planting) = get_planting(plot) else {
        return Vec::new();
    };
    let Some(crop) = get_crop(&planting.crop) else {
        return Vec::new();
    };
    if planting.stage < crop.last_stage() {
        return Vec::new();
    }
    yield_of(&planting, &crop)
}

/// Harvests a ripe plot for its yield and resets it to the first stage. `None` when it is not
/// ripe.
pub fn harvest(plot: &str) -> Option<Vec<ItemStack>> {
    let planting = get_planting(plot)?;
    let crop = get_crop(&planting.crop)?;
    if planting.stage < crop.last_stage() {
        return None;
    }
    let yield_ = yield_of(&planting, &crop);
    with_plantings(|plantings| {
        if let Some(planting) = plantings.get_mut(plot) {
            planting.stage = 0;
            planting.progress = 0;
            // The next harvest is a different draw, as the game's own is.
            planting.seed = planting.seed.wrapping_add(1);
            planting.rolls = 0;
        }
    });
    Some(yield_)
}

/// Records a harvest handler a mod ships, so a caller can see what reacts to a harvest.
pub fn register_harvest_handler(name: &str) -> bool {
    with_handlers(|handlers| {
        if handlers.iter().any(|held| held == name) {
            false
        } else {
            handlers.push(name.to_owned());
            true
        }
    })
}

/// The harvest handlers a mod registered.
#[must_use]
pub fn get_harvest_handlers() -> Vec<String> {
    with_handlers(|handlers| handlers.clone())
}

/// A live crop's stage. `None`, always: [`why_unknown`] says why.
#[must_use]
pub fn get_live_stage() -> Option<u32> {
    None
}

/// Grows a live crop, which is the client's world. `false`.
pub fn grow_live() -> bool {
    refused("growth")
}

/// Harvests a live crop, which is the client's. Empty.
#[must_use]
pub fn harvest_live() -> Vec<ItemStack> {
    let _ = refused("harvest");
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_crops(|crops| crops.clear());
        with_plantings(|plantings| plantings.clear());
        with_rules(|rules| rules.clear());
        with_rates(|rates| rates.clear());
        with_handlers(|handlers| handlers.clear());
    }

    fn wheat() -> Crop {
        Crop::new("wheat", "Wheat")
            .stages(4)
            .ticks_per_stage(20)
            .yield_range(1, 3)
            .drop_item(ItemStack::new("minecraft:wheat", 1))
    }

    #[test]
    fn a_crop_registers_and_a_plot_plants_at_the_first_stage() {
        let _serial = serial();
        fresh();
        assert!(register_crop(wheat()));
        assert!(!register_crop(wheat()), "the second replaces the first");
        assert_eq!(get_crop("wheat").map(|crop| crop.stages), Some(4));
        assert_eq!(get_crop("wheat").map(|crop| crop.last_stage()), Some(3));
        assert_eq!(get_crops().len(), 1);
        assert!(plant("plot", "wheat", 1));
        assert!(!plant("plot", "nothing", 1));
        let planting = get_planting("plot").expect("planted");
        assert_eq!(planting.stage, 0);
        assert_eq!(planting.progress, 0);
        assert_eq!(planting.seed, 1);
        assert_eq!(get_growth_stage("plot"), Some(0));
        assert_eq!(can_grow("plot"), Some(true));
        assert!(unregister_crop("wheat"));
        assert!(!unregister_crop("wheat"));
    }

    #[test]
    fn growth_advances_one_stage_per_stage_of_ticks() {
        let _serial = serial();
        fresh();
        register_crop(wheat());
        assert!(plant("plot", "wheat", 1));
        assert_eq!(grow("plot", 20), Some(1), "one stage's worth of ticks");
        assert_eq!(get_growth_stage("plot"), Some(1));
        assert_eq!(grow("plot", 20), Some(2));
        assert_eq!(grow("plot", 40), Some(3), "two more stages at once");
        assert_eq!(get_growth_stage("plot"), Some(3));
        assert_eq!(can_grow("plot"), Some(false), "ripe");
        assert_eq!(grow("plot", 100), Some(3), "a ripe crop does not grow past its last stage");
        assert_eq!(grow("nothing", 20), None);
    }

    #[test]
    fn the_growth_chance_is_applied_rather_than_assumed() {
        let _serial = serial();
        fresh();
        register_crop(wheat());
        assert!(plant("plot", "wheat", 1));
        assert!(register_growth_rule(GrowthRule::new("wheat").chance(0.0)));
        assert_eq!(grow("plot", 100), Some(0), "no chance, no stage");
        assert_eq!(get_growth_stage("plot"), Some(0));
        assert!(!register_growth_rule(GrowthRule::new("wheat").chance(1.0)), "replaces the rule");
        assert_eq!(grow("plot", 100), Some(3), "a certain chance reaches every stage");
        assert_eq!(get_growth_rule("wheat").map(|rule| rule.chance), Some(1.0));
        assert_eq!(chance_of("nothing"), 1.0, "no rule means a certain chance");
    }

    #[test]
    fn the_growth_rate_scales_the_ticks_that_arrive() {
        let _serial = serial();
        fresh();
        register_crop(wheat());
        assert!(plant("plot", "wheat", 1));
        assert_eq!(get_growth_rate("wheat"), 1.0);
        assert!(modify_growth_rate("wheat", 2.0));
        assert_eq!(grow("plot", 10), Some(1), "ten ticks at double rate is one stage");
        assert_eq!(grow("plot", 30), Some(3), "thirty at double is three stages");
        assert!(!modify_growth_rate("nothing", 2.0));
        assert_eq!(get_growth_rate("nothing"), 1.0);
    }

    #[test]
    fn a_bone_meal_boost_moves_the_stage_without_a_roll() {
        let _serial = serial();
        fresh();
        register_crop(wheat().bone_meal(2));
        assert!(plant("plot", "wheat", 1));
        assert_eq!(apply_bone_meal("plot"), Some(2), "two stages");
        assert_eq!(apply_bone_meal("plot"), Some(3), "clamped to ripe");
        assert_eq!(apply_bone_meal("plot"), Some(3), "and stays there");
        assert_eq!(apply_bone_meal("nothing"), None);
    }

    #[test]
    fn a_ripe_plot_answers_a_real_yield_and_harvest_resets_it() {
        let _serial = serial();
        fresh();
        register_crop(wheat());
        assert!(plant("plot", "wheat", 1));
        assert!(get_harvest_drops("plot").is_empty(), "green wheat drops nothing");
        assert_eq!(harvest("plot"), None);
        assert!(set_growth_stage("plot", 3));
        let drops = get_harvest_drops("plot");
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item(), "minecraft:wheat");
        assert!((1..=3).contains(&drops[0].count()), "the yield is in range");
        assert_eq!(drops, get_harvest_drops("plot"), "the same seed answers the same yield");
        let harvested = harvest("plot").expect("ripe");
        assert_eq!(harvested, drops);
        assert_eq!(get_growth_stage("plot"), Some(0), "reset to the first stage");
        assert!(get_harvest_drops("plot").is_empty());
        assert!(!set_growth_stage("nothing", 1));
        assert!(register_harvest_handler("hello:seeds"));
        assert!(!register_harvest_handler("hello:seeds"), "once");
        assert_eq!(get_harvest_handlers(), vec!["hello:seeds".to_owned()]);
    }

    #[test]
    fn the_clients_crops_and_their_growth_are_refused() {
        let _serial = serial();
        assert!(get_live_stage().is_none());
        assert!(!grow_live());
        assert!(harvest_live().is_empty());
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_farming_event_kinds_can_be_subscribed_to_before_they_fire() {
        let _serial = serial();
        assert!(register_event_kinds() > 0);
        assert!(has_event_kind("crop_harvested"));
        assert!(!has_event_kind("nothing_at_all"));
        assert_eq!(event_kinds().len(), 6);
    }
}
