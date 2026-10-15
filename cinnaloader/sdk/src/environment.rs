//! Weather, time, sky and light: a real environment model a mod owns, and the client's own
//! weather out of reach.
//!
//! The client's weather, its sky, its time of day, the light at one of its positions, the
//! climate of its biomes and the effects it is running are all inside the client, and every
//! one of those is refused with a reason from [`why_unknown`] rather than guessed at. [`refusals`]
//! lists them.
//!
//! What a mod genuinely owns here is a model, and it runs:
//!
//! * [`WeatherMachine`] is a state machine over clear, rain and thunder. Each spell draws a
//!   real duration from a seeded [`Random`], its rain and thunder levels ramp toward the
//!   state's target rather than jumping, [`advance`](WeatherMachine::advance) moves the clock,
//!   and a transition draws the next state from the same seed — so a seed replays the same
//!   sequence.
//! * [`LightModel`] combines block light and sky light (the brighter wins), skylight reduced
//!   by the time of day, and light falling off one level per block from a source, clamped to
//!   0..[`MAX_LIGHT`].
//! * [`BiomeClimate`] is a declared temperature, humidity, downfall and category per biome id,
//!   with [`temperature_at`]/[`humidity_at`] lookups and [`precipitation_at`], which turns the
//!   temperature into rain, snow or nothing.
//! * [`EnvironmentEffect`] is an environmental effect a mod declares with a kind, a duration and
//!   an amplifier; [`EffectSet`] applies, queries and expires them over an [`EnvironmentClock`].
//!
//! ```
//! use cinnaloader::environment::{self, LightModel, LightSource, WeatherMachine, WeatherState};
//!
//! let mut weather = WeatherMachine::with_weather(7, WeatherState::Rain, 400);
//! weather.advance(50);
//! assert_eq!(weather.rain_level(), 0.5, "the rain eased up, not jumped");
//! assert_eq!(weather.weather(), WeatherState::Rain);
//!
//! let light = LightModel::new().with_source(LightSource::block((0, 0, 0), 15));
//! assert_eq!(light.block_light_at(1, 0, 0), 14, "one level per block");
//!
//! assert!(environment::get_weather().is_none(), "the client's weather is out of reach");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::worldgen::Random;

/// The ticks in one day: noon at 6000, midnight at 18000.
pub const TICKS_PER_DAY: u64 = 24_000;
/// The brightest light gets.
pub const MAX_LIGHT: u8 = 15;
/// The most the sky dims at night, which is what leaves a level-15 skylight at 4.
pub const MAX_SKY_DARKEN: u8 = 11;
/// How many ticks a weather level takes to ramp the whole way.
pub const RAMP_TICKS: u64 = 100;
/// The temperature under which precipitation falls as snow, the game's own threshold.
pub const SNOW_TEMPERATURE: f32 = 0.15;

/// Which of the three kinds of weather the sky is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WeatherState {
    #[default]
    Clear,
    Rain,
    Thunder,
}

impl WeatherState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Rain => "rain",
            Self::Thunder => "thunder",
        }
    }

    /// Whether anything is falling.
    #[must_use]
    pub fn is_precipitating(self) -> bool {
        matches!(self, Self::Rain | Self::Thunder)
    }

    /// The rain and thunder levels the weather eases toward.
    #[must_use]
    pub fn targets(self) -> (f64, f64) {
        match self {
            Self::Clear => (0.0, 0.0),
            Self::Rain => (1.0, 0.0),
            Self::Thunder => (1.0, 1.0),
        }
    }

    /// How long a spell of this weather lasts, at least and at most, in ticks.
    #[must_use]
    pub fn duration_range(self) -> (u64, u64) {
        match self {
            Self::Clear => (12_000, 24_000),
            Self::Rain => (3_000, 8_000),
            Self::Thunder => (2_000, 4_000),
        }
    }
}

impl fmt::Display for WeatherState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A weather kind a mod declares, for the day the client can be told about one.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherType {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub state: WeatherState,
    pub rain_level: f64,
    pub thunder_level: f64,
    pub min_duration: u64,
    pub max_duration: u64,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered: bool,
}

impl WeatherType {
    /// A weather kind of a state, taking that state's own levels and durations.
    #[must_use]
    pub fn new(id: &str, name: &str, state: WeatherState) -> Self {
        let (rain_level, thunder_level) = state.targets();
        let (min_duration, max_duration) = state.duration_range();
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            owner: crate::metadata::id(),
            state,
            rain_level,
            thunder_level,
            min_duration,
            max_duration,
            registered: false,
        }
    }

    #[must_use]
    pub fn rain_level(mut self, rain_level: f64) -> Self {
        self.rain_level = rain_level.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn thunder_level(mut self, thunder_level: f64) -> Self {
        self.thunder_level = thunder_level.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn durations(mut self, minimum: u64, maximum: u64) -> Self {
        self.min_duration = minimum.min(maximum).max(1);
        self.max_duration = minimum.max(maximum).max(1);
        self
    }
}

impl fmt::Display for WeatherType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}, rain {:.2}, thunder {:.2})",
            self.name, self.id, self.state, self.rain_level, self.thunder_level
        )
    }
}

/// A weather state machine a mod owns, driven by a seed and moved a tick at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherMachine {
    seed: u64,
    rng: Random,
    state: WeatherState,
    duration: u64,
    remaining: u64,
    rain_level: f64,
    thunder_level: f64,
    step: u64,
}

