//! Audio: the mixer a mod builds, and the device it cannot play on.
//!
//! The model here is real. A mod registers the sounds it ships — id, source path, duration,
//! category, default volume and pitch, looping — and loads, unloads and reads them back,
//! with `get_duration` answering the real number it declared. It creates buses and sources,
//! and the volume that reaches a source is genuine arithmetic: the chain walks
//! source → bus → category → master and multiplies, every setter clamps to a sane range, and
//! `resolve_volume` answers the product. A `play` returns a handle and records a playback
//! the caller can move through playing, paused and stopped, advance through the sound,
//! loop, pan, pitch and stop.
//!
//! The spatial maths is real too: a listener has a position and an orientation, a source has
//! a position, `set_distance` sets the near and far ends, attenuation falls off along a
//! documented curve clamped to `[0, 1]`, and panning comes from the source's position against
//! the listener's right vector. `play_3d` and `play_at` compute the resulting volume and pan
//! rather than guessing them.
//!
//! What cannot be done is hearing any of it. The client owns the audio device, so nothing is
//! written to an output; the client's own master volume and sound list are inside its sound
//! engine; its reverb and filter processing runs there too; and this host has no decoder, so a
//! sound loaded from a real file is not decoded — the duration is what the mod declares, and a
//! mod that ships an ogg or wav decodes it itself. Those are refused with their reasons.
//!
//! ```
//! use cinnaloader::audio::{self, PlaybackState, Sound, SoundCategory};
//!
//! audio::register_sound(
//!     Sound::new("hello:bell", "sounds/bell.ogg")
//!         .duration(1000)
//!         .category(SoundCategory::Player),
//! );
//! assert!(audio::load_sound("hello:bell"));
//! assert_eq!(audio::get_duration("hello:bell"), Some(1000));
//! let handle = audio::play("hello:bell").expect("a loaded sound plays");
//! assert_eq!(audio::get_playback_state(handle), Some(PlaybackState::Playing));
//! assert!(!audio::set_output_device("speakers"), "the client owns the device");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// Which part of the game a sound or a bus belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum SoundCategory {
    Ambient,
    #[default]
    Blocks,
    Hostile,
    Music,
    Neutral,
    Player,
    Records,
    Ui,
    Voice,
    Weather,
}

impl SoundCategory {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ambient => "ambient",
            Self::Blocks => "blocks",
            Self::Hostile => "hostile",
            Self::Music => "music",
            Self::Neutral => "neutral",
            Self::Player => "player",
            Self::Records => "records",
            Self::Ui => "ui",
            Self::Voice => "voice",
            Self::Weather => "weather",
        }
    }

    /// Every category, which is what a settings screen lists.
    #[must_use]
    pub fn all() -> Vec<Self> {
        vec![
            Self::Ambient,
            Self::Blocks,
            Self::Hostile,
            Self::Music,
            Self::Neutral,
            Self::Player,
            Self::Records,
            Self::Ui,
            Self::Voice,
            Self::Weather,
        ]
    }

    /// Parses the name a caller writes, falling back to blocks.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "ambient" => Self::Ambient,
            "hostile" => Self::Hostile,
            "music" => Self::Music,
            "neutral" => Self::Neutral,
            "player" => Self::Player,
            "records" => Self::Records,
            "ui" => Self::Ui,
            "voice" => Self::Voice,
            "weather" => Self::Weather,
            _ => Self::Blocks,
        }
    }
}

impl fmt::Display for SoundCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a source's loudness falls off with distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Attenuation {
    /// No rolloff: the source is as loud at any distance.
    None,
    /// Reaches silence at the far end of the range.
    #[default]
    Linear,
    /// Falls as `min / distance` and never quite reaches silence.
    Inverse,
}

impl Attenuation {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Linear => "linear",
            Self::Inverse => "inverse",
        }
    }

    /// Parses the name a caller writes, falling back to linear.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "none" => Self::None,
            "inverse" => Self::Inverse,
            _ => Self::Linear,
        }
    }
}

impl fmt::Display for Attenuation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    if length <= f32::EPSILON {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / length, v[1] / length, v[2] / length]
    }
}

/// Where a listener is and which way it faces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        }
    }
}

impl Listener {
    #[must_use]
    pub fn new(position: [f32; 3], forward: [f32; 3], up: [f32; 3]) -> Self {
        Self {
            position,
            forward,
            up,
        }
    }

    /// The listener's right, which is the axis panning is measured against.
    #[must_use]
    pub fn right(&self) -> [f32; 3] {
        normalized(cross(self.forward, self.up))
    }

    /// How far a point is from the listener.
    #[must_use]
    pub fn distance_to(&self, position: [f32; 3]) -> f32 {
        dot(sub(position, self.position), sub(position, self.position)).sqrt()
    }
}

/// A sound a mod ships.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub id: String,
    pub owner: String,
    /// Where the sample lives, as the mod wrote it.
    pub source: String,
    /// How long it runs, in milliseconds, which is what the mod declares.
    pub duration: u64,
    pub category: SoundCategory,
    pub volume: f32,
    pub pitch: f32,
    pub looping: bool,
    /// Whether it has been loaded, which is the mod's own lifecycle.
    pub loaded: bool,
}

