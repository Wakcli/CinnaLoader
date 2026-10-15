//! Particles and visual effects: the simulation a mod owns, and the drawing it cannot do.
//!
//! A particle here is real. A mod declares a kind — its sprite, size, colour and alpha fade,
//! gravity, initial velocity, blend mode, and how long it lives — registers it, and reads it
//! back. It then spawns particles of that kind into a pool this host integrates itself:
//! `position += velocity * dt`, `velocity += gravity * dt`, `age += dt`, size and alpha
//! interpolated over the particle's life, and removal once the age passes the maximum
//! ([`tick`] answers how many it removed). Spawning is seeded with a PRNG this host owns, so a
//! burst reproduces exactly, and [`spawn_in_box`] clamps every particle it makes to the box it
//! was given.
//!
//! Emitters are real too. One holds a rate in particles per second and a fractional
//! accumulator, a position, a direction and a spread, and an optional cap on how many it keeps
//! alive; [`Emitter::advance`] earns particles the accumulator has paid for and
//! [`advance_emitters`] turns those into particles in the pool. [`pause_emitter`],
//! [`resume_emitter`] and [`stop_emitter`] drive them, and a visual effect is a declaration
//! naming the emitters and kinds it is made of, its duration and whether it loops.
//!
//! What this host will not pretend to do is draw. The client owns its GPU, its particle pass
//! and its own particle list; binding a particle texture, applying blend or depth state, and
//! registering a particle renderer are refused with a sentence each in [`why_unknown`], and the
//! refusing functions answer `false` or empty rather than a success that never happened.
//!
//! ```
//! use cinnaloader::particles::{self, ParticleType};
//!
//! particles::register(ParticleType::new("hello:spark").gravity(-9.8).max_age(1.0));
//! let handle = particles::spawn_at("hello:spark", (0.0, 10.0, 0.0)).expect("the kind is registered");
//! particles::tick(0.5);
//! assert_eq!(particles::get_count(), 1);
//! assert!(!particles::set_texture(handle, "hello:skin"), "binding a texture is the client's");
//! particles::clear();
//! ```

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use crate::draw::BlendMode;

/// The corner points of a box a particle is kept inside.
pub type Bounds = ((f32, f32, f32), (f32, f32, f32));

/// splitmix64, so a seed reproduces a burst exactly across runs and machines.
#[derive(Debug, Clone, Copy)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / 16_777_216.0
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.unit()
    }

    /// A point no further than `radius` from the origin.
    fn in_sphere(&mut self, radius: f32) -> (f32, f32, f32) {
        let (x, y, z) = (
            self.range(-1.0, 1.0),
            self.range(-1.0, 1.0),
            self.range(-1.0, 1.0),
        );
        let len = (x * x + y * y + z * z).sqrt();
        let (x, y, z) = if len < 1e-6 {
            (0.0, 1.0, 0.0)
        } else {
            (x / len, y / len, z / len)
        };
        let r = radius * self.unit().cbrt();
        (x * r, y * r, z * r)
    }

    /// A point in the xz plane no further than `radius` from the origin.
    fn in_disc(&mut self, radius: f32) -> (f32, f32) {
        let angle = self.range(0.0, std::f32::consts::TAU);
        let r = radius * self.unit().sqrt();
        (r * angle.cos(), r * angle.sin())
    }
}

fn normalize(v: (f32, f32, f32)) -> (f32, f32, f32) {
    let len = (v.0 * v.0 + v.1 * v.1 + v.2 * v.2).sqrt();
    if len < 1e-6 {
        (0.0, 1.0, 0.0)
    } else {
        (v.0 / len, v.1 / len, v.2 / len)
    }
}

/// The declaration of a particle kind a mod ships.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticleType {
    pub id: String,
    pub owner: String,
    /// The sprite or texture it is drawn with, recorded as data.
    pub sprite: String,
    pub size: f32,
    /// Red, green, blue and the starting alpha; the alpha fades over the lifetime.
    pub color: [f32; 4],
    /// Downward acceleration, in units per second squared, on the y axis.
    pub gravity: f32,
    /// The velocity it is born with.
    pub velocity: (f32, f32, f32),
    pub blend: BlendMode,
    /// When it is removed, in seconds.
    pub max_age: f32,
    /// The span the size and alpha fade over, in seconds.
    pub lifetime: f32,
}

impl ParticleType {
    /// A kind with the defaults a caller would expect: one second, unit size, opaque white.
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            sprite: String::new(),
            size: 1.0,
            color: [1.0, 1.0, 1.0, 1.0],
            gravity: 0.0,
            velocity: (0.0, 0.0, 0.0),
            blend: BlendMode::Alpha,
            max_age: 1.0,
            lifetime: 1.0,
        }
    }

    #[must_use]
    pub fn sprite(mut self, sprite: &str) -> Self {
        self.sprite = sprite.to_owned();
        self
    }

    #[must_use]
    pub fn size(mut self, size: f32) -> Self {
        self.size = size.max(0.0);
        self
    }

    #[must_use]
    pub fn color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    #[must_use]
    pub fn gravity(mut self, gravity: f32) -> Self {
        self.gravity = gravity;
        self
    }

    #[must_use]
    pub fn velocity(mut self, velocity: (f32, f32, f32)) -> Self {
        self.velocity = velocity;
        self
    }

    #[must_use]
    pub fn blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    #[must_use]
    pub fn max_age(mut self, max_age: f32) -> Self {
        self.max_age = max_age.max(0.0);
        self
    }

    #[must_use]
    pub fn lifetime(mut self, lifetime: f32) -> Self {
        self.lifetime = lifetime.max(0.0);
        self
    }
}