impl WeatherMachine {
    /// A machine that starts clear, its first spell drawn from the seed.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        let mut rng = Random::new(seed);
        let (minimum, maximum) = WeatherState::Clear.duration_range();
        let duration = draw_duration(&mut rng, minimum, maximum);
        Self {
            seed,
            rng,
            state: WeatherState::Clear,
            duration,
            remaining: duration,
            rain_level: 0.0,
            thunder_level: 0.0,
            step: 0,
        }
    }

    /// A machine forced into a state for a known number of ticks, its levels still at zero so
    /// the ramp can be watched.
    #[must_use]
    pub fn with_weather(seed: u64, state: WeatherState, duration: u64) -> Self {
        let duration = duration.max(1);
        Self {
            seed,
            rng: Random::new(seed),
            state,
            duration,
            remaining: duration,
            rain_level: 0.0,
            thunder_level: 0.0,
            step: 0,
        }
    }

    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// How many transitions the machine has made.
    #[must_use]
    pub fn step(&self) -> u64 {
        self.step
    }

    #[must_use]
    pub fn weather(&self) -> WeatherState {
        self.state
    }

    #[must_use]
    pub fn rain_level(&self) -> f64 {
        self.rain_level
    }

    #[must_use]
    pub fn thunder_level(&self) -> f64 {
        self.thunder_level
    }

    /// How long the current spell lasts in total.
    #[must_use]
    pub fn duration(&self) -> u64 {
        self.duration
    }

    /// How many ticks of the current spell are left.
    #[must_use]
    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    #[must_use]
    pub fn is_precipitating(&self) -> bool {
        self.state.is_precipitating()
    }

    /// Forces a state, drawing the new spell's duration from the seed.
    pub fn set_weather(&mut self, state: WeatherState) {
        self.state = state;
        let (minimum, maximum) = state.duration_range();
        self.duration = draw_duration(&mut self.rng, minimum, maximum);
        self.remaining = self.duration;
    }

    pub fn set_rain_level(&mut self, rain_level: f64) {
        self.rain_level = rain_level.clamp(0.0, 1.0);
    }

    pub fn set_thunder_level(&mut self, thunder_level: f64) {
        self.thunder_level = thunder_level.clamp(0.0, 1.0);
    }

    /// Sets how long is left of the current spell.
    pub fn set_duration(&mut self, ticks: u64) {
        let ticks = ticks.max(1);
        self.duration = ticks;
        self.remaining = ticks;
    }

    /// Moves the weather on by `ticks`, ramping the levels and turning the weather over when a
    /// spell ends.
    pub fn advance(&mut self, ticks: u64) {
        let mut left = ticks;
        while left > 0 {
            if self.remaining == 0 {
                self.transition();
            }
            let step = left.min(self.remaining);
            self.ramp(step);
            self.remaining -= step;
            left -= step;
            if self.remaining == 0 {
                self.transition();
            }
        }
    }

    // levels ease toward the state's target a RAMP_TICKS-th per tick instead of jumping
    fn ramp(&mut self, ticks: u64) {
        let (rain_target, thunder_target) = self.state.targets();
        let delta = ticks as f64 / RAMP_TICKS as f64;
        self.rain_level = approach(self.rain_level, rain_target, delta);
        self.thunder_level = approach(self.thunder_level, thunder_target, delta);
    }

    fn transition(&mut self) {
        self.step += 1;
        let roll = self.rng.next_f64();
        let next = match self.state {
            WeatherState::Clear => {
                if roll < 0.6 {
                    WeatherState::Rain
                } else {
                    WeatherState::Clear
                }
            }
            WeatherState::Rain => {
                if roll < 0.35 {
                    WeatherState::Thunder
                } else if roll < 0.75 {
                    WeatherState::Clear
                } else {
                    WeatherState::Rain
                }
            }
            WeatherState::Thunder => {
                if roll < 0.5 {
                    WeatherState::Rain
                } else {
                    WeatherState::Clear
                }
            }
        };
        self.state = next;
        let (minimum, maximum) = next.duration_range();
        self.duration = draw_duration(&mut self.rng, minimum, maximum);
        self.remaining = self.duration;
    }
}

impl fmt::Display for WeatherMachine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} for {} more ticks, rain {:.2}, thunder {:.2} (seed {})",
            self.state, self.remaining, self.rain_level, self.thunder_level, self.seed
        )
    }
}

fn draw_duration(rng: &mut Random, minimum: u64, maximum: u64) -> u64 {
    rng.next_range(minimum as i64, maximum as i64) as u64
}

fn approach(value: f64, target: f64, step: f64) -> f64 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

/// Whether a light source is a block's own light or the sky's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LightKind {
    #[default]
    Block,
    Sky,
}

impl LightKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Sky => "sky",
        }
    }
}

/// One light source a mod places in its own model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LightSource {
    pub position: (i32, i32, i32),
    pub level: u8,
    pub kind: LightKind,
}

impl LightSource {
    #[must_use]
    pub fn block(position: (i32, i32, i32), level: u8) -> Self {
        Self {
            position,
            level: level.min(MAX_LIGHT),
            kind: LightKind::Block,
        }
    }

    #[must_use]
    pub fn sky(position: (i32, i32, i32), level: u8) -> Self {
        Self {
            position,
            level: level.min(MAX_LIGHT),
            kind: LightKind::Sky,
        }
    }
}

impl fmt::Display for LightSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} light {} at ({}, {}, {})",
            self.kind.as_str(),
            self.level,
            self.position.0,
            self.position.1,
            self.position.2
        )
    }
}

/// A model of light a mod owns: the sources it placed and the time of day.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LightModel {
    pub sources: Vec<LightSource>,
    pub day_time: u64,
}

impl LightModel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_day_time(mut self, day_time: u64) -> Self {
        self.day_time = day_time;
        self
    }

    #[must_use]
    pub fn with_source(mut self, source: LightSource) -> Self {
        self.sources.push(source);
        self
    }

    pub fn add_source(&mut self, source: LightSource) {
        self.sources.push(source);
    }

    /// The block light at a position: the brightest source after one level per block of
    /// falloff.
    #[must_use]
    pub fn block_light_at(&self, x: i32, y: i32, z: i32) -> u8 {
        self.sources
            .iter()
            .filter(|source| source.kind == LightKind::Block)
            .map(|source| falloff(source.level, manhattan(source.position, (x, y, z))))
            .max()
            .unwrap_or(0)
    }

    /// The sky light at a position: the brightest sky source after falloff, reduced by the
    /// time of day.
    #[must_use]
    pub fn sky_light_at(&self, x: i32, y: i32, z: i32) -> u8 {
        let darken = sky_darken(self.day_time);
        self.sources
            .iter()
            .filter(|source| source.kind == LightKind::Sky)
            .map(|source| falloff(source.level, manhattan(source.position, (x, y, z))))
            .map(|level| level.saturating_sub(darken))
            .max()
            .unwrap_or(0)
    }

    /// The light at a position: block light and sky light combined.
    #[must_use]
    pub fn light_at(&self, x: i32, y: i32, z: i32) -> u8 {
        combine(self.block_light_at(x, y, z), self.sky_light_at(x, y, z))
    }
}