impl Sound {
    #[must_use]
    pub fn new(id: &str, source: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            source: source.to_owned(),
            duration: 0,
            category: SoundCategory::Blocks,
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            loaded: false,
        }
    }

    #[must_use]
    pub fn duration(mut self, millis: u64) -> Self {
        self.duration = millis;
        self
    }

    #[must_use]
    pub fn category(mut self, category: SoundCategory) -> Self {
        self.category = category;
        self
    }

    #[must_use]
    pub fn volume(mut self, volume: f32) -> Self {
        self.volume = sane(volume, 0.0, 1.0);
        self
    }

    #[must_use]
    pub fn pitch(mut self, pitch: f32) -> Self {
        self.pitch = sane(pitch, PITCH_MIN, PITCH_MAX);
        self
    }

    #[must_use]
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }
}

impl fmt::Display for Sound {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {} ms, {}, {})",
            self.id,
            self.source,
            self.duration,
            self.category,
            if self.loaded { "loaded" } else { "unloaded" }
        )
    }
}

/// A bus a mod routes sources through, with a volume and effect settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Bus {
    pub id: u64,
    pub name: String,
    pub volume: f32,
    pub category: SoundCategory,
    /// The bus it feeds into, when it is not the top.
    pub parent: Option<u64>,
    pub reverb: f32,
    pub filter: String,
}

impl Bus {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            id: 0,
            name: name.to_owned(),
            volume: 1.0,
            category: SoundCategory::Blocks,
            parent: None,
            reverb: 0.0,
            filter: String::new(),
        }
    }

    #[must_use]
    pub fn volume(mut self, volume: f32) -> Self {
        self.volume = sane(volume, 0.0, 1.0);
        self
    }

    #[must_use]
    pub fn category(mut self, category: SoundCategory) -> Self {
        self.category = category;
        self
    }

    #[must_use]
    pub fn parent(mut self, bus: BusHandle) -> Self {
        self.parent = Some(bus.0);
        self
    }

    #[must_use]
    pub fn reverb(mut self, amount: f32) -> Self {
        self.reverb = sane(amount, 0.0, 1.0);
        self
    }

    #[must_use]
    pub fn filter(mut self, filter: &str) -> Self {
        self.filter = filter.to_owned();
        self
    }
}

impl fmt::Display for Bus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} (volume {:.2}, {}, {} parent)",
            self.name,
            self.volume,
            self.category,
            if self.parent.is_some() { "with a" } else { "no" }
        )
    }
}

/// A source a mod plays a sound through: where it sits and how it sounds.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub id: u64,
    pub sound: String,
    pub bus: Option<u64>,
    pub category: SoundCategory,
    pub volume: f32,
    pub pitch: f32,
    pub position: [f32; 3],
    /// A manual pan, used when spatialization is off.
    pub pan: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    pub attenuation: Attenuation,
    /// Whether the position drives the pan and the rolloff.
    pub spatialization: bool,
    pub looping: bool,
}

impl Source {
    #[must_use]
    pub fn new(sound: &str) -> Self {
        Self {
            id: 0,
            sound: sound.to_owned(),
            bus: None,
            category: SoundCategory::Blocks,
            volume: 1.0,
            pitch: 1.0,
            position: [0.0, 0.0, 0.0],
            pan: 0.0,
            min_distance: 1.0,
            max_distance: 16.0,
            attenuation: Attenuation::Linear,
            spatialization: false,
            looping: false,
        }
    }

    #[must_use]
    pub fn bus(mut self, bus: BusHandle) -> Self {
        self.bus = Some(bus.0);
        self
    }

    #[must_use]
    pub fn category(mut self, category: SoundCategory) -> Self {
        self.category = category;
        self
    }

    #[must_use]
    pub fn volume(mut self, volume: f32) -> Self {
        self.volume = sane(volume, 0.0, 1.0);
        self
    }

    #[must_use]
    pub fn pitch(mut self, pitch: f32) -> Self {
        self.pitch = sane(pitch, PITCH_MIN, PITCH_MAX);
        self
    }

    #[must_use]
    pub fn position(mut self, position: [f32; 3]) -> Self {
        self.position = position;
        self
    }

    #[must_use]
    pub fn distance(mut self, min_distance: f32, max_distance: f32) -> Self {
        self.min_distance = sane(min_distance, 0.0, f32::MAX);
        self.max_distance = sane(max_distance, self.min_distance, f32::MAX);
        self
    }

    #[must_use]
    pub fn attenuation(mut self, attenuation: Attenuation) -> Self {
        self.attenuation = attenuation;
        self
    }

    #[must_use]
    pub fn spatialization(mut self, on: bool) -> Self {
        self.spatialization = on;
        self
    }

    #[must_use]
    pub fn looping(mut self, on: bool) -> Self {
        self.looping = on;
        self
    }

    /// How far the source is from a listener.
    #[must_use]
    pub fn distance_to(&self, listener: &Listener) -> f32 {
        listener.distance_to(self.position)
    }

    /// The factor a distance leaves of the source's volume, always within `[0, 1]`.
    ///
    /// Linear falls evenly from one at `min_distance` to zero at `max_distance` and stays
    /// silent beyond it; inverse falls as `min_distance / distance` and never quite reaches
    /// zero; none leaves the volume alone.
    #[must_use]
    pub fn attenuation_at(&self, distance: f32) -> f32 {
        let distance = distance.max(0.0);
        match self.attenuation {
            Attenuation::None => 1.0,
            Attenuation::Linear => {
                if distance <= self.min_distance {
                    1.0
                } else if distance >= self.max_distance {
                    0.0
                } else {
                    1.0 - (distance - self.min_distance) / (self.max_distance - self.min_distance)
                }
            }
            Attenuation::Inverse => {
                if distance <= self.min_distance {
                    1.0
                } else {
                    (self.min_distance / distance).clamp(0.0, 1.0)
                }
            }
        }
    }