/// One particle this host is simulating.
#[derive(Debug, Clone, PartialEq)]
pub struct Particle {
    pub handle: u64,
    /// The [`ParticleType`] it was made from.
    pub kind: String,
    pub position: (f32, f32, f32),
    pub velocity: (f32, f32, f32),
    pub color: [f32; 4],
    pub size: f32,
    /// How long it has lived, in seconds.
    pub age: f32,
    /// When it is removed.
    pub max_age: f32,
    /// The span the size and alpha fade over.
    pub lifetime: f32,
    pub gravity: f32,
    pub collision: bool,
    /// When set, the particle is clamped inside it every tick.
    pub bounds: Option<Bounds>,
    pub sprite: String,
    pub blend: BlendMode,
    /// The emitter that made it, when one did.
    pub emitter: Option<u64>,
}

impl Particle {
    fn from_type(kind: &ParticleType, position: (f32, f32, f32)) -> Self {
        Self {
            handle: 0,
            kind: kind.id.clone(),
            position,
            velocity: kind.velocity,
            color: kind.color,
            size: kind.size,
            age: 0.0,
            max_age: kind.max_age,
            lifetime: kind.lifetime,
            gravity: kind.gravity,
            collision: false,
            bounds: None,
            sprite: kind.sprite.clone(),
            blend: kind.blend,
            emitter: None,
        }
    }

    /// How far through its life it is, from zero to one, over the fade span.
    #[must_use]
    pub fn life_fraction(&self) -> f32 {
        self.fraction_at(self.age)
    }

    /// The same for a given age.
    #[must_use]
    pub fn fraction_at(&self, age: f32) -> f32 {
        if self.lifetime <= 0.0 {
            return 1.0;
        }
        (age / self.lifetime).clamp(0.0, 1.0)
    }

    /// The alpha now, faded from the colour's own alpha to nothing over the lifetime.
    #[must_use]
    pub fn alpha(&self) -> f32 {
        self.alpha_at(self.age)
    }

    /// The alpha at a given age.
    #[must_use]
    pub fn alpha_at(&self, age: f32) -> f32 {
        self.color[3] * (1.0 - self.fraction_at(age))
    }

    /// The size now, faded the same way.
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.size * (1.0 - self.life_fraction())
    }

    /// Moves it on by `dt` seconds and ages it.
    pub fn integrate(&mut self, dt: f32) {
        self.position.0 += self.velocity.0 * dt;
        self.position.1 += self.velocity.1 * dt;
        self.position.2 += self.velocity.2 * dt;
        self.velocity.1 += self.gravity * dt;
        self.age += dt;
        if let Some((min, max)) = self.bounds {
            self.position.0 = self.position.0.clamp(min.0, max.0);
            self.position.1 = self.position.1.clamp(min.1, max.1);
            self.position.2 = self.position.2.clamp(min.2, max.2);
            if self.collision {
                if self.position.0 == min.0 || self.position.0 == max.0 {
                    self.velocity.0 = 0.0;
                }
                if self.position.1 == min.1 || self.position.1 == max.1 {
                    self.velocity.1 = 0.0;
                }
                if self.position.2 == min.2 || self.position.2 == max.2 {
                    self.velocity.2 = 0.0;
                }
            }
        }
    }

    /// Whether it has outlived its maximum age.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.age >= self.max_age
    }
}

/// Where an emitter puts the particles it makes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmissionShape {
    Point,
    Sphere { radius: f32 },
    Box { size: (f32, f32, f32) },
    Disc { radius: f32 },
}

/// An emitter: a rate, a place, a direction, and how many it may keep alive.
#[derive(Debug, Clone)]
pub struct Emitter {
    /// Its name in the template registry, empty for an unregistered one.
    pub id: String,
    /// The [`ParticleType`] it makes.
    pub kind: String,
    /// Particles per second.
    pub rate: f32,
    pub position: (f32, f32, f32),
    pub direction: (f32, f32, f32),
    /// How wide the direction fans out, in degrees.
    pub spread: f32,
    /// How fast what it makes leaves.
    pub speed: f32,
    pub shape: EmissionShape,
    /// The most it keeps alive at once, when a caller sets one.
    pub max_alive: Option<usize>,
    pub running: bool,
    pub paused: bool,
    pub seed: u64,
    /// The handle of the live copy, zero in the template registry.
    pub handle: u64,
    accumulator: f32,
    spawned: u64,
    rng: Rng,
}

impl Emitter {
    /// An emitter of `kind`, registered under `id`, making one particle a second upward.
    #[must_use]
    pub fn new(id: &str, kind: &str) -> Self {
        Self {
            id: id.to_owned(),
            kind: kind.to_owned(),
            rate: 1.0,
            position: (0.0, 0.0, 0.0),
            direction: (0.0, 1.0, 0.0),
            spread: 0.0,
            speed: 1.0,
            shape: EmissionShape::Point,
            max_alive: None,
            running: true,
            paused: false,
            seed: 0,
            handle: 0,
            accumulator: 0.0,
            spawned: 0,
            rng: Rng::new(0),
        }
    }