/// A light level after one level per block of falloff, clamped to 0..[`MAX_LIGHT`] at both ends.
#[must_use]
pub fn falloff(level: u8, distance: u32) -> u8 {
    let distance = distance.min(u32::from(MAX_LIGHT)) as u8;
    level.min(MAX_LIGHT).saturating_sub(distance)
}

/// Block light and sky light combined: the brighter of the two, clamped to [`MAX_LIGHT`].
#[must_use]
pub fn combine(block: u8, sky: u8) -> u8 {
    block.min(MAX_LIGHT).max(sky.min(MAX_LIGHT))
}

/// How much the sky is dimmed at a time of day, 0 by day and up to [`MAX_SKY_DARKEN`] at night.
#[must_use]
pub fn sky_darken(day_time: u64) -> u8 {
    // the game dims skylight by up to 11 across a 1800-tick dusk and a 1800-tick dawn
    let tick = day_time % TICKS_PER_DAY;
    if tick < 12_000 {
        0
    } else if tick < 13_800 {
        ((tick - 12_000) * u64::from(MAX_SKY_DARKEN) / 1_800) as u8
    } else if tick < 22_200 {
        MAX_SKY_DARKEN
    } else {
        (u64::from(MAX_SKY_DARKEN) - (tick - 22_200) * u64::from(MAX_SKY_DARKEN) / 1_800) as u8
    }
}

/// A sky light level after the time of day has dimmed it.
#[must_use]
pub fn sky_light(level: u8, day_time: u64) -> u8 {
    level.min(MAX_LIGHT).saturating_sub(sky_darken(day_time))
}

fn manhattan(from: (i32, i32, i32), to: (i32, i32, i32)) -> u32 {
    (from.0 - to.0).unsigned_abs() + (from.1 - to.1).unsigned_abs() + (from.2 - to.2).unsigned_abs()
}

/// How warm or cold a biome is, in bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ClimateCategory {
    Frozen,
    Cold,
    #[default]
    Temperate,
    Warm,
    Hot,
}

impl ClimateCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Frozen => "frozen",
            Self::Cold => "cold",
            Self::Temperate => "temperate",
            Self::Warm => "warm",
            Self::Hot => "hot",
        }
    }

    /// The band a temperature falls in.
    #[must_use]
    pub fn from_temperature(temperature: f32) -> Self {
        if temperature < SNOW_TEMPERATURE {
            Self::Frozen
        } else if temperature < 0.4 {
            Self::Cold
        } else if temperature < 0.8 {
            Self::Temperate
        } else if temperature < 1.2 {
            Self::Warm
        } else {
            Self::Hot
        }
    }
}

impl fmt::Display for ClimateCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What falls from the sky in a biome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Precipitation {
    #[default]
    None,
    Rain,
    Snow,
}

impl Precipitation {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Rain => "rain",
            Self::Snow => "snow",
        }
    }
}

impl fmt::Display for Precipitation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A climate a mod declares for a biome: how warm, how wet, and how much falls.
#[derive(Debug, Clone, PartialEq)]
pub struct BiomeClimate {
    pub biome: String,
    pub owner: String,
    pub temperature: f32,
    pub humidity: f32,
    pub downfall: f32,
    pub category: ClimateCategory,
}

impl BiomeClimate {
    /// A climate of a temperature, its category and its defaults taken from it.
    #[must_use]
    pub fn new(biome: &str, temperature: f32) -> Self {
        Self {
            biome: biome.to_owned(),
            owner: crate::metadata::id(),
            temperature,
            humidity: 0.5,
            downfall: 0.5,
            category: ClimateCategory::from_temperature(temperature),
        }
    }

    #[must_use]
    pub fn humidity(mut self, humidity: f32) -> Self {
        self.humidity = humidity.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn downfall(mut self, downfall: f32) -> Self {
        self.downfall = downfall.max(0.0);
        self
    }

    #[must_use]
    pub fn category(mut self, category: ClimateCategory) -> Self {
        self.category = category;
        self
    }

    /// What the temperature means: snow below the threshold, rain above it, nothing when
    /// nothing falls.
    #[must_use]
    pub fn precipitation(&self) -> Precipitation {
        if self.downfall <= 0.0 {
            Precipitation::None
        } else if self.temperature < SNOW_TEMPERATURE {
            Precipitation::Snow
        } else {
            Precipitation::Rain
        }
    }

    /// Whether snow settles in this biome.
    #[must_use]
    pub fn snows(&self) -> bool {
        self.precipitation() == Precipitation::Snow
    }
}

impl fmt::Display for BiomeClimate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, temp {:.2}, humidity {:.2}, downfall {:.2}, {})",
            self.biome,
            self.owner,
            self.temperature,
            self.humidity,
            self.downfall,
            self.category
        )
    }
}

/// What an environmental effect does to the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EffectKind {
    #[default]
    Fog,
    Wind,
    Temperature,
    Precipitation,
    Sky,
    Ambient,
    Custom,
}

impl EffectKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fog => "fog",
            Self::Wind => "wind",
            Self::Temperature => "temperature",
            Self::Precipitation => "precipitation",
            Self::Sky => "sky",
            Self::Ambient => "ambient",
            Self::Custom => "custom",
        }
    }
}

impl fmt::Display for EffectKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An environmental effect a mod declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentEffect {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub kind: EffectKind,
    /// How long it lasts in ticks; zero means it does not expire.
    pub duration: u64,
    pub amplifier: u8,
    pub registered: bool,
}

impl EnvironmentEffect {
    #[must_use]
    pub fn new(id: &str, kind: EffectKind) -> Self {
        Self {
            id: id.to_owned(),
            name: id.to_owned(),
            owner: crate::metadata::id(),
            kind,
            duration: 0,
            amplifier: 0,
            registered: false,
        }
    }

    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    #[must_use]
    pub fn duration(mut self, duration_ticks: u64) -> Self {
        self.duration = duration_ticks;
        self
    }