    /// Where the pan lands, from hard left to hard right, against the listener's right.
    #[must_use]
    pub fn pan_towards(&self, listener: &Listener) -> f32 {
        let towards = normalized(sub(self.position, listener.position));
        dot(towards, listener.right()).clamp(-1.0, 1.0)
    }
}

impl fmt::Display for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} -> {} (volume {:.2}, pan {:.2}, {})",
            self.id,
            self.sound,
            self.volume,
            self.pan,
            self.attenuation
        )
    }
}

/// The three states a playback can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
}

impl PlaybackState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }
}

impl fmt::Display for PlaybackState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A playback a mod started, with the state and the resolved volume and pan.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundPlayback {
    pub handle: u64,
    pub sound: String,
    pub source: Option<u64>,
    /// The volume it reached after the chain and the rolloff.
    pub volume: f32,
    pub pitch: f32,
    pub pan: f32,
    pub looping: bool,
    pub spatial: bool,
    pub state: PlaybackState,
    /// Where it is in the sound, in milliseconds.
    pub time: u64,
    pub position: [f32; 3],
}

impl fmt::Display for SoundPlayback {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "playback#{} {} (volume {:.2}, pan {:.2}, {})",
            self.handle, self.sound, self.volume, self.pan, self.state
        )
    }
}

/// A source a mod created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceHandle(pub u64);

impl fmt::Display for SourceHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "source#{}", self.0)
    }
}

/// A bus a mod created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BusHandle(pub u64);

impl fmt::Display for BusHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "bus#{}", self.0)
    }
}

/// A playback a mod started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlaybackHandle(pub u64);

impl fmt::Display for PlaybackHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "playback#{}", self.0)
    }
}

const PITCH_MIN: f32 = 0.05;
const PITCH_MAX: f32 = 4.0;

/// Clamps a value to a range, folding a NaN back to the low end.
fn sane(value: f32, low: f32, high: f32) -> f32 {
    if value.is_nan() {
        low
    } else {
        value.clamp(low, high)
    }
}