    #[must_use]
    pub fn rate(mut self, rate: f32) -> Self {
        self.rate = rate.max(0.0);
        self
    }

    #[must_use]
    pub fn at(mut self, position: (f32, f32, f32)) -> Self {
        self.position = position;
        self
    }

    #[must_use]
    pub fn direction(mut self, direction: (f32, f32, f32)) -> Self {
        self.direction = direction;
        self
    }

    #[must_use]
    pub fn spread(mut self, degrees: f32) -> Self {
        self.spread = degrees.max(0.0);
        self
    }

    #[must_use]
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed.max(0.0);
        self
    }

    #[must_use]
    pub fn shape(mut self, shape: EmissionShape) -> Self {
        self.shape = shape;
        self
    }

    #[must_use]
    pub fn max_alive(mut self, max_alive: usize) -> Self {
        self.max_alive = Some(max_alive);
        self
    }

    #[must_use]
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self.rng = Rng::new(seed);
        self
    }

    fn accumulate(&mut self, dt: f32) -> usize {
        self.accumulator += self.rate.max(0.0) * dt.max(0.0);
        let earned = self.accumulator.floor().max(0.0) as usize;
        self.accumulator -= earned as f32;
        earned
    }

    /// Advances it by `dt` seconds and answers how many particles the accumulator earned.
    pub fn advance(&mut self, dt: f32) -> usize {
        if !self.running || self.paused {
            return 0;
        }
        self.accumulate(dt)
    }

    /// How many it has made.
    #[must_use]
    pub fn spawned(&self) -> u64 {
        self.spawned
    }

    fn sample_direction(&mut self) -> (f32, f32, f32) {
        let spread = self.spread.to_radians();
        let (rx, ry, rz) = (
            self.rng.range(-1.0, 1.0),
            self.rng.range(-1.0, 1.0),
            self.rng.range(-1.0, 1.0),
        );
        normalize((
            self.direction.0 + rx * spread,
            self.direction.1 + ry * spread,
            self.direction.2 + rz * spread,
        ))
    }

    fn shape_offset(&mut self) -> (f32, f32, f32) {
        match self.shape {
            EmissionShape::Point => (0.0, 0.0, 0.0),
            EmissionShape::Sphere { radius } => self.rng.in_sphere(radius),
            EmissionShape::Box { size } => (
                self.rng.range(-size.0 / 2.0, size.0 / 2.0),
                self.rng.range(-size.1 / 2.0, size.1 / 2.0),
                self.rng.range(-size.2 / 2.0, size.2 / 2.0),
            ),
            EmissionShape::Disc { radius } => {
                let (x, z) = self.rng.in_disc(radius);
                (x, 0.0, z)
            }
        }
    }

    fn make_particle(&mut self, kind: &ParticleType) -> Particle {
        let (ox, oy, oz) = self.shape_offset();
        let dir = self.sample_direction();
        let mut particle = Particle::from_type(
            kind,
            (
                self.position.0 + ox,
                self.position.1 + oy,
                self.position.2 + oz,
            ),
        );
        particle.velocity = (
            kind.velocity.0 + dir.0 * self.speed,
            kind.velocity.1 + dir.1 * self.speed,
            kind.velocity.2 + dir.2 * self.speed,
        );
        particle.emitter = Some(self.handle);
        particle
    }
}

/// A visual effect: the emitters and kinds it is made of, how long it runs and whether it loops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualEffect {
    pub id: String,
    pub owner: String,
    /// The registered emitter ids it plays.
    pub emitters: Vec<String>,
    /// The particle kinds it uses.
    pub kinds: Vec<String>,
    /// How long it runs, in whole seconds.
    pub duration: u64,
    pub looping: bool,
}

impl VisualEffect {
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            emitters: Vec::new(),
            kinds: Vec::new(),
            duration: 0,
            looping: false,
        }
    }

    #[must_use]
    pub fn emitter(mut self, emitter: &str) -> Self {
        if !self.emitters.iter().any(|held| held == emitter) {
            self.emitters.push(emitter.to_owned());
        }
        self
    }

    #[must_use]
    pub fn kind(mut self, kind: &str) -> Self {
        if !self.kinds.iter().any(|held| held == kind) {
            self.kinds.push(kind.to_owned());
        }
        self
    }

    #[must_use]
    pub fn duration(mut self, seconds: u64) -> Self {
        self.duration = seconds;
        self
    }

    #[must_use]
    pub fn looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }
}

#[derive(Debug)]
struct Pool {
    next: u64,
    particles: Vec<Particle>,
}

impl Pool {
    fn push(&mut self, mut particle: Particle) -> u64 {
        self.next += 1;
        particle.handle = self.next;
        let handle = particle.handle;
        self.particles.push(particle);
        handle
    }
}

#[derive(Debug)]
struct Live {
    next: u64,
    emitters: BTreeMap<u64, Emitter>,
}