    #[must_use]
    pub fn amplifier(mut self, amplifier: u8) -> Self {
        self.amplifier = amplifier;
        self
    }
}

impl fmt::Display for EnvironmentEffect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {}, {} ticks, amplifier {})",
            self.name, self.id, self.kind, self.duration, self.amplifier
        )
    }
}

/// One environmental effect running in an [`EffectSet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveEffect {
    pub id: String,
    pub kind: EffectKind,
    pub amplifier: u8,
    pub started: u64,
    /// Zero means it does not expire.
    pub duration: u64,
}

impl ActiveEffect {
    /// The tick it expires on, or `None` when it does not.
    #[must_use]
    pub fn expires_at(&self) -> Option<u64> {
        (self.duration != 0).then_some(self.started + self.duration)
    }

    /// Whether it is running at a tick.
    #[must_use]
    pub fn is_active_at(&self, tick: u64) -> bool {
        tick >= self.started && self.expires_at().is_none_or(|end| tick < end)
    }

    /// How long it has left at a tick, or `None` when it does not expire.
    #[must_use]
    pub fn remaining_at(&self, tick: u64) -> Option<u64> {
        self.expires_at().map(|end| end.saturating_sub(tick))
    }
}

impl fmt::Display for ActiveEffect {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, amplifier {}) since tick {}",
            self.id, self.kind, self.amplifier, self.started
        )
    }
}

/// The environment's clock, in ticks, a mod owns and moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EnvironmentClock {
    pub tick: u64,
}

impl EnvironmentClock {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn now(&self) -> u64 {
        self.tick
    }

    pub fn advance(&mut self, ticks: u64) {
        self.tick += ticks;
    }

    pub fn set(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn reset(&mut self) {
        self.tick = 0;
    }
}

/// The environmental effects a mod has running, over an [`EnvironmentClock`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectSet {
    pub effects: Vec<ActiveEffect>,
}

impl EffectSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a declared effect now, replacing one of the same id.
    pub fn apply(&mut self, effect: &EnvironmentEffect, clock: &EnvironmentClock) {
        self.apply_at(effect, clock.now());
    }

    /// Applies a declared effect as of a tick, replacing one of the same id.
    pub fn apply_at(&mut self, effect: &EnvironmentEffect, tick: u64) {
        self.effects.retain(|held| held.id != effect.id);
        self.effects.push(ActiveEffect {
            id: effect.id.clone(),
            kind: effect.kind,
            amplifier: effect.amplifier,
            started: tick,
            duration: effect.duration,
        });
    }

    /// Drops every effect that has run out at the clock's tick, answering how many went.
    #[must_use]
    pub fn expire(&mut self, clock: &EnvironmentClock) -> usize {
        let now = clock.now();
        let before = self.effects.len();
        self.effects
            .retain(|held| held.expires_at().is_none_or(|end| end > now));
        before - self.effects.len()
    }

    /// The effects running at a tick.
    #[must_use]
    pub fn active_at(&self, tick: u64) -> Vec<ActiveEffect> {
        self.effects
            .iter()
            .filter(|held| held.is_active_at(tick))
            .cloned()
            .collect()
    }

    /// The effects running now.
    #[must_use]
    pub fn active(&self, clock: &EnvironmentClock) -> Vec<ActiveEffect> {
        self.active_at(clock.now())
    }

    /// Whether a given effect is running at a tick.
    #[must_use]
    pub fn has_at(&self, id: &str, tick: u64) -> bool {
        self.effects
            .iter()
            .any(|held| held.id == id && held.is_active_at(tick))
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<ActiveEffect> {
        self.effects.iter().find(|held| held.id == id).cloned()
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.effects.len();
        self.effects.retain(|held| held.id != id);
        self.effects.len() != before
    }

    /// Takes every effect off, answering how many went.
    #[must_use]
    pub fn clear(&mut self) -> usize {
        let count = self.effects.len();
        self.effects.clear();
        count
    }
}

/// What kind of declaration an [`EnvironmentPiece`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum PieceKind {
    #[default]
    SkyRenderer,
    FogModifier,
    CloudRenderer,
    SkyEffect,
    AmbientSound,
}

impl PieceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SkyRenderer => "sky_renderer",
            Self::FogModifier => "fog_modifier",
            Self::CloudRenderer => "cloud_renderer",
            Self::SkyEffect => "sky_effect",
            Self::AmbientSound => "ambient_sound",
        }
    }
}

impl fmt::Display for PieceKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A sky, fog, cloud or sound declaration a mod ships, with the settings it gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPiece {
    pub id: String,
    pub name: String,
    pub kind: PieceKind,
    pub owner: String,
    pub settings: Vec<(String, String)>,
}

impl EnvironmentPiece {
    #[must_use]
    pub fn new(kind: PieceKind, id: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: id.to_owned(),
            kind,
            owner: crate::metadata::id(),
            settings: Vec::new(),
        }
    }

    #[must_use]
    pub fn named(mut self, name: &str) -> Self {
        self.name = name.to_owned();
        self
    }

    #[must_use]
    pub fn setting(mut self, key: &str, value: &str) -> Self {
        self.settings.retain(|(held, _)| held != key);
        self.settings.push((key.to_owned(), value.to_owned()));
        self
    }

    #[must_use]
    pub fn setting_of(&self, key: &str) -> Option<&str> {
        self.settings
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value.as_str())
    }
}

impl fmt::Display for EnvironmentPiece {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {})",
            self.name,
            self.id,
            self.kind.as_str()
        )
    }
}