#[derive(Debug, Clone)]
struct State {
    sounds: BTreeMap<String, Sound>,
    sources: BTreeMap<u64, Source>,
    buses: BTreeMap<u64, Bus>,
    playbacks: BTreeMap<u64, SoundPlayback>,
    master: f32,
    categories: BTreeMap<SoundCategory, f32>,
    listener: Listener,
    next_source: u64,
    next_bus: u64,
    next_playback: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sounds: BTreeMap::new(),
            sources: BTreeMap::new(),
            buses: BTreeMap::new(),
            playbacks: BTreeMap::new(),
            master: 1.0,
            categories: BTreeMap::new(),
            listener: Listener::default(),
            next_source: 1,
            next_bus: 1,
            next_playback: 1,
        }
    }
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn with<T>(body: impl FnOnce(&mut State) -> T) -> T {
    let mut guard = state().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn category_volume(state: &State, category: SoundCategory) -> f32 {
    state
        .categories
        .get(&category)
        .copied()
        .unwrap_or(1.0)
}

/// The product of the whole chain a source sits on: master × bus(es) × category × source.
fn resolve(state: &State, source: &Source) -> f32 {
    let mut volume = state.master;
    let mut category = source.category;
    let mut current = source.bus;
    let mut walked = 0;
    while let Some(id) = current {
        let Some(bus) = state.buses.get(&id) else {
            break;
        };
        volume *= bus.volume;
        category = bus.category;
        current = bus.parent;
        walked += 1;
        if walked > 64 {
            break;
        }
    }
    volume * category_volume(state, category) * source.volume
}

fn start(
    state: &mut State,
    sound_id: &str,
    source: Option<&Source>,
    position: [f32; 3],
    spatial: bool,
) -> Option<PlaybackHandle> {
    let sound = state.sounds.get(sound_id)?.clone();
    if !sound.loaded {
        return None;
    }
    let (volume, pan, pitch, looping) = match source {
        Some(source) => {
            let (gain, pan) = if source.spatialization {
                (
                    source.attenuation_at(source.distance_to(&state.listener)),
                    source.pan_towards(&state.listener),
                )
            } else {
                (1.0, source.pan)
            };
            (resolve(state, source) * gain, pan, source.pitch, source.looping)
        }
        None => {
            let probe = Source::new(sound_id)
                .volume(sound.volume)
                .pitch(sound.pitch)
                .position(position)
                .attenuation(Attenuation::Linear)
                .spatialization(spatial);
            let gain = if spatial {
                probe.attenuation_at(probe.distance_to(&state.listener))
            } else {
                1.0
            };
            let pan = if spatial {
                probe.pan_towards(&state.listener)
            } else {
                0.0
            };
            let volume = state.master * category_volume(state, sound.category) * sound.volume * gain;
            (volume, pan, sound.pitch, sound.looping)
        }
    };
    let handle = state.next_playback;
    state.next_playback += 1;
    state.playbacks.insert(
        handle,
        SoundPlayback {
            handle,
            sound: sound_id.to_owned(),
            source: source.map(|source| source.id),
            volume,
            pitch,
            pan: sane(pan, -1.0, 1.0),
            looping,
            spatial,
            state: PlaybackState::Playing,
            time: 0,
            position,
        },
    );
    Some(PlaybackHandle(handle))
}

/// Registers a sound, or replaces the one with that id.
pub fn register_sound(sound: Sound) -> bool {
    with(|state| state.sounds.insert(sound.id.clone(), sound).is_none())
}

/// Forgets a sound.
pub fn unregister_sound(id: &str) -> bool {
    with(|state| state.sounds.remove(id).is_some())
}

/// A registered sound, by id.
#[must_use]
pub fn get_sound(id: &str) -> Option<Sound> {
    with(|state| state.sounds.get(id).cloned())
}

/// Every sound a mod registered.
#[must_use]
pub fn get_sounds() -> Vec<Sound> {
    with(|state| state.sounds.values().cloned().collect())
}

/// Marks a sound loaded, which is the mod's own lifecycle. `false` when there is no such sound.
pub fn load_sound(id: &str) -> bool {
    with(|state| match state.sounds.get_mut(id) {
        Some(sound) => {
            sound.loaded = true;
            true
        }
        None => false,
    })
}

/// Unmarks it.
pub fn unload_sound(id: &str) -> bool {
    with(|state| match state.sounds.get_mut(id) {
        Some(sound) => {
            sound.loaded = false;
            true
        }
        None => false,
    })
}

/// How long a sound runs, which is the number the mod declared.
#[must_use]
pub fn get_duration(id: &str) -> Option<u64> {
    get_sound(id).map(|sound| sound.duration)
}

/// Creates a bus and returns its handle.
#[must_use]
pub fn create_bus(bus: Bus) -> BusHandle {
    with(|state| {
        let handle = state.next_bus;
        state.next_bus += 1;
        let mut bus = bus;
        bus.id = handle;
        state.buses.insert(handle, bus);
        BusHandle(handle)
    })
}

/// Registers a named bus, which fails when the name is taken.
pub fn register_bus(bus: Bus) -> bool {
    with(|state| {
        if state.buses.values().any(|held| held.name == bus.name) {
            return false;
        }
        let handle = state.next_bus;
        state.next_bus += 1;
        let mut bus = bus;
        bus.id = handle;
        state.buses.insert(handle, bus);
        true
    })
}

/// A bus, by handle.
#[must_use]
pub fn get_bus(handle: BusHandle) -> Option<Bus> {
    with(|state| state.buses.get(&handle.0).cloned())
}

/// Every bus a mod created.
#[must_use]
pub fn get_buses() -> Vec<Bus> {
    with(|state| state.buses.values().cloned().collect())
}

/// Creates a source and returns its handle.
#[must_use]
pub fn create_source(source: Source) -> SourceHandle {
    with(|state| {
        let handle = state.next_source;
        state.next_source += 1;
        let mut source = source;
        source.id = handle;
        state.sources.insert(handle, source);
        SourceHandle(handle)
    })
}

/// A source, by handle.
#[must_use]
pub fn get_source(handle: SourceHandle) -> Option<Source> {
    with(|state| state.sources.get(&handle.0).cloned())
}

/// Every source a mod created.
#[must_use]
pub fn get_sources() -> Vec<Source> {
    with(|state| state.sources.values().cloned().collect())
}

/// The volume set on the source itself, before the rest of the chain.
#[must_use]
pub fn get_volume(handle: SourceHandle) -> Option<f32> {
    get_source(handle).map(|source| source.volume)
}

/// Sets it, clamped to `[0, 1]`.
pub fn set_volume(handle: SourceHandle, volume: f32) -> bool {
    with(|state| match state.sources.get_mut(&handle.0) {
        Some(source) => {
            source.volume = sane(volume, 0.0, 1.0);
            true
        }
        None => false,
    })
}

/// The volume the whole chain leaves for a source: master × bus(es) × category × source.
#[must_use]
pub fn resolve_volume(handle: SourceHandle) -> Option<f32> {
    with(|state| state.sources.get(&handle.0).map(|source| resolve(state, source)))
}

/// The pitch set on the source.
#[must_use]
pub fn get_pitch(handle: SourceHandle) -> Option<f32> {
    get_source(handle).map(|source| source.pitch)
}

/// Sets it, clamped to a range a player would accept.
pub fn set_pitch(handle: SourceHandle, pitch: f32) -> bool {
    with(|state| match state.sources.get_mut(&handle.0) {
        Some(source) => {
            source.pitch = sane(pitch, PITCH_MIN, PITCH_MAX);
            true
        }
        None => false,
    })
}

/// Where the source sits.
#[must_use]
pub fn get_position(handle: SourceHandle) -> Option<[f32; 3]> {
    get_source(handle).map(|source| source.position)
}

/// Moves it.
pub fn set_position(handle: SourceHandle, position: [f32; 3]) -> bool {
    with(|state| match state.sources.get_mut(&handle.0) {
        Some(source) => {
            source.position = position;
            true
        }
        None => false,
    })
}

/// The near and far ends of the source's rolloff.
#[must_use]
pub fn get_distance(handle: SourceHandle) -> Option<(f32, f32)> {
    get_source(handle).map(|source| (source.min_distance, source.max_distance))
}

/// Sets them, keeping the far end at or beyond the near one.
pub fn set_distance(handle: SourceHandle, min_distance: f32, max_distance: f32) -> bool {
    with(|state| match state.sources.get_mut(&handle.0) {
        Some(source) => {
            source.min_distance = sane(min_distance, 0.0, f32::MAX);
            source.max_distance = sane(max_distance, source.min_distance, f32::MAX);
            true
        }
        None => false,
    })
}

/// The manual pan, used when spatialization is off.
#[must_use]
pub fn get_pan(handle: SourceHandle) -> Option<f32> {
    get_source(handle).map(|source| source.pan)
}

/// Sets it, clamped to hard left and hard right.
pub fn set_pan(handle: SourceHandle, pan: f32) -> bool {
    with(|state| match state.sources.get_mut(&handle.0) {
        Some(source) => {
            source.pan = sane(pan, -1.0, 1.0);
            true
        }
        None => false,
    })
}

/// The rolloff model.
#[must_use]
pub fn get_attenuation(handle: SourceHandle) -> Option<Attenuation> {
    get_source(handle).map(|source| source.attenuation)
}

/// Sets it.
pub fn set_attenuation(handle: SourceHandle, attenuation: Attenuation) -> bool {
    with(|state| match state.sources.get_mut(&handle.0) {
        Some(source) => {
            source.attenuation = attenuation;
            true
        }
        None => false,
    })
}

/// Whether the position drives the pan and the rolloff.
#[must_use]
pub fn get_spatialization(handle: SourceHandle) -> Option<bool> {
    get_source(handle).map(|source| source.spatialization)
}

/// Turns it on or off.
pub fn set_spatialization(handle: SourceHandle, on: bool) -> bool {
    with(|state| match state.sources.get_mut(&handle.0) {
        Some(source) => {
            source.spatialization = on;
            true
        }
        None => false,
    })
}

/// Whether the source loops.
#[must_use]
pub fn get_looping(handle: SourceHandle) -> Option<bool> {
    get_source(handle).map(|source| source.looping)
}

/// Sets it.
pub fn set_looping(handle: SourceHandle, on: bool) -> bool {
    with(|state| match state.sources.get_mut(&handle.0) {
        Some(source) => {
            source.looping = on;
            true
        }
        None => false,
    })
}

/// The requested reverb of a bus.
#[must_use]
pub fn get_reverb(handle: BusHandle) -> Option<f32> {
    get_bus(handle).map(|bus| bus.reverb)
}

/// Sets a bus's reverb, clamped to `[0, 1]`. Applying it is the client's processing.
pub fn set_reverb(handle: BusHandle, amount: f32) -> bool {
    with(|state| match state.buses.get_mut(&handle.0) {
        Some(bus) => {
            bus.reverb = sane(amount, 0.0, 1.0);
            true
        }
        None => false,
    })
}

/// The requested filter of a bus.
#[must_use]
pub fn get_filter(handle: BusHandle) -> Option<String> {
    get_bus(handle).map(|bus| bus.filter)
}

/// Sets a bus's filter by name. Applying it is the client's processing.
pub fn set_filter(handle: BusHandle, filter: &str) -> bool {
    with(|state| match state.buses.get_mut(&handle.0) {
        Some(bus) => {
            bus.filter = filter.to_owned();
            true
        }
        None => false,
    })
}

/// The listener's position and orientation.
#[must_use]
pub fn get_listener() -> Listener {
    with(|state| state.listener)
}

/// Moves the listener.
pub fn set_listener(listener: Listener) {
    with(|state| state.listener = listener);
}

/// The mod's mixer master volume, which is not the client's.
#[must_use]
pub fn get_master_volume() -> f32 {
    with(|state| state.master)
}

/// Sets it, clamped to `[0, 1]`, and answers what stuck.
pub fn set_master_volume(volume: f32) -> f32 {
    with(|state| {
        state.master = sane(volume, 0.0, 1.0);
        state.master
    })
}

/// A category's volume in the mod's mixer.
#[must_use]
pub fn get_category_volume(category: SoundCategory) -> f32 {
    with(|state| category_volume(state, category))
}

/// Sets it, clamped to `[0, 1]`, and answers what stuck.
pub fn set_category_volume(category: SoundCategory, volume: f32) -> f32 {
    with(|state| {
        let volume = sane(volume, 0.0, 1.0);
        state.categories.insert(category, volume);
        volume
    })
}

/// Plays a sound flat, at the listener, with no rolloff or pan.
pub fn play(sound: &str) -> Option<PlaybackHandle> {
    play_2d(sound)
}

/// The same, named for what it is.
pub fn play_2d(sound: &str) -> Option<PlaybackHandle> {
    with(|state| {
        let at = state.listener.position;
        start(state, sound, None, at, false)
    })
}

/// Plays a sound at a position in the world.
pub fn play_at(sound: &str, position: [f32; 3]) -> Option<PlaybackHandle> {
    play_3d(sound, position)
}

/// The same, computing the pan and the rolloff from the listener.
pub fn play_3d(sound: &str, position: [f32; 3]) -> Option<PlaybackHandle> {
    with(|state| start(state, sound, None, position, true))
}

/// Plays a sound through a source a mod created, with the whole chain and its settings.
pub fn play_source(handle: SourceHandle) -> Option<PlaybackHandle> {
    with(|state| {
        let source = state.sources.get(&handle.0)?.clone();
        start(
            state,
            &source.sound,
            Some(&source),
            source.position,
            source.spatialization,
        )
    })
}

/// Stops one playback.
pub fn stop(handle: PlaybackHandle) -> bool {
    with(|state| match state.playbacks.get_mut(&handle.0) {
        Some(playback) => {
            playback.state = PlaybackState::Stopped;
            true
        }
        None => false,
    })
}

/// Stops every playback that is still going, and answers how many there were.
pub fn stop_all() -> usize {
    with(|state| {
        let mut stopped = 0;
        for playback in state.playbacks.values_mut() {
            if playback.state != PlaybackState::Stopped {
                playback.state = PlaybackState::Stopped;
                stopped += 1;
            }
        }
        stopped
    })
}

/// Pauses a playing playback.
pub fn pause(handle: PlaybackHandle) -> bool {
    with(|state| match state.playbacks.get_mut(&handle.0) {
        Some(playback) if playback.state == PlaybackState::Playing => {
            playback.state = PlaybackState::Paused;
            true
        }
        _ => false,
    })
}

/// Starts a paused playback going again.
pub fn resume(handle: PlaybackHandle) -> bool {
    with(|state| match state.playbacks.get_mut(&handle.0) {
        Some(playback) if playback.state == PlaybackState::Paused => {
            playback.state = PlaybackState::Playing;
            true
        }
        _ => false,
    })
}

/// A playback's state.
#[must_use]
pub fn get_playback_state(handle: PlaybackHandle) -> Option<PlaybackState> {
    with(|state| state.playbacks.get(&handle.0).map(|playback| playback.state))
}

/// A playback, by handle.
#[must_use]
pub fn get_playback(handle: PlaybackHandle) -> Option<SoundPlayback> {
    with(|state| state.playbacks.get(&handle.0).cloned())
}

/// Every playback a mod started.
#[must_use]
pub fn get_playbacks() -> Vec<SoundPlayback> {
    with(|state| state.playbacks.values().cloned().collect())
}

/// Where a playback is in the sound, in milliseconds.
#[must_use]
pub fn get_playback_position(handle: PlaybackHandle) -> Option<u64> {
    get_playback(handle).map(|playback| playback.time)
}

/// Moves every playing playback on by a slice of time, following its pitch and looping.
/// A looping playback wraps around the sound; one that does not stops at the end.
pub fn advance(delta_millis: u64) -> usize {
    with(|state| {
        let mut updates: Vec<(u64, u64, bool)> = Vec::new();
        for (handle, playback) in &state.playbacks {
            if playback.state != PlaybackState::Playing {
                continue;
            }
            let duration = state
                .sounds
                .get(&playback.sound)
                .map(|sound| sound.duration)
                .unwrap_or(0);
            let step = (delta_millis as f32 * playback.pitch) as u64;
            let mut time = playback.time + step;
            let mut finished = false;
            if duration > 0 {
                if playback.looping {
                    time %= duration;
                } else if time >= duration {
                    time = duration;
                    finished = true;
                }
            }
            updates.push((*handle, time, finished));
        }
        for (handle, time, finished) in &updates {
            if let Some(playback) = state.playbacks.get_mut(handle) {
                playback.time = *time;
                if *finished {
                    playback.state = PlaybackState::Stopped;
                }
            }
        }
        updates.len()
    })
}

/// A one-line summary of the mod's own mixer, which is real; the client's is not.
#[must_use]
pub fn get_diagnostics() -> Vec<String> {
    with(|state| {
        let playing = state
            .playbacks
            .values()
            .filter(|playback| playback.state == PlaybackState::Playing)
            .count();
        vec![
            format!("master={:.3}", state.master),
            format!("sounds={}", state.sounds.len()),
            format!("sources={}", state.sources.len()),
            format!("buses={}", state.buses.len()),
            format!("playbacks={}", state.playbacks.len()),
            format!("playing={playing}"),
        ]
    })
}

/// Registers an effect with the client's mixer. `false`.
pub fn register_effect(_id: &str, _kind: &str) -> bool {
    refused("effects")
}

/// Installs an effect chain on the client's mixer. `false`.
pub fn set_effect_chain(_bus: BusHandle, _chain: &[&str]) -> bool {
    refused("effects")
}

/// The client's audio devices. Empty.
#[must_use]
pub fn get_audio_devices() -> Vec<String> {
    refused("device");
    Vec::new()
}

/// Selects the client's output device. `false`.
pub fn set_output_device(_name: &str) -> bool {
    refused("device")
}

/// The client's master volume. `None`.
#[must_use]
pub fn get_client_master_volume() -> Option<f32> {
    refused("client");
    None
}

/// Sets the client's master volume. `false`.
pub fn set_client_master_volume(_volume: f32) -> bool {
    refused("client")
}

/// The client's own sound list. Empty.
#[must_use]
pub fn get_client_sounds() -> Vec<String> {
    refused("client");
    Vec::new()
}

/// Decodes a real audio file. `false`: this host has no decoder.
pub fn load_sound_from_file(_id: &str, _path: &str) -> bool {
    refused("decode")
}

/// Why a question about the client's sound has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "device" => {
            "actually hearing a sound means writing samples to the client's audio device, which \
             this host has no handle on; the mixer here is a model a mod can query, not an output"
                .to_owned()
        }
        "client" => {
            "the client's own master volume and its sound list live inside its sound engine; the \
             volumes here are the mod's own mixer's, which is real but is not the client's"
                .to_owned()
        }
        "effects" => {
            "the client's own reverb and filter processing runs in its sound engine; set_reverb \
             and set_filter record what a mod's bus asks for, which this host cannot apply"
                .to_owned()
        }
        "decode" => {
            "this host has no audio decoder: a sound's duration is what the mod declares, and a \
             mod that ships an ogg or wav decodes it itself"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["device", "client", "effects", "decode"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about audio: {}", why_unknown(what));
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|state| *state = State::default());
    }

    #[test]
    fn the_volume_chain_multiplies_source_bus_category_and_master() {
        let _serial = serial();
        fresh();
        register_sound(Sound::new("hello:step", "sounds/step.ogg").duration(300));
        let bus = create_bus(Bus::new("music").volume(0.8).category(SoundCategory::Music));
        let source = create_source(Source::new("hello:step").bus(bus).volume(0.5));
        assert_eq!(set_master_volume(0.5), 0.5);
        assert_eq!(resolve_volume(source), Some(0.2), "0.5 * 0.8 * 1.0 * 0.5");
        assert_eq!(get_volume(source), Some(0.5));
        assert!(set_volume(source, 5.0));
        assert_eq!(get_volume(source), Some(1.0), "clamped to one");
        assert_eq!(set_master_volume(7.0), 1.0, "clamped");
        assert_eq!(set_category_volume(SoundCategory::Music, -1.0), 0.0);
        assert_eq!(get_category_volume(SoundCategory::Music), 0.0);
        assert_eq!(resolve_volume(source), Some(0.0), "the chain multiplies to silence");
        assert_eq!(resolve_volume(SourceHandle(999)), None);
    }

    #[test]
    fn a_sound_is_registered_loaded_and_unloaded() {
        let _serial = serial();
        fresh();
        assert!(register_sound(
            Sound::new("hello:bell", "sounds/bell.ogg")
                .duration(1200)
                .category(SoundCategory::Player)
                .looping(true)
        ));
        let sound = get_sound("hello:bell").expect("registered");
        assert_eq!(sound.duration, 1200);
        assert_eq!(sound.owner, crate::metadata::id());
        assert_eq!(sound.category, SoundCategory::Player);
        assert!(sound.looping);
        assert!(!sound.loaded);
        assert_eq!(get_duration("hello:bell"), Some(1200));
        assert_eq!(get_duration("nothing"), None);
        assert!(!register_sound(Sound::new("hello:bell", "sounds/bell.ogg")), "it replaces");
        assert!(load_sound("hello:bell"));
        assert!(get_sound("hello:bell").expect("still there").loaded);
        assert!(unload_sound("hello:bell"));
        assert!(!get_sound("hello:bell").expect("still there").loaded);
        assert!(sound.to_string().contains("1200 ms"), "{sound}");
        assert!(unregister_sound("hello:bell"));
        assert!(!unregister_sound("hello:bell"));
        assert_eq!(get_sounds().len(), 0);
        assert!(!load_sound("nothing"));
        assert!(!unload_sound("nothing"));
    }

    #[test]
    fn a_playback_moves_through_playing_paused_and_stopped() {
        let _serial = serial();
        fresh();
        register_sound(Sound::new("hello:tone", "sounds/tone.ogg").duration(1000));
        assert_eq!(play("hello:tone"), None, "an unloaded sound does not play");
        assert!(load_sound("hello:tone"));
        let handle = play("hello:tone").expect("it plays");
        assert_eq!(get_playback_state(handle), Some(PlaybackState::Playing));
        assert!(pause(handle));
        assert_eq!(get_playback_state(handle), Some(PlaybackState::Paused));
        assert!(!pause(handle), "it is already paused");
        assert!(resume(handle));
        assert_eq!(get_playback_state(handle), Some(PlaybackState::Playing));
        assert!(stop(handle));
        assert_eq!(get_playback_state(handle), Some(PlaybackState::Stopped));
        assert!(!resume(handle), "a stopped playback stays stopped");
        play("hello:tone").expect("again");
        assert_eq!(stop_all(), 1, "only the one still playing");
        assert!(!stop(PlaybackHandle(999)));
        assert_eq!(get_playback_state(PlaybackHandle(999)), None);
        assert_eq!(get_playbacks().len(), 2);
    }

    #[test]
    fn looping_wraps_the_position_and_a_plain_one_stops_at_the_end() {
        let _serial = serial();
        fresh();
        register_sound(Sound::new("hello:loop", "sounds/loop.ogg").duration(1000).looping(true));
        register_sound(Sound::new("hello:once", "sounds/once.ogg").duration(1000));
        assert!(load_sound("hello:loop"));
        assert!(load_sound("hello:once"));
        let looping = play("hello:loop").expect("plays");
        let plain = play("hello:once").expect("plays");
        assert_eq!(advance(400), 2);
        assert_eq!(get_playback_position(looping), Some(400));
        assert_eq!(get_playback_position(plain), Some(400));
        assert_eq!(advance(700), 2);
        assert_eq!(get_playback_position(looping), Some(100), "1000 wrapped");
        assert_eq!(get_playback_position(plain), Some(1000), "held at the end");
        assert_eq!(get_playback_state(plain), Some(PlaybackState::Stopped));
        assert_eq!(get_playback_state(looping), Some(PlaybackState::Playing));
        assert_eq!(advance(50), 1, "only the looping one still advances");
        assert_eq!(get_playback_position(looping), Some(150));
    }

    #[test]
    fn attenuation_falls_off_with_distance_and_is_clamped() {
        let linear = Source::new("hello:x")
            .distance(1.0, 9.0)
            .attenuation(Attenuation::Linear);
        assert_eq!(linear.attenuation_at(0.0), 1.0, "inside the near end");
        assert_eq!(linear.attenuation_at(1.0), 1.0, "at the minimum");
        assert_eq!(linear.attenuation_at(5.0), 0.5);
        assert_eq!(linear.attenuation_at(9.0), 0.0, "at the maximum");
        assert_eq!(linear.attenuation_at(100.0), 0.0, "beyond it");
        let inverse = Source::new("hello:y")
            .distance(1.0, 9.0)
            .attenuation(Attenuation::Inverse);
        assert_eq!(inverse.attenuation_at(1.0), 1.0);
        assert_eq!(inverse.attenuation_at(2.0), 0.5);
        let far = inverse.attenuation_at(100.0);
        assert!(far > 0.0 && far < 0.02, "inverse never quite reaches silence: {far}");
        let none = Source::new("hello:z").attenuation(Attenuation::None);
        assert_eq!(none.attenuation_at(1000.0), 1.0, "no rolloff");
        assert_eq!(Attenuation::parse("inverse"), Attenuation::Inverse);
        assert_eq!(Attenuation::parse("nonsense"), Attenuation::Linear);
        assert_eq!(Attenuation::Linear.to_string(), "linear");
    }

    #[test]
    fn pan_comes_from_the_listeners_right_vector() {
        let listener = Listener::default();
        assert_eq!(listener.right(), [1.0, 0.0, 0.0], "facing -z, up +y, right is +x");
        let right = Source::new("hello:r").position([2.0, 0.0, 0.0]);
        let left = Source::new("hello:l").position([-2.0, 0.0, 0.0]);
        let ahead = Source::new("hello:a").position([0.0, 0.0, -2.0]);
        assert_eq!(right.pan_towards(&listener), 1.0, "hard right");
        assert_eq!(left.pan_towards(&listener), -1.0, "hard left");
        assert_eq!(ahead.pan_towards(&listener), 0.0, "dead centre");
        assert_eq!(right.distance_to(&listener), 2.0);
        let turned = Listener::new([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]);
        assert_eq!(turned.right(), [-1.0, 0.0, 0.0], "turning around swaps the sides");
        assert_eq!(right.pan_towards(&turned), -1.0);
    }

    #[test]
    fn play_3d_computes_the_volume_and_pan_it_would_sound_at() {
        let _serial = serial();
        fresh();
        assert_eq!(set_master_volume(0.5), 0.5);
        assert_eq!(set_category_volume(SoundCategory::Hostile, 0.5), 0.5);
        register_sound(
            Sound::new("hello:growl", "sounds/growl.ogg")
                .duration(800)
                .category(SoundCategory::Hostile)
                .volume(0.4),
        );
        assert!(load_sound("hello:growl"));
        let bus = create_bus(
            Bus::new("enemies")
                .volume(0.8)
                .category(SoundCategory::Hostile),
        );
        let source = create_source(
            Source::new("hello:growl")
                .bus(bus)
                .volume(0.4)
                .position([2.0, 0.0, 0.0])
                .distance(1.0, 3.0)
                .spatialization(true),
        );
        let handle = play_source(source).expect("it plays");
        let playback = get_playback(handle).expect("there");
        assert_eq!(playback.pan, 1.0, "hard right");
        // master 0.5 * bus 0.8 * category 0.5 * source 0.4 = 0.08, gain at 2 of 1..3 = 0.5
        assert!((playback.volume - 0.04).abs() < 1e-6, "{}", playback.volume);
        assert!(playback.spatial);
        let at = play_at("hello:growl", [-2.0, 0.0, 0.0]).expect("plays");
        assert_eq!(get_playback(at).expect("there").pan, -1.0, "the other side");
    }

    #[test]
    fn the_audio_device_and_the_clients_mixer_are_refused() {
        let _serial = serial();
        fresh();
        assert!(get_audio_devices().is_empty());
        assert!(!set_output_device("speakers"));
        assert_eq!(get_client_master_volume(), None);
        assert!(!set_client_master_volume(0.5));
        assert!(get_client_sounds().is_empty());
        assert!(!register_effect("hello:echo", "reverb"));
        assert!(!set_effect_chain(BusHandle(1), &["hello:echo"]));
        assert!(!load_sound_from_file("hello:decoded", "sounds/real.ogg"));
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("device").contains("audio device"));
        assert!(why_unknown("decode").contains("decoder"));

        register_sound(Sound::new("hello:bell", "sounds/bell.ogg"));
        assert!(load_sound("hello:bell"));
        play("hello:bell");
        let diagnostics = get_diagnostics();
        assert!(diagnostics.iter().any(|line| line.starts_with("master=")), "{diagnostics:?}");
        assert!(diagnostics.iter().any(|line| line == "playbacks=1"), "{diagnostics:?}");
    }
}