fn types() -> &'static Mutex<BTreeMap<String, ParticleType>> {
    static TYPES: OnceLock<Mutex<BTreeMap<String, ParticleType>>> = OnceLock::new();
    TYPES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_types<T>(body: impl FnOnce(&mut BTreeMap<String, ParticleType>) -> T) -> T {
    let mut guard = types().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn effects() -> &'static Mutex<BTreeMap<String, VisualEffect>> {
    static EFFECTS: OnceLock<Mutex<BTreeMap<String, VisualEffect>>> = OnceLock::new();
    EFFECTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_effects<T>(body: impl FnOnce(&mut BTreeMap<String, VisualEffect>) -> T) -> T {
    let mut guard = effects().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn templates() -> &'static Mutex<BTreeMap<String, Emitter>> {
    static TEMPLATES: OnceLock<Mutex<BTreeMap<String, Emitter>>> = OnceLock::new();
    TEMPLATES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_templates<T>(body: impl FnOnce(&mut BTreeMap<String, Emitter>) -> T) -> T {
    let mut guard = templates().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn pool() -> &'static Mutex<Pool> {
    static POOL: OnceLock<Mutex<Pool>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(Pool { next: 0, particles: Vec::new() }))
}

fn with_pool<T>(body: impl FnOnce(&mut Pool) -> T) -> T {
    let mut guard = pool().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn live() -> &'static Mutex<Live> {
    static LIVE: OnceLock<Mutex<Live>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(Live { next: 0, emitters: BTreeMap::new() }))
}