fn weather_types() -> &'static Mutex<BTreeMap<String, WeatherType>> {
    static WEATHER: OnceLock<Mutex<BTreeMap<String, WeatherType>>> = OnceLock::new();
    WEATHER.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_weather_types<T>(body: impl FnOnce(&mut BTreeMap<String, WeatherType>) -> T) -> T {
    let mut guard = weather_types().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn climates() -> &'static Mutex<BTreeMap<String, BiomeClimate>> {
    static CLIMATES: OnceLock<Mutex<BTreeMap<String, BiomeClimate>>> = OnceLock::new();
    CLIMATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_climates<T>(body: impl FnOnce(&mut BTreeMap<String, BiomeClimate>) -> T) -> T {
    let mut guard = climates().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn environment_effects() -> &'static Mutex<BTreeMap<String, EnvironmentEffect>> {
    static EFFECTS: OnceLock<Mutex<BTreeMap<String, EnvironmentEffect>>> = OnceLock::new();
    EFFECTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_environment_effects<T>(
    body: impl FnOnce(&mut BTreeMap<String, EnvironmentEffect>) -> T,
) -> T {
    let mut guard = environment_effects()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn pieces() -> &'static Mutex<BTreeMap<(PieceKind, String), EnvironmentPiece>> {
    static PIECES: OnceLock<Mutex<BTreeMap<(PieceKind, String), EnvironmentPiece>>> =
        OnceLock::new();
    PIECES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_pieces<T>(body: impl FnOnce(&mut BTreeMap<(PieceKind, String), EnvironmentPiece>) -> T) -> T {
    let mut guard = pieces().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a weather kind, or replaces the one with that id.
pub fn register_weather(weather: WeatherType) -> bool {
    with_weather_types(|weather_types| weather_types.insert(weather.id.clone(), weather).is_none())
}

/// Forgets a weather kind.
pub fn unregister_weather(id: &str) -> bool {
    with_weather_types(|weather_types| weather_types.remove(id).is_some())
}

/// A weather kind a mod declared, by id.
#[must_use]
pub fn get_weather_type(id: &str) -> Option<WeatherType> {
    with_weather_types(|weather_types| weather_types.get(id).cloned())
}

/// Every weather kind a mod declared.
#[must_use]
pub fn get_weather_types() -> Vec<WeatherType> {
    with_weather_types(|weather_types| weather_types.values().cloned().collect())
}

/// Registers an environmental effect a mod declares.
pub fn register_weather_effect(effect: EnvironmentEffect) -> bool {
    with_environment_effects(|effects| effects.insert(effect.id.clone(), effect).is_none())
}

/// Forgets a declared environmental effect.
pub fn unregister_effect(id: &str) -> bool {
    with_environment_effects(|effects| effects.remove(id).is_some())
}

/// A declared environmental effect, by id.
#[must_use]
pub fn get_effect(id: &str) -> Option<EnvironmentEffect> {
    with_environment_effects(|effects| effects.get(id).cloned())
}

/// Every declared environmental effect.
#[must_use]
pub fn get_effects() -> Vec<EnvironmentEffect> {
    with_environment_effects(|effects| effects.values().cloned().collect())
}

/// Registers a climate a mod declares for a biome.
pub fn register_climate(climate: BiomeClimate) -> bool {
    with_climates(|climates| climates.insert(climate.biome.clone(), climate).is_none())
}

/// Forgets a declared climate.
pub fn unregister_climate(biome: &str) -> bool {
    with_climates(|climates| climates.remove(biome).is_some())
}

/// A declared climate, by biome id.
#[must_use]
pub fn get_climate(biome: &str) -> Option<BiomeClimate> {
    with_climates(|climates| climates.get(biome).cloned())
}

/// Every declared climate.
#[must_use]
pub fn get_climates() -> Vec<BiomeClimate> {
    with_climates(|climates| climates.values().cloned().collect())
}

/// The temperature of a declared biome climate.
#[must_use]
pub fn temperature_at(biome: &str) -> Option<f32> {
    get_climate(biome).map(|climate| climate.temperature)
}

/// The humidity of a declared biome climate.
#[must_use]
pub fn humidity_at(biome: &str) -> Option<f32> {
    get_climate(biome).map(|climate| climate.humidity)
}

/// The downfall of a declared biome climate.
#[must_use]
pub fn downfall_at(biome: &str) -> Option<f32> {
    get_climate(biome).map(|climate| climate.downfall)
}

/// What falls in a declared biome climate.
#[must_use]
pub fn precipitation_at(biome: &str) -> Option<Precipitation> {
    get_climate(biome)
        .as_ref()
        .map(BiomeClimate::precipitation)
}

/// Whether snow settles in a declared biome climate.
#[must_use]
pub fn snows_at(biome: &str) -> bool {
    get_climate(biome).as_ref().is_some_and(BiomeClimate::snows)
}

/// Registers a sky, fog, cloud or sound declaration a mod ships.
pub fn register_piece(piece: EnvironmentPiece) -> bool {
    with_pieces(|pieces| pieces.insert((piece.kind, piece.id.clone()), piece).is_none())
}

/// Forgets a declaration.
pub fn unregister_piece(kind: PieceKind, id: &str) -> bool {
    with_pieces(|pieces| pieces.remove(&(kind, id.to_owned())).is_some())
}

/// A declaration of a kind, by id.
#[must_use]
pub fn get_piece(kind: PieceKind, id: &str) -> Option<EnvironmentPiece> {
    with_pieces(|pieces| pieces.get(&(kind, id.to_owned())).cloned())
}

/// Every declaration, or every one of a kind.
#[must_use]
pub fn get_pieces(kind: Option<PieceKind>) -> Vec<EnvironmentPiece> {
    with_pieces(|pieces| {
        pieces
            .values()
            .filter(|piece| kind.is_none_or(|kind| piece.kind == kind))
            .cloned()
            .collect()
    })
}

/// Registers a sky renderer a mod ships.
pub fn register_sky_renderer(piece: EnvironmentPiece) -> bool {
    register_piece(piece)
}

/// Registers a fog modifier a mod ships.
pub fn register_fog_modifier(piece: EnvironmentPiece) -> bool {
    register_piece(piece)
}

/// Registers a cloud renderer a mod ships.
pub fn register_cloud_renderer(piece: EnvironmentPiece) -> bool {
    register_piece(piece)
}

/// Registers a sky effect a mod ships.
pub fn register_sky_effect(piece: EnvironmentPiece) -> bool {
    register_piece(piece)
}

/// Registers an ambient sound a mod ships.
pub fn register_ambient_sound(piece: EnvironmentPiece) -> bool {
    register_piece(piece)
}

/// Why a question about the client's environment has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "weather" => {
            "the client's weather, its rain and thunder, its sky, its fog, its sun and moon, \
             its stars and its time of day are drawn and kept inside the client's own world \
             renderer"
                .to_owned()
        }
        "light" => {
            "the light at a position is the client's world state; reading it needs the hooks \
             section 10 lists as never"
                .to_owned()
        }
        "climate" => {
            "the climate at a position is looked up in the client's biome, which this host \
             cannot see"
                .to_owned()
        }
        "effects" => {
            "the effects the client is running are its own list; this host only knows the ones \
             a mod applies to itself"
                .to_owned()
        }
        other => format!("{other} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["weather", "light", "climate", "effects"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The client's own environment, which this host can neither read nor change. Every one warns
/// and answers nothing.
pub mod known_only_to_the_client {
    use super::{why_unknown, WeatherState};

    /// The weather the client is in.
    #[must_use]
    pub fn get_weather() -> Option<WeatherState> {
        crate::warn!("a mod asked this host for the client's weather: {}", why_unknown("weather"));
        None
    }

    /// Asks the client to change its weather.
    pub fn set_weather(_weather: &str) -> bool {
        crate::warn!(
            "a mod asked this host to set the client's weather: {}",
            why_unknown("weather")
        );
        false
    }

    /// The client's rain level.
    #[must_use]
    pub fn get_rain_level() -> Option<f64> {
        crate::warn!(
            "a mod asked this host for the client's rain level: {}",
            why_unknown("weather")
        );
        None
    }

    /// Sets the client's rain level.
    pub fn set_rain_level(_level: f64) -> bool {
        crate::warn!(
            "a mod asked this host to set the client's rain level: {}",
            why_unknown("weather")
        );
        false
    }

    /// The client's thunder level.
    #[must_use]
    pub fn get_thunder_level() -> Option<f64> {
        crate::warn!(
            "a mod asked this host for the client's thunder level: {}",
            why_unknown("weather")
        );
        None
    }

    /// Sets the client's thunder level.
    pub fn set_thunder_level(_level: f64) -> bool {
        crate::warn!(
            "a mod asked this host to set the client's thunder level: {}",
            why_unknown("weather")
        );
        false
    }

    /// The client's temperature.
    #[must_use]
    pub fn get_temperature() -> Option<f32> {
        crate::warn!(
            "a mod asked this host for the client's temperature: {}",
            why_unknown("climate")
        );
        None
    }

    /// The client's humidity.
    #[must_use]
    pub fn get_humidity() -> Option<f32> {
        crate::warn!(
            "a mod asked this host for the client's humidity: {}",
            why_unknown("climate")
        );
        None
    }

    /// The wind in the client's world.
    #[must_use]
    pub fn get_wind() -> Option<(f64, f64, f64)> {
        crate::warn!("a mod asked this host for the client's wind: {}", why_unknown("weather"));
        None
    }

    /// The colour the client's sky is drawn in.
    #[must_use]
    pub fn get_sky_color() -> Option<u32> {
        crate::warn!(
            "a mod asked this host for the client's sky colour: {}",
            why_unknown("weather")
        );
        None
    }

    /// The colour the client's fog is drawn in.
    #[must_use]
    pub fn get_fog_color() -> Option<u32> {
        crate::warn!(
            "a mod asked this host for the client's fog colour: {}",
            why_unknown("weather")
        );
        None
    }

    /// How thick the client's fog is.
    #[must_use]
    pub fn get_fog_density() -> Option<f32> {
        crate::warn!(
            "a mod asked this host for the client's fog density: {}",
            why_unknown("weather")
        );
        None
    }

    /// Where the client's sun is in the sky.
    #[must_use]
    pub fn get_sun_angle() -> Option<f32> {
        crate::warn!(
            "a mod asked this host for the client's sun angle: {}",
            why_unknown("weather")
        );
        None
    }

    /// Which phase the client's moon is in.
    #[must_use]
    pub fn get_moon_phase() -> Option<u8> {
        crate::warn!(
            "a mod asked this host for the client's moon phase: {}",
            why_unknown("weather")
        );
        None
    }

    /// How visible the client's stars are.
    #[must_use]
    pub fn get_star_visibility() -> Option<f32> {
        crate::warn!(
            "a mod asked this host for the client's stars: {}",
            why_unknown("weather")
        );
        None
    }

    /// The client's time of day, in ticks.
    #[must_use]
    pub fn get_day_time() -> Option<u64> {
        crate::warn!(
            "a mod asked this host for the client's time of day: {}",
            why_unknown("weather")
        );
        None
    }

    /// Sets the client's time of day.
    pub fn set_day_time(_ticks: u64) -> bool {
        crate::warn!(
            "a mod asked this host to set the client's time of day: {}",
            why_unknown("weather")
        );
        false
    }

    /// How long the client's weather will last.
    #[must_use]
    pub fn get_weather_duration() -> Option<u64> {
        crate::warn!(
            "a mod asked this host for the client's weather duration: {}",
            why_unknown("weather")
        );
        None
    }

    /// Sets how long the client's weather will last.
    pub fn set_weather_duration(_ticks: u64) -> bool {
        crate::warn!(
            "a mod asked this host to set the client's weather duration: {}",
            why_unknown("weather")
        );
        false
    }

    /// The client's light at a position.
    #[must_use]
    pub fn get_light(_x: i32, _y: i32, _z: i32) -> Option<u8> {
        crate::warn!(
            "a mod asked this host for the client's light at a position: {}",
            why_unknown("light")
        );
        None
    }

    /// The effects the client is running.
    #[must_use]
    pub fn get_active_effects() -> Vec<String> {
        crate::warn!(
            "a mod asked this host for the client's effects: {}",
            why_unknown("effects")
        );
        Vec::new()
    }
}

pub use known_only_to_the_client::{
    get_day_time, get_fog_color, get_fog_density, get_humidity, get_moon_phase, get_rain_level,
    get_sky_color, get_star_visibility, get_sun_angle, get_temperature, get_thunder_level,
    get_weather, get_weather_duration, get_wind, set_day_time, set_rain_level,
    set_thunder_level, set_weather, set_weather_duration,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with_weather_types(|weather_types| weather_types.clear());
        with_climates(|climates| climates.clear());
        with_environment_effects(|effects| effects.clear());
        with_pieces(|pieces| pieces.clear());
    }

    #[test]
    fn weather_ramps_and_turns_over_with_time() {
        let mut machine = WeatherMachine::with_weather(7, WeatherState::Rain, 400);
        assert_eq!(machine.weather(), WeatherState::Rain);
        assert_eq!(machine.rain_level(), 0.0);
        assert!(machine.is_precipitating());
        machine.advance(50);
        assert_eq!(machine.rain_level(), 0.5, "half a ramp after 50 of 100 ticks");
        assert_eq!(machine.thunder_level(), 0.0);
        machine.advance(50);
        assert_eq!(machine.rain_level(), 1.0, "the ramp finishes after 100");
        assert_eq!(machine.remaining(), 300);

        machine.set_weather(WeatherState::Clear);
        assert_eq!(machine.weather(), WeatherState::Clear);
        machine.advance(50);
        assert_eq!(machine.rain_level(), 0.5, "it ramps back down, it does not jump");
        machine.advance(50);
        assert_eq!(machine.rain_level(), 0.0);

        let mut storm = WeatherMachine::with_weather(1, WeatherState::Thunder, 400);
        storm.advance(100);
        assert_eq!(storm.rain_level(), 1.0);
        assert_eq!(storm.thunder_level(), 1.0);

        let mut seen = BTreeSet::new();
        let mut rolling = WeatherMachine::new(5);
        for _ in 0..400 {
            seen.insert(rolling.weather().as_str());
            rolling.advance(100);
        }
        assert!(seen.len() >= 2, "the machine moves between states: {seen:?}");
        assert!(rolling.step() > 0, "it turned over at least once");
    }

    #[test]
    fn the_same_seed_replays_the_same_weather() {
        let mut left = WeatherMachine::new(42);
        let mut right = WeatherMachine::new(42);
        let mut left_states = Vec::new();
        let mut right_states = Vec::new();
        for _ in 0..64 {
            left_states.push(left.weather());
            right_states.push(right.weather());
            left.advance(500);
            right.advance(500);
        }
        assert_eq!(left_states, right_states, "one seed, one sequence");
        assert_eq!(left, right, "and the machines stay identical");

        let mut other = WeatherMachine::new(43);
        let mut other_states = Vec::new();
        for _ in 0..64 {
            other_states.push(other.weather());
            other.advance(500);
        }
        assert_ne!(left_states, other_states, "another seed tells another story");
    }

    #[test]
    fn light_falls_off_one_per_block_and_clamps_at_both_ends() {
        let light = LightModel::new().with_source(LightSource::block((0, 0, 0), 15));
        assert_eq!(light.block_light_at(0, 0, 0), 15);
        assert_eq!(light.block_light_at(1, 0, 0), 14, "one block away, one level less");
        assert_eq!(light.block_light_at(5, 0, 0), 10, "five blocks away, five less");
        assert_eq!(light.block_light_at(15, 0, 0), 0);
        assert_eq!(light.block_light_at(16, 0, 0), 0);

        assert_eq!(falloff(15, 0), 15);
        assert_eq!(falloff(20, 0), MAX_LIGHT, "clamped up to 15");
        assert_eq!(falloff(5, 6), 0, "clamped down to 0");
        assert_eq!(falloff(MAX_LIGHT, u32::MAX), 0);
    }

    #[test]
    fn sky_light_is_reduced_by_the_time_of_day() {
        assert_eq!(sky_darken(6000), 0, "noon is full brightness");
        assert_eq!(sky_darken(18_000), MAX_SKY_DARKEN, "midnight dims the most");
        assert_eq!(sky_darken(0), 0);
        assert_eq!(sky_light(15, 6000), 15);
        assert_eq!(sky_light(15, 18_000), 4, "15 minus 11");
        assert_eq!(sky_light(3, 18_000), 0, "cannot go below zero");

        let sky = LightModel::new().with_source(LightSource::sky((0, 0, 0), 15));
        assert_eq!(sky.clone().with_day_time(6000).light_at(0, 0, 0), 15);
        assert_eq!(sky.with_day_time(18_000).light_at(0, 0, 0), 4);
    }

    #[test]
    fn block_and_sky_light_combine_by_the_brighter() {
        let model = LightModel::new()
            .with_source(LightSource::block((0, 0, 0), 10))
            .with_source(LightSource::sky((0, 0, 0), 15))
            .with_day_time(18_000);
        assert_eq!(model.block_light_at(0, 0, 0), 10);
        assert_eq!(model.sky_light_at(0, 0, 0), 4, "the moonlight");
        assert_eq!(model.light_at(0, 0, 0), 10, "the brighter of the two wins");

        assert_eq!(combine(3, 4), 4);
        assert_eq!(combine(10, 15), 15);
        assert_eq!(combine(20, 2), MAX_LIGHT, "clamped");
    }

    #[test]
    fn a_biome_climate_reads_back_and_decides_what_falls() {
        let _serial = serial();
        fresh();
        assert!(register_climate(
            BiomeClimate::new("hello:tiaga", -0.5)
                .humidity(0.8)
                .downfall(0.4)
        ));
        let climate = get_climate("hello:tiaga").expect("registered");
        assert_eq!(climate.temperature, -0.5);
        assert_eq!(climate.humidity, 0.8);
        assert_eq!(climate.downfall, 0.4);
        assert_eq!(climate.category, ClimateCategory::Frozen);
        assert_eq!(temperature_at("hello:tiaga"), Some(-0.5));
        assert_eq!(humidity_at("hello:tiaga"), Some(0.8));
        assert_eq!(downfall_at("hello:tiaga"), Some(0.4));
        assert_eq!(precipitation_at("hello:tiaga"), Some(Precipitation::Snow));
        assert!(snows_at("hello:tiaga"));
        assert_eq!(get_climates().len(), 1);
        assert!(climate.to_string().contains("hello:tiaga"));

        register_climate(BiomeClimate::new("hello:desert", 2.0).humidity(0.0).downfall(0.0));
        assert_eq!(
            get_climate("hello:desert").map(|climate| climate.category),
            Some(ClimateCategory::Hot)
        );
        assert_eq!(precipitation_at("hello:desert"), Some(Precipitation::None));
        assert!(!snows_at("hello:desert"));

        register_climate(BiomeClimate::new("hello:plains", 0.5).humidity(0.4).downfall(0.3));
        assert_eq!(precipitation_at("hello:plains"), Some(Precipitation::Rain));
        assert_eq!(temperature_at("hello:missing"), None);

        assert!(unregister_climate("hello:tiaga"));
        assert!(!unregister_climate("hello:tiaga"));
    }

    #[test]
    fn effects_apply_query_and_expire_on_the_clock() {
        let mut clock = EnvironmentClock::new();
        assert_eq!(clock.now(), 0);
        let fog = EnvironmentEffect::new("hello:fog", EffectKind::Fog)
            .named("Fog")
            .duration(100)
            .amplifier(2);
        assert_eq!(fog.duration, 100);
        assert_eq!(fog.amplifier, 2);

        let mut running = EffectSet::new();
        running.apply(&fog, &clock);
        assert!(running.has_at("hello:fog", 0));
        assert_eq!(running.active_at(99).len(), 1, "still running one tick before it ends");
        assert_eq!(running.active_at(100).len(), 0, "gone at exactly its duration");
        assert_eq!(
            running.get("hello:fog").and_then(|effect| effect.expires_at()),
            Some(100)
        );
        assert_eq!(running.active_at(0)[0].amplifier, 2);

        clock.advance(99);
        assert_eq!(running.expire(&clock), 0, "one tick short");
        assert!(running.has_at("hello:fog", clock.now()));
        clock.advance(1);
        assert_eq!(running.expire(&clock), 1, "it expires at exactly its duration");
        assert!(running.get("hello:fog").is_none());

        let permanent = EnvironmentEffect::new("hello:sky", EffectKind::Sky);
        running.apply(&permanent, &clock);
        clock.advance(1_000_000);
        assert_eq!(running.expire(&clock), 0, "a zero duration never expires");
        assert!(running.has_at("hello:sky", clock.now()));
        assert_eq!(running.clear(), 1);
    }

    #[test]
    fn weather_and_renderer_declarations_read_back() {
        let _serial = serial();
        fresh();
        assert!(register_weather(
            WeatherType::new("hello:storm", "Storm", WeatherState::Thunder)
                .rain_level(1.0)
                .thunder_level(1.0)
                .durations(2_000, 4_000)
        ));
        assert!(register_weather_effect(
            EnvironmentEffect::new("hello:fog", EffectKind::Fog)
                .duration(600)
                .amplifier(1)
        ));
        assert!(register_sky_renderer(
            EnvironmentPiece::new(PieceKind::SkyRenderer, "hello:sky")
                .named("Sky")
                .setting("shader", "hello:sky")
        ));
        assert!(register_fog_modifier(EnvironmentPiece::new(
            PieceKind::FogModifier,
            "hello:fog"
        )));
        assert!(register_cloud_renderer(EnvironmentPiece::new(
            PieceKind::CloudRenderer,
            "hello:clouds"
        )));
        assert!(register_sky_effect(EnvironmentPiece::new(
            PieceKind::SkyEffect,
            "hello:aurora"
        )));
        assert!(register_ambient_sound(EnvironmentPiece::new(
            PieceKind::AmbientSound,
            "hello:wind"
        )));

        assert_eq!(
            get_weather_type("hello:storm").map(|weather| weather.state),
            Some(WeatherState::Thunder)
        );
        assert_eq!(get_weather_types().len(), 1);
        assert_eq!(get_effect("hello:fog").map(|effect| effect.duration), Some(600));
        assert_eq!(get_effects().len(), 1);
        assert_eq!(get_pieces(None).len(), 5);
        assert_eq!(get_pieces(Some(PieceKind::SkyRenderer)).len(), 1);
        assert_eq!(
            get_piece(PieceKind::SkyRenderer, "hello:sky")
                .and_then(|piece| piece.setting_of("shader").map(str::to_owned)),
            Some("hello:sky".to_owned())
        );
        assert_eq!(
            get_piece(PieceKind::SkyRenderer, "hello:sky")
                .and_then(|piece| piece.setting_of("nothing").map(str::to_owned)),
            None
        );
        assert_eq!(PieceKind::FogModifier.as_str(), "fog_modifier");
        assert_eq!(WeatherState::Rain.as_str(), "rain");
        assert!(unregister_weather("hello:storm"));
        assert!(!unregister_weather("hello:storm"));
        assert!(unregister_piece(PieceKind::SkyEffect, "hello:aurora"));
        assert_eq!(get_pieces(Some(PieceKind::SkyEffect)).len(), 0);
        assert_eq!(WeatherState::Clear.to_string(), "clear");
        assert_eq!(EffectKind::Ambient.to_string(), "ambient");
        assert_eq!(Precipitation::Snow.to_string(), "snow");
        assert_eq!(ClimateCategory::Hot.as_str(), "hot");
    }

    #[test]
    fn the_clients_environment_is_refused_with_a_reason() {
        let _serial = serial();
        assert!(known_only_to_the_client::get_weather().is_none());
        assert!(!known_only_to_the_client::set_weather("rain"));
        assert!(known_only_to_the_client::get_rain_level().is_none());
        assert!(!known_only_to_the_client::set_rain_level(1.0));
        assert!(known_only_to_the_client::get_thunder_level().is_none());
        assert!(!known_only_to_the_client::set_thunder_level(1.0));
        assert!(known_only_to_the_client::get_temperature().is_none());
        assert!(known_only_to_the_client::get_humidity().is_none());
        assert!(known_only_to_the_client::get_wind().is_none());
        assert!(known_only_to_the_client::get_sky_color().is_none());
        assert!(known_only_to_the_client::get_fog_color().is_none());
        assert!(known_only_to_the_client::get_fog_density().is_none());
        assert!(known_only_to_the_client::get_sun_angle().is_none());
        assert!(known_only_to_the_client::get_moon_phase().is_none());
        assert!(known_only_to_the_client::get_star_visibility().is_none());
        assert!(known_only_to_the_client::get_day_time().is_none());
        assert!(!known_only_to_the_client::set_day_time(6000));
        assert!(known_only_to_the_client::get_weather_duration().is_none());
        assert!(!known_only_to_the_client::set_weather_duration(100));
        assert!(known_only_to_the_client::get_light(0, 64, 0).is_none());
        assert!(known_only_to_the_client::get_active_effects().is_empty());

        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