fn with_live<T>(body: impl FnOnce(&mut Live) -> T) -> T {
    let mut guard = live().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn type_for(id: &str) -> Option<ParticleType> {
    with_types(|types| types.get(id).cloned())
}

/// Registers a particle kind, or replaces the one with that id.
pub fn register(kind: ParticleType) -> bool {
    with_types(|types| types.insert(kind.id.clone(), kind).is_none())
}

/// Forgets a particle kind.
pub fn unregister(id: &str) -> bool {
    with_types(|types| types.remove(id).is_some())
}

/// Starts a particle kind, for a mod that wants to build one up.
#[must_use]
pub fn create(id: &str) -> ParticleType {
    ParticleType::new(id)
}

/// A particle kind by id.
#[must_use]
pub fn get_type(id: &str) -> Option<ParticleType> {
    type_for(id)
}

/// Every particle kind a mod registered.
#[must_use]
pub fn get_types() -> Vec<ParticleType> {
    with_types(|types| types.values().cloned().collect())
}

/// Spawns one particle of `kind` at the origin.
pub fn spawn(kind: &str) -> Option<u64> {
    spawn_at(kind, (0.0, 0.0, 0.0))
}

/// Spawns one particle of `kind` at a position.
pub fn spawn_at(kind: &str, position: (f32, f32, f32)) -> Option<u64> {
    let kind = type_for(kind)?;
    Some(with_pool(|pool| pool.push(Particle::from_type(&kind, position))))
}

/// Spawns `count` particles of `kind` inside a box, each clamped to it every tick.
pub fn spawn_in_box(
    kind: &str,
    min: (f32, f32, f32),
    max: (f32, f32, f32),
    count: usize,
    seed: u64,
) -> Vec<u64> {
    let Some(kind) = type_for(kind) else {
        return Vec::new();
    };
    let mut rng = Rng::new(seed);
    let mut handles = Vec::with_capacity(count);
    with_pool(|pool| {
        for _ in 0..count {
            let position = (
                rng.range(min.0, max.0).clamp(min.0, max.0),
                rng.range(min.1, max.1).clamp(min.1, max.1),
                rng.range(min.2, max.2).clamp(min.2, max.2),
            );
            let mut particle = Particle::from_type(&kind, position);
            particle.bounds = Some((min, max));
            handles.push(pool.push(particle));
        }
    });
    handles
}

/// Spawns `count` particles of `kind` evenly spaced from `from` to `to`.
pub fn spawn_along_line(
    kind: &str,
    from: (f32, f32, f32),
    to: (f32, f32, f32),
    count: usize,
) -> Vec<u64> {
    let Some(kind) = type_for(kind) else {
        return Vec::new();
    };
    if count == 0 {
        return Vec::new();
    }
    let last = count.saturating_sub(1);
    with_pool(|pool| {
        (0..count)
            .map(|index| {
                let t = if last == 0 {
                    0.0
                } else {
                    index as f32 / last as f32
                };
                let position = (
                    from.0 + (to.0 - from.0) * t,
                    from.1 + (to.1 - from.1) * t,
                    from.2 + (to.2 - from.2) * t,
                );
                pool.push(Particle::from_type(&kind, position))
            })
            .collect()
    })
}

/// Spawns `count` particles of `kind` inside a sphere, no further out than `radius`.
pub fn spawn_in_sphere(
    kind: &str,
    center: (f32, f32, f32),
    radius: f32,
    count: usize,
    seed: u64,
) -> Vec<u64> {
    let Some(kind) = type_for(kind) else {
        return Vec::new();
    };
    let mut rng = Rng::new(seed);
    let mut handles = Vec::with_capacity(count);
    with_pool(|pool| {
        for _ in 0..count {
            let (x, y, z) = rng.in_sphere(radius.max(0.0));
            handles.push(pool.push(Particle::from_type(
                &kind,
                (center.0 + x, center.1 + y, center.2 + z),
            )));
        }
    });
    handles
}

/// Spawns `count` particles of `kind` at a position, each thrown outward at `speed`.
pub fn spawn_burst(
    kind: &str,
    position: (f32, f32, f32),
    count: usize,
    speed: f32,
    seed: u64,
) -> Vec<u64> {
    let Some(kind) = type_for(kind) else {
        return Vec::new();
    };
    let mut rng = Rng::new(seed);
    let mut handles = Vec::with_capacity(count);
    with_pool(|pool| {
        for _ in 0..count {
            let dir = normalize((
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
                rng.range(-1.0, 1.0),
            ));
            let mut particle = Particle::from_type(&kind, position);
            particle.velocity = (
                kind.velocity.0 + dir.0 * speed,
                kind.velocity.1 + dir.1 * speed,
                kind.velocity.2 + dir.2 * speed,
            );
            handles.push(pool.push(particle));
        }
    });
    handles
}

/// One particle by handle.
#[must_use]
pub fn get(handle: u64) -> Option<Particle> {
    with_pool(|pool| pool.particles.iter().find(|p| p.handle == handle).cloned())
}

/// Every particle this host is simulating.
#[must_use]
pub fn get_active() -> Vec<Particle> {
    with_pool(|pool| pool.particles.clone())
}

/// Removes one particle.
pub fn remove(handle: u64) -> bool {
    with_pool(|pool| {
        let before = pool.particles.len();
        pool.particles.retain(|particle| particle.handle != handle);
        pool.particles.len() != before
    })
}

/// Removes every particle, and answers how many there were.
pub fn clear() -> usize {
    with_pool(|pool| {
        let count = pool.particles.len();
        pool.particles.clear();
        count
    })
}

/// How many particles are alive.
#[must_use]
pub fn get_count() -> usize {
    with_pool(|pool| pool.particles.len())
}

/// Moves every particle on by `dt` seconds, removes the aged-out ones, and answers how many it
/// removed.
pub fn tick(dt: f32) -> usize {
    with_pool(|pool| {
        let mut removed = 0;
        pool.particles.retain_mut(|particle| {
            particle.integrate(dt);
            let alive = !particle.is_dead();
            if !alive {
                removed += 1;
            }
            alive
        });
        removed
    })
}

fn edit(handle: u64, body: impl FnOnce(&mut Particle)) -> bool {
    with_pool(|pool| match pool.particles.iter_mut().find(|p| p.handle == handle) {
        Some(particle) => {
            body(particle);
            true
        }
        None => false,
    })
}

/// Moves one particle.
pub fn set_position(handle: u64, position: (f32, f32, f32)) -> bool {
    edit(handle, |particle| particle.position = position)
}

/// Changes its velocity.
pub fn set_velocity(handle: u64, velocity: (f32, f32, f32)) -> bool {
    edit(handle, |particle| particle.velocity = velocity)
}

/// Changes its colour, including the alpha the fade starts from.
pub fn set_color(handle: u64, color: [f32; 4]) -> bool {
    edit(handle, |particle| particle.color = color)
}

/// Changes how big it is.
pub fn set_scale(handle: u64, scale: f32) -> bool {
    edit(handle, |particle| particle.size = scale.max(0.0))
}

/// Changes the span its size and alpha fade over.
pub fn set_lifetime(handle: u64, lifetime: f32) -> bool {
    edit(handle, |particle| particle.lifetime = lifetime.max(0.0))
}

/// Changes the gravity on it.
pub fn set_gravity(handle: u64, gravity: f32) -> bool {
    edit(handle, |particle| particle.gravity = gravity)
}

/// Whether it collides with its bounding box, which stops it rather than sliding.
pub fn set_collision(handle: u64, collision: bool) -> bool {
    edit(handle, |particle| particle.collision = collision)
}

/// Registers an emitter template under its own id.
pub fn register_emitter(emitter: Emitter) -> bool {
    if emitter.id.is_empty() {
        return false;
    }
    with_templates(|templates| templates.insert(emitter.id.clone(), emitter).is_none())
}

/// Forgets an emitter template.
pub fn unregister_emitter(id: &str) -> bool {
    with_templates(|templates| templates.remove(id).is_some())
}

/// Starts a live emitter from a registered template, and answers its handle.
pub fn create_emitter(id: &str) -> Option<u64> {
    let mut template = with_templates(|templates| templates.get(id).cloned())?;
    Some(with_live(|live| {
        live.next += 1;
        let handle = live.next;
        template.handle = handle;
        template.accumulator = 0.0;
        template.spawned = 0;
        template.running = true;
        template.paused = false;
        template.rng = Rng::new(template.seed);
        live.emitters.insert(handle, template);
        handle
    }))
}

/// Stops a live emitter and forgets it.
pub fn stop_emitter(handle: u64) -> bool {
    with_live(|live| live.emitters.remove(&handle).is_some())
}

/// Stops a live emitter earning particles until it is resumed.
pub fn pause_emitter(handle: u64) -> bool {
    with_live(|live| match live.emitters.get_mut(&handle) {
        Some(emitter) => {
            emitter.paused = true;
            true
        }
        None => false,
    })
}

/// Lets a paused emitter earn again.
pub fn resume_emitter(handle: u64) -> bool {
    with_live(|live| match live.emitters.get_mut(&handle) {
        Some(emitter) => {
            emitter.paused = false;
            true
        }
        None => false,
    })
}

/// Changes how many particles a second a live emitter makes.
pub fn set_emission_rate(handle: u64, rate: f32) -> bool {
    with_live(|live| match live.emitters.get_mut(&handle) {
        Some(emitter) => {
            emitter.rate = rate.max(0.0);
            true
        }
        None => false,
    })
}

/// Changes where a live emitter puts what it makes.
pub fn set_emission_shape(handle: u64, shape: EmissionShape) -> bool {
    with_live(|live| match live.emitters.get_mut(&handle) {
        Some(emitter) => {
            emitter.shape = shape;
            true
        }
        None => false,
    })
}

/// A live emitter by handle.
#[must_use]
pub fn get_emitter(handle: u64) -> Option<Emitter> {
    with_live(|live| live.emitters.get(&handle).cloned())
}

/// Every live emitter, in the order they started.
#[must_use]
pub fn get_emitters() -> Vec<Emitter> {
    with_live(|live| live.emitters.values().cloned().collect())
}

/// Advances every live emitter by `dt` seconds, spawning what each accumulator earned, and
/// answers how many particles that made.
pub fn advance_emitters(dt: f32) -> usize {
    if dt <= 0.0 {
        return 0;
    }
    let mut live = live().lock().unwrap_or_else(|error| error.into_inner());
    let mut pool = pool().lock().unwrap_or_else(|error| error.into_inner());
    let mut total = 0;
    for emitter in live.emitters.values_mut() {
        let earned = emitter.advance(dt);
        if earned == 0 {
            continue;
        }
        let Some(kind) = type_for(&emitter.kind) else {
            continue;
        };
        let alive = pool
            .particles
            .iter()
            .filter(|particle| particle.emitter == Some(emitter.handle))
            .count();
        let make = match emitter.max_alive {
            Some(max) => earned.min(max.saturating_sub(alive)),
            None => earned,
        };
        for _ in 0..make {
            let particle = emitter.make_particle(&kind);
            pool.push(particle);
            total += 1;
        }
        emitter.spawned += make as u64;
    }
    total
}

/// Registers a visual effect, or replaces the one with that id.
pub fn register_effect(effect: VisualEffect) -> bool {
    with_effects(|effects| effects.insert(effect.id.clone(), effect).is_none())
}

/// Forgets a visual effect.
pub fn unregister_effect(id: &str) -> bool {
    with_effects(|effects| effects.remove(id).is_some())
}

/// A visual effect by id.
#[must_use]
pub fn get_effect(id: &str) -> Option<VisualEffect> {
    with_effects(|effects| effects.get(id).cloned())
}

/// Every visual effect a mod registered.
#[must_use]
pub fn get_effects() -> Vec<VisualEffect> {
    with_effects(|effects| effects.values().cloned().collect())
}

fn refuse(what: &str) {
    crate::warn!("a mod asked this host for particles: {}", why_unknown(what));
}

/// Binding a particle texture is a GPU bind on the client's device. `false`.
pub fn set_texture(_handle: u64, _texture: &str) -> bool {
    refuse("texture");
    false
}

/// An animation over a particle is played by the client's renderer. `false`.
pub fn set_animation(_handle: u64, _animation: &str) -> bool {
    refuse("draw");
    false
}

/// Blend and depth state are applied by the client's renderer. `false`.
pub fn set_blend_mode(_handle: u64, _blend: BlendMode) -> bool {
    refuse("blend");
    false
}

/// A particle renderer would be called by the client as it draws. `false`.
pub fn register_particle_renderer(_name: &str) -> bool {
    refuse("draw");
    false
}

/// The client's own particle list, which lives in its world. Empty.
#[must_use]
pub fn client_particles() -> Vec<Particle> {
    refuse("list");
    Vec::new()
}

/// Spawning into the client's world means its particle engine. `None`.
pub fn spawn_into_world(_kind: &str, _position: (f32, f32, f32)) -> Option<u64> {
    refuse("world");
    None
}

/// Why a particle question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "draw" => {
            "a particle renderer, or an animation over a particle, is called by the client as \
             it draws each frame; this host runs the simulation and never draws"
                .to_owned()
        }
        "list" => {
            "the client keeps its own particle list inside its own world; this host can only \
             list the particles a mod spawned into its own pool"
                .to_owned()
        }
        "world" => {
            "spawning into the client's world means reaching its particle engine, which a mod \
             host cannot do"
                .to_owned()
        }
        "texture" => {
            "binding a particle texture is a GPU bind on the client's render device, which this \
             host has no handle on"
                .to_owned()
        }
        "blend" => {
            "blend and depth state are set by the client's renderer around its own draw call, \
             not by a mod"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["draw", "list", "world", "texture", "blend"]
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

    fn fresh() {
        with_types(|types| types.clear());
        with_effects(|effects| effects.clear());
        with_templates(|templates| templates.clear());
        with_pool(|pool| {
            pool.particles.clear();
            pool.next = 0;
        });
        with_live(|live| {
            live.emitters.clear();
            live.next = 0;
        });
    }

    #[test]
    fn one_tick_moves_a_particle_by_its_velocity() {
        let _serial = serial();
        fresh();
        assert!(register(ParticleType::new("hello:spark")));
        assert!(!register(ParticleType::new("hello:spark")), "it replaces");
        let handle = spawn_at("hello:spark", (0.0, 0.0, 0.0)).expect("the kind is registered");
        assert!(set_velocity(handle, (1.0, 2.0, 3.0)));
        assert_eq!(tick(0.5), 0);
        let particle = get(handle).expect("still alive");
        assert_eq!(particle.position, (0.5, 1.0, 1.5));
        assert_eq!(get_count(), 1);
        assert_eq!(get_active().len(), 1);
        assert!(set_position(handle, (9.0, 9.0, 9.0)));
        assert!(set_color(handle, [1.0, 0.0, 0.0, 1.0]));
        assert!(set_scale(handle, 4.0));
        assert!(set_gravity(handle, -1.0));
        assert!(set_collision(handle, true));
        assert!(set_lifetime(handle, 3.0));
        let particle = get(handle).expect("still alive");
        assert_eq!(particle.position, (9.0, 9.0, 9.0));
        assert_eq!(particle.color, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(particle.size, 4.0);
        assert_eq!(particle.gravity, -1.0);
        assert!(particle.collision);
        assert_eq!(particle.lifetime, 3.0);
        assert!(remove(handle));
        assert!(!remove(handle));
        assert_eq!(get(handle), None);
        assert!(!set_position(handle, (0.0, 0.0, 0.0)), "a missing particle says so");
    }

    #[test]
    fn gravity_pulls_a_particle_down_over_several_ticks() {
        let _serial = serial();
        fresh();
        register(ParticleType::new("hello:spark").gravity(-10.0).max_age(100.0));
        let handle = spawn("hello:spark").expect("registered");
        for _ in 0..5 {
            tick(0.1);
        }
        let particle = get(handle).expect("still alive");
        assert!((particle.position.1 + 1.0).abs() < 1e-5, "{:?}", particle.position);
        assert!((particle.velocity.1 + 5.0).abs() < 1e-5, "{:?}", particle.velocity);
    }

    #[test]
    fn a_particle_dies_exactly_at_its_max_age() {
        let _serial = serial();
        fresh();
        register(
            ParticleType::new("hello:spark")
                .max_age(1.0)
                .lifetime(1.0),
        );
        let handle = spawn("hello:spark").expect("registered");
        for _ in 0..3 {
            assert_eq!(tick(0.25), 0);
            assert!(get(handle).is_some());
        }
        assert_eq!(tick(0.25), 1, "it dies on the tick that reaches its max age");
        assert_eq!(get(handle), None);
        assert_eq!(get_count(), 0);
    }

    #[test]
    fn spawn_in_box_keeps_every_particle_inside_over_a_burst() {
        let _serial = serial();
        fresh();
        register(
            ParticleType::new("hello:spark")
                .gravity(-10.0)
                .velocity((2.0, 3.0, -4.0))
                .max_age(100.0),
        );
        let min = (0.0, 0.0, 0.0);
        let max = (2.0, 2.0, 2.0);
        let handles = spawn_in_box("hello:spark", min, max, 50, 7);
        assert_eq!(handles.len(), 50);
        for _ in 0..40 {
            tick(0.05);
            for handle in &handles {
                let particle = get(*handle).expect("kept alive");
                let p = particle.position;
                assert!(
                    p.0 >= min.0 && p.0 <= max.0 && p.1 >= min.1 && p.1 <= max.1 && p.2 >= min.2 && p.2 <= max.2,
                    "stayed in the box: {p:?}"
                );
            }
        }
        // the same seed reproduces the same burst
        let again: Vec<(f32, f32, f32)> = spawn_burst("hello:spark", (0.0, 0.0, 0.0), 5, 2.0, 42)
            .iter()
            .map(|handle| get(*handle).expect("there").velocity)
            .collect();
        clear();
        let repeat: Vec<(f32, f32, f32)> = spawn_burst("hello:spark", (0.0, 0.0, 0.0), 5, 2.0, 42)
            .iter()
            .map(|handle| get(*handle).expect("there").velocity)
            .collect();
        assert_eq!(again, repeat, "the seed reproduces the burst");
    }

    #[test]
    fn an_emitter_with_a_2_5_rate_earns_two_then_three() {
        let _serial = serial();
        fresh();
        register(ParticleType::new("hello:spark").max_age(100.0));
        let mut emitter = Emitter::new("hello:smoke", "hello:spark").rate(2.5);
        assert_eq!(emitter.advance(1.0), 2);
        assert_eq!(emitter.advance(1.0), 3);

        assert!(register_emitter(Emitter::new("hello:smoke", "hello:spark").rate(2.5).seed(1)));
        let handle = create_emitter("hello:smoke").expect("the template is registered");
        assert_eq!(advance_emitters(1.0), 2);
        assert_eq!(get_count(), 2);
        assert_eq!(advance_emitters(1.0), 3);
        assert_eq!(get_count(), 5);
        assert_eq!(get_emitter(handle).map(|held| held.spawned()), Some(5));
        assert!(set_emission_rate(handle, 0.0));
        assert_eq!(advance_emitters(1.0), 0);
        assert!(set_emission_shape(handle, EmissionShape::Sphere { radius: 1.0 }));
    }

    #[test]
    fn the_alpha_fades_over_the_particles_life() {
        let _serial = serial();
        fresh();
        register(
            ParticleType::new("hello:spark")
                .color([1.0, 1.0, 1.0, 1.0])
                .lifetime(2.0)
                .max_age(4.0),
        );
        let handle = spawn("hello:spark").expect("registered");
        assert_eq!(get(handle).expect("alive").alpha(), 1.0);
        assert_eq!(get(handle).expect("alive").scale(), 1.0);
        tick(1.0);
        assert_eq!(get(handle).expect("alive").alpha(), 0.5);
        assert_eq!(get(handle).expect("alive").scale(), 0.5);
        tick(1.0);
        assert_eq!(get(handle).expect("alive").alpha(), 0.0);
        assert_eq!(get(handle).expect("alive").scale(), 0.0);
        let particle = get(handle).expect("still alive, its max age is four");
        assert_eq!(particle.alpha_at(0.0), 1.0);
        assert_eq!(particle.alpha_at(1.0), 0.5);
        assert_eq!(particle.alpha_at(2.0), 0.0);
    }

    #[test]
    fn emitters_start_pause_resume_and_stop() {
        let _serial = serial();
        fresh();
        register(ParticleType::new("hello:spark").max_age(100.0));
        assert!(!register_emitter(Emitter::new("", "hello:spark")), "it needs an id");
        assert!(register_emitter(Emitter::new("hello:smoke", "hello:spark")));
        assert!(!register_emitter(Emitter::new("hello:smoke", "hello:spark")), "it replaces");
        register_emitter(Emitter::new("hello:smoke", "hello:spark").rate(4.0).seed(3));
        let handle = create_emitter("hello:smoke").expect("registered");
        assert!(pause_emitter(handle));
        assert_eq!(advance_emitters(1.0), 0, "a paused emitter earns nothing");
        assert!(resume_emitter(handle));
        assert_eq!(advance_emitters(0.5), 2);
        assert!(stop_emitter(handle));
        assert!(!stop_emitter(handle));
        assert_eq!(advance_emitters(1.0), 0);
        assert!(!pause_emitter(handle));
        assert!(!resume_emitter(handle));
        assert!(!set_emission_rate(handle, 1.0));
        assert!(!set_emission_shape(handle, EmissionShape::Point));
        assert!(unregister_emitter("hello:smoke"));
        assert!(!unregister_emitter("hello:smoke"));
    }

    #[test]
    fn effects_are_declarations_a_mod_reads_back() {
        let _serial = serial();
        fresh();
        register(ParticleType::new("hello:spark"));
        assert!(register(ParticleType::new("hello:ash")));
        assert!(register_effect(VisualEffect::new("hello:campfire").kind("hello:spark")));
        assert!(!register_effect(VisualEffect::new("hello:campfire")), "it replaces");
        register_effect(
            VisualEffect::new("hello:campfire")
                .emitter("hello:smoke")
                .kind("hello:spark")
                .kind("hello:ash")
                .duration(3)
                .looping(true),
        );
        let effect = get_effect("hello:campfire").expect("registered");
        assert_eq!(effect.emitters, vec!["hello:smoke".to_owned()]);
        assert_eq!(
            effect.kinds,
            vec!["hello:spark".to_owned(), "hello:ash".to_owned()]
        );
        assert_eq!(effect.duration, 3);
        assert!(effect.looping);
        assert_eq!(get_effects().len(), 1);
        assert!(unregister_effect("hello:campfire"));
        assert!(!unregister_effect("hello:campfire"));
        assert_eq!(get_effect("hello:campfire"), None);
        assert_eq!(get_type("hello:spark").map(|kind| kind.id), Some("hello:spark".to_owned()));
        assert_eq!(get_types().len(), 2);
        assert!(unregister("hello:spark"));
        assert!(!unregister("hello:spark"));
        assert_eq!(create("hello:new").id, "hello:new");
    }

    #[test]
    fn spawn_shapes_answer_real_counts() {
        let _serial = serial();
        fresh();
        register(ParticleType::new("hello:spark").max_age(100.0));
        assert_eq!(spawn("nothing"), None, "an unregistered kind makes nothing");
        assert_eq!(spawn_along_line("hello:spark", (0.0, 0.0, 0.0), (3.0, 0.0, 0.0), 4).len(), 4);
        assert_eq!(
            get(1).expect("the first one").position,
            (0.0, 0.0, 0.0),
            "the line starts at the first endpoint"
        );
        assert_eq!(spawn_in_sphere("hello:spark", (1.0, 1.0, 1.0), 2.0, 16, 5).len(), 16);
        for handle in 5..=20 {
            let particle = get(handle).expect("there");
            let (x, y, z) = particle.position;
            let dx = x - 1.0;
            let dy = y - 1.0;
            let dz = z - 1.0;
            assert!((dx * dx + dy * dy + dz * dz).sqrt() <= 2.0 + 1e-4);
        }
        assert_eq!(spawn_in_box("hello:spark", (0.0, 0.0, 0.0), (1.0, 1.0, 1.0), 0, 1).len(), 0);
        assert_eq!(spawn_burst("nothing", (0.0, 0.0, 0.0), 3, 1.0, 1).len(), 0);
        assert_eq!(clear(), 20);
        assert_eq!(get_count(), 0);
    }

    #[test]
    fn every_refusal_answers_false_or_empty_with_a_reason() {
        let _serial = serial();
        fresh();
        assert!(!set_texture(1, "hello:skin"));
        assert!(!set_animation(1, "hello:spin"));
        assert!(!set_blend_mode(1, BlendMode::Additive));
        assert!(!register_particle_renderer("hello:renderer"));
        assert!(client_particles().is_empty());
        assert_eq!(spawn_into_world("hello:spark", (0.0, 0.0, 0.0)), None);
        let refusals = refusals();
        assert_eq!(refusals.len(), 5);
        for (what, why) in refusals {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_eq!(
            why_unknown("texture"),
            why_unknown("texture"),
            "the reason is stable"
        );
        assert!(why_unknown("something else").contains("not something"));
    }
}
