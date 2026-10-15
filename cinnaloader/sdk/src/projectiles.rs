//! Projectiles: a ballistic body a mod declares, launches and steps, the arithmetic that
//! decides what it hits, and the client's world and entity store, refused.
//!
//! A declaration is real data — an id, damage, a speed, gravity, drag, a lifetime, a range, a
//! collision behaviour, an impact behaviour and whether the path is recorded — and every mod
//! can read it back. A launched projectile is a real body: [`Projectile::step`] applies gravity
//! and then drag, in that order, to the velocity, integrates the position, counts the distance
//! travelled against the range, ticks the lifetime down and records the trail. [`Projectile::would_hit`]
//! casts the step's own motion against an axis-aligned box or a sphere using [`crate::math`],
//! and [`Projectile::collide`] applies the bounce-or-despawn rule.
//!
//! What is refused is every live projectile: spawning one into the client's world and reading
//! its entity store, each with its reason in [`why_unknown`], logged at the refusing entry
//! point.
//!
//! ```
//! use cinnaloader::math::Vec3;
//! use cinnaloader::projectiles::{self, CollisionBehavior, ProjectileType};
//!
//! let arrow = ProjectileType::new("arrow")
//!     .damage(2.0)
//!     .speed(1.5)
//!     .gravity(0.05)
//!     .drag(0.99)
//!     .lifetime(100)
//!     .collision(CollisionBehavior::Bounce);
//! assert!(projectiles::register_type(arrow));
//!
//! let mut shot = projectiles::launch("arrow", Vec3::new(0.0, 10.0, 0.0), Vec3::X, "hello:archer")
//!     .expect("declared");
//! shot.step(1.0);
//! assert!(shot.velocity().y < 0.0, "gravity pulls it down");
//! assert_eq!(shot.trail().len(), 2, "the launch point and one step");
//! assert!(projectiles::get_type("arrow").is_some());
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::math::{self, Aabb, EPSILON, Ray, Sphere, Vec3};

/// Why a question about the client's world or its live projectiles has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "world" => {
            "collision against the client's world needs its voxel store, which this host cannot \
             read"
                .to_owned()
        }
        "store" => {
            "the client's entity store is the only place a live projectile exists; reading it \
             needs the hooks section 10 lists as never"
                .to_owned()
        }
        "spawn" => {
            "spawning into the client's world is the server's, and this host cannot even ask the \
             client to send it"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about live projectiles.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["world", "store", "spawn"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about a live projectile: {}", why_unknown(what));
    false
}

/// What a projectile does when it touches something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CollisionBehavior {
    #[default]
    Despawn,
    Bounce,
    PassThrough,
}

impl CollisionBehavior {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Despawn => "despawn",
            Self::Bounce => "bounce",
            Self::PassThrough => "pass_through",
        }
    }
}

impl fmt::Display for CollisionBehavior {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a projectile does to what it hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImpactBehavior {
    #[default]
    Damage,
    NoDamage,
    Stick,
}

impl ImpactBehavior {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Damage => "damage",
            Self::NoDamage => "no_damage",
            Self::Stick => "stick",
        }
    }
}

/// What a step or a collision resolved to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impact {
    Bounced,
    Despawned,
    PassedThrough,
}

/// A projectile type a mod declares.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectileType {
    pub id: String,
    pub owner: String,
    pub damage: f64,
    pub speed: f64,
    pub gravity: f64,
    pub drag: f64,
    /// The maximum number of steps it lives for.
    pub lifetime: u32,
    /// How far it may travel, with `0` meaning no limit.
    pub range: f64,
    pub collision: CollisionBehavior,
    pub impact: ImpactBehavior,
    pub trail: bool,
    pub hit_block_handler: Option<String>,
    pub hit_entity_handler: Option<String>,
    pub expire_handler: Option<String>,
    /// Whether the client has been told about it, which this host cannot do.
    pub registered_with_client: bool,
}

impl ProjectileType {
    #[must_use]
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            damage: 1.0,
            speed: 1.0,
            gravity: crate::physics::GRAVITY,
            drag: crate::physics::VERTICAL_DRAG,
            lifetime: 100,
            range: 0.0,
            collision: CollisionBehavior::Despawn,
            impact: ImpactBehavior::Damage,
            trail: true,
            hit_block_handler: None,
            hit_entity_handler: None,
            expire_handler: None,
            registered_with_client: false,
        }
    }

    #[must_use]
    pub fn damage(mut self, damage: f64) -> Self {
        self.damage = damage;
        self
    }

    #[must_use]
    pub fn speed(mut self, speed: f64) -> Self {
        self.speed = speed;
        self
    }

    #[must_use]
    pub fn gravity(mut self, gravity: f64) -> Self {
        self.gravity = gravity;
        self
    }

    #[must_use]
    pub fn drag(mut self, drag: f64) -> Self {
        self.drag = drag;
        self
    }

    #[must_use]
    pub fn lifetime(mut self, lifetime: u32) -> Self {
        self.lifetime = lifetime;
        self
    }

    #[must_use]
    pub fn range(mut self, range: f64) -> Self {
        self.range = range;
        self
    }

    #[must_use]
    pub fn collision(mut self, collision: CollisionBehavior) -> Self {
        self.collision = collision;
        self
    }

    #[must_use]
    pub fn impact(mut self, impact: ImpactBehavior) -> Self {
        self.impact = impact;
        self
    }

    #[must_use]
    pub fn trail(mut self, trail: bool) -> Self {
        self.trail = trail;
        self
    }

    /// The registry id, with the owning mod in front.
    #[must_use]
    pub fn registry_id(&self) -> String {
        format!("{}:{}", self.owner, self.id)
    }
}

impl fmt::Display for ProjectileType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} ({}, {} damage, {} speed, gravity {}, drag {}, {} ticks, {}, {})",
            self.id,
            self.owner,
            self.damage,
            self.speed,
            self.gravity,
            self.drag,
            self.lifetime,
            self.collision,
            self.impact.as_str()
        )
    }
}

/// Every reason a projectile declaration is not one this host would accept. Empty means valid.
#[must_use]
pub fn validate_type(projectile_type: &ProjectileType) -> Vec<String> {
    let mut problems = Vec::new();
    if projectile_type.id.trim().is_empty() {
        problems.push("the id is empty".to_owned());
    }
    if projectile_type.speed <= 0.0 {
        problems.push(format!("the speed is not positive ({})", projectile_type.speed));
    }
    if projectile_type.lifetime == 0 {
        problems.push("the lifetime is zero".to_owned());
    }
    if projectile_type.damage < 0.0 {
        problems.push("the damage is negative".to_owned());
    }
    if projectile_type.range < 0.0 {
        problems.push("the range is negative".to_owned());
    }
    problems
}

fn registry() -> &'static Mutex<BTreeMap<String, ProjectileType>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, ProjectileType>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, ProjectileType>) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Declares a projectile type, refusing an empty id, a non-positive speed or a zero lifetime.
pub fn register_type(projectile_type: ProjectileType) -> bool {
    let problems = validate_type(&projectile_type);
    if !problems.is_empty() {
        crate::warn!(
            "{} refused the projectile {}: {}",
            crate::metadata::id(),
            projectile_type.id,
            problems.join("; ")
        );
        return false;
    }
    with(|registry| registry.insert(projectile_type.id.clone(), projectile_type).is_none())
}

/// Forgets a declared projectile type.
pub fn unregister_type(id: &str) -> bool {
    with(|registry| registry.remove(id).is_some())
}

/// A declared projectile type, by id.
#[must_use]
pub fn get_type(id: &str) -> Option<ProjectileType> {
    with(|registry| registry.get(id).cloned())
}

/// Every declared projectile type, in id order.
#[must_use]
pub fn get_types() -> Vec<ProjectileType> {
    with(|registry| registry.values().cloned().collect())
}

/// Every declared projectile type, grouped by the mod that declared it.
#[must_use]
pub fn registry_by_mod() -> Vec<(String, Vec<String>)> {
    with(|registry| {
        let mut by_mod: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for projectile_type in registry.values() {
            by_mod
                .entry(projectile_type.owner.clone())
                .or_default()
                .push(projectile_type.id.clone());
        }
        by_mod.into_iter().collect()
    })
}

fn mutate(id: &str, change: impl FnOnce(&mut ProjectileType)) -> bool {
    with(|registry| match registry.get_mut(id) {
        Some(projectile_type) => {
            change(projectile_type);
            true
        }
        None => false,
    })
}

/// Records the handler run when a projectile of this type hits a block.
pub fn on_hit_block(id: &str, handler: &str) -> bool {
    mutate(id, |projectile_type| projectile_type.hit_block_handler = Some(handler.to_owned()))
}

/// Records the handler run when a projectile of this type hits an entity.
pub fn on_hit_entity(id: &str, handler: &str) -> bool {
    mutate(id, |projectile_type| projectile_type.hit_entity_handler = Some(handler.to_owned()))
}

/// Records the handler run when a projectile of this type expires.
pub fn on_expire(id: &str, handler: &str) -> bool {
    mutate(id, |projectile_type| projectile_type.expire_handler = Some(handler.to_owned()))
}

/// A live projectile a mod owns: a ballistic body, the rules it was declared with, and its
/// path.
#[derive(Debug, Clone, PartialEq)]
pub struct Projectile {
    pub projectile_type: String,
    pub position: Vec3,
    pub velocity: Vec3,
    pub owner: Option<String>,
    pub gravity: f64,
    pub drag: f64,
    pub damage: f64,
    pub range: f64,
    pub lifetime: u32,
    pub max_lifetime: u32,
    pub collision: CollisionBehavior,
    pub impact: ImpactBehavior,
    pub collect_trail: bool,
    pub trail: Vec<Vec3>,
    pub distance_travelled: f64,
    pub ticks: u64,
    pub alive: bool,
}

impl Projectile {
    /// One tick of ballistic motion. Gravity lands on the vertical velocity first, then drag
    /// multiplies the whole velocity, then the position integrates that velocity. The distance
    /// travelled is counted against the range, the lifetime ticks down, and the new position
    /// joins the trail. Answers whether it is still alive.
    pub fn step(&mut self, dt: f64) -> bool {
        if !self.alive {
            return false;
        }
        self.velocity.y -= self.gravity * dt;
        let keep = self.drag.powf(dt);
        self.velocity = self.velocity * keep;
        self.position = self.position + self.velocity * dt;
        self.distance_travelled += self.velocity.length() * dt;
        self.ticks += 1;
        if self.collect_trail {
            self.trail.push(self.position);
        }
        self.lifetime = self.lifetime.saturating_sub(1);
        if self.lifetime == 0 || (self.range > 0.0 && self.distance_travelled >= self.range) {
            self.alive = false;
        }
        self.alive
    }

    /// Whether this step's own motion would meet a target.
    #[must_use]
    pub fn would_hit(&self, target: &HitTarget) -> bool {
        let reach = self.velocity.length();
        if reach < EPSILON {
            return false;
        }
        let ray = Ray::new(self.position, self.velocity);
        let hit = match target {
            HitTarget::Box(aabb) => math::intersect_aabb(&ray, aabb),
            HitTarget::Sphere(sphere) => math::intersect_ray(&ray, sphere),
        };
        hit.is_some_and(|hit| hit.distance <= reach + EPSILON)
    }

    /// What a projectile's collision behaviour does on contact, given the surface normal.
    pub fn collide(&mut self, normal: Vec3) -> Impact {
        match self.collision {
            CollisionBehavior::Bounce => {
                self.velocity = math::reflect(self.velocity, normal);
                Impact::Bounced
            }
            CollisionBehavior::Despawn => {
                self.alive = false;
                Impact::Despawned
            }
            CollisionBehavior::PassThrough => Impact::PassedThrough,
        }
    }

    /// Ends it, as an expiry or a removal.
    pub fn despawn(&mut self) {
        self.alive = false;
    }

    #[must_use]
    pub fn position(&self) -> Vec3 {
        self.position
    }

    #[must_use]
    pub fn velocity(&self) -> Vec3 {
        self.velocity
    }

    #[must_use]
    pub fn trail(&self) -> &[Vec3] {
        &self.trail
    }

    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.alive
    }

    #[must_use]
    pub fn damage(&self) -> f64 {
        self.damage
    }
}

/// What [`Projectile::would_hit`] is asked about: a box, or a point and a radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HitTarget {
    Box(Aabb),
    Sphere(Sphere),
}

/// Builds a fresh projectile of a declared type at rest. `None` when nothing is declared.
#[must_use]
pub fn create(id: &str, at: Vec3) -> Option<Projectile> {
    let projectile_type = get_type(id)?;
    let mut trail = Vec::new();
    if projectile_type.trail {
        trail.push(at);
    }
    Some(Projectile {
        projectile_type: projectile_type.id,
        position: at,
        velocity: Vec3::ZERO,
        owner: None,
        gravity: projectile_type.gravity,
        drag: projectile_type.drag,
        damage: projectile_type.damage,
        range: projectile_type.range,
        lifetime: projectile_type.lifetime,
        max_lifetime: projectile_type.lifetime,
        collision: projectile_type.collision,
        impact: projectile_type.impact,
        collect_trail: projectile_type.trail,
        trail,
        distance_travelled: 0.0,
        ticks: 0,
        alive: true,
    })
}

/// Builds a projectile and fires it along a direction at the type's speed, owed to an owner.
#[must_use]
pub fn launch(id: &str, from: Vec3, direction: Vec3, owner: &str) -> Option<Projectile> {
    let mut projectile = create(id, from)?;
    let speed = get_type(id).map_or(1.0, |projectile_type| projectile_type.speed);
    projectile.velocity = direction.normalize() * speed;
    projectile.owner = Some(owner.to_owned());
    Some(projectile)
}

/// Who launched a projectile.
#[must_use]
pub fn get_owner(projectile: &Projectile) -> Option<&str> {
    projectile.owner.as_deref()
}

pub fn set_owner(projectile: &mut Projectile, owner: &str) {
    projectile.owner = Some(owner.to_owned());
}

#[must_use]
pub fn get_velocity(projectile: &Projectile) -> Vec3 {
    projectile.velocity
}

pub fn set_velocity(projectile: &mut Projectile, velocity: Vec3) {
    projectile.velocity = velocity;
}

pub fn set_gravity(projectile: &mut Projectile, gravity: f64) {
    projectile.gravity = gravity;
}

pub fn set_drag(projectile: &mut Projectile, drag: f64) {
    projectile.drag = drag;
}

pub fn set_damage(projectile: &mut Projectile, damage: f64) {
    projectile.damage = damage;
}

pub fn set_range(projectile: &mut Projectile, range: f64) {
    projectile.range = range;
}

pub fn set_lifetime(projectile: &mut Projectile, lifetime: u32) {
    projectile.lifetime = lifetime;
    projectile.max_lifetime = lifetime;
}

pub fn set_collision_behavior(projectile: &mut Projectile, collision: CollisionBehavior) {
    projectile.collision = collision;
}

pub fn set_impact_behavior(projectile: &mut Projectile, impact: ImpactBehavior) {
    projectile.impact = impact;
}

pub fn set_trail(projectile: &mut Projectile, trail: bool) {
    projectile.collect_trail = trail;
}

/// The live half: the client's world and entity store, each refused with its reason.
pub mod live {
    use super::Vec3;

    /// Spawns into the client's world. `false`.
    pub fn spawn(_id: &str, _at: Vec3) -> bool {
        super::refused("spawn")
    }

    /// Removes from the client's entity store. `false`.
    pub fn despawn(_id: &str) -> bool {
        super::refused("store")
    }

    /// A live projectile by id. `None`.
    #[must_use]
    pub fn get(_id: &str) -> Option<()> {
        None
    }

    /// A live projectile's position. `None`.
    #[must_use]
    pub fn get_position(_id: &str) -> Option<Vec3> {
        None
    }

    /// Every live projectile. Empty.
    #[must_use]
    pub fn all() -> Vec<String> {
        Vec::new()
    }
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

    fn arrow() -> ProjectileType {
        ProjectileType::new("arrow")
            .damage(2.0)
            .speed(1.5)
            .gravity(0.05)
            .drag(0.99)
            .lifetime(4)
            .range(50.0)
            .collision(CollisionBehavior::Bounce)
            .impact(ImpactBehavior::Damage)
    }

    #[test]
    fn a_declared_projectile_can_be_read_back() {
        let _serial = serial();
        fresh();
        assert!(register_type(arrow()));
        let declared = get_type("arrow").expect("declared");
        assert_eq!(declared.damage, 2.0);
        assert_eq!(declared.speed, 1.5);
        assert_eq!(declared.gravity, 0.05);
        assert_eq!(declared.drag, 0.99);
        assert_eq!(declared.lifetime, 4);
        assert_eq!(declared.collision, CollisionBehavior::Bounce);
        assert_eq!(declared.impact, ImpactBehavior::Damage);
        assert!(!declared.registered_with_client);
        assert!(get_types().len() == 1);
        assert_eq!(registry_by_mod().len(), 1);
        assert!(on_hit_block("arrow", "hello:on_block"));
        assert!(on_hit_entity("arrow", "hello:on_entity"));
        assert!(on_expire("arrow", "hello:on_expire"));
        assert!(!on_hit_block("nothing", "hello:on_block"));
        let declared = get_type("arrow").expect("declared");
        assert_eq!(declared.hit_block_handler.as_deref(), Some("hello:on_block"));
        assert_eq!(declared.hit_entity_handler.as_deref(), Some("hello:on_entity"));
        assert_eq!(declared.expire_handler.as_deref(), Some("hello:on_expire"));
        assert!(unregister_type("arrow"));
        assert!(!unregister_type("arrow"));
        assert!(declared.to_string().contains("arrow"));
    }

    #[test]
    fn an_invalid_projectile_is_refused() {
        let _serial = serial();
        fresh();
        assert!(validate_type(&arrow()).is_empty());
        assert!(register_type(arrow()));

        let nameless = ProjectileType::new("").speed(1.0).lifetime(1);
        assert!(validate_type(&nameless)[0].contains("id is empty"));
        assert!(!register_type(nameless));

        let still = ProjectileType::new("still").speed(0.0).lifetime(1);
        assert!(validate_type(&still)[0].contains("speed is not positive"));
        assert!(!register_type(still));

        let brief = ProjectileType::new("brief").speed(1.0).lifetime(0);
        assert!(validate_type(&brief)[0].contains("lifetime is zero"));
        assert!(!register_type(brief));

        assert_eq!(get_types().len(), 1, "only the good one landed");
    }

    #[test]
    fn a_ballistic_step_applies_gravity_then_drag() {
        let _serial = serial();
        fresh();
        register_type(arrow());
        let mut shot = create("arrow", Vec3::new(0.0, 10.0, 0.0)).expect("declared");
        assert_eq!(shot.position(), Vec3::new(0.0, 10.0, 0.0));
        assert_eq!(shot.trail().len(), 1);
        set_velocity(&mut shot, Vec3::new(1.0, 0.0, 0.0));
        set_gravity(&mut shot, 0.08);
        set_drag(&mut shot, 0.5);
        assert!(shot.step(1.0));
        assert!(
            shot.position().approx_eq(Vec3::new(0.5, 9.96, 0.0), 1e-9),
            "{:?}",
            shot.position()
        );
        assert!(shot.velocity().approx_eq(Vec3::new(0.5, -0.04, 0.0), 1e-9));
        assert!(shot.step(1.0));
        assert!(
            shot.position().approx_eq(Vec3::new(0.75, 9.9, 0.0), 1e-9),
            "{:?}",
            shot.position()
        );
        assert!(shot.velocity().approx_eq(Vec3::new(0.25, -0.06, 0.0), 1e-9));
        assert_eq!(shot.trail().len(), 3, "the launch and two steps");
    }

    #[test]
    fn a_projectile_hits_a_box_and_a_sphere_or_misses() {
        let _serial = serial();
        fresh();
        register_type(arrow());
        let mut shot = create("arrow", Vec3::ZERO).expect("declared");
        set_velocity(&mut shot, Vec3::new(2.0, 0.0, 0.0));

        let boxed = Aabb::new(Vec3::new(1.0, -0.5, -0.5), Vec3::new(2.0, 0.5, 0.5));
        assert!(shot.would_hit(&HitTarget::Box(boxed)), "the box is in the way");
        let high = Aabb::new(Vec3::new(1.0, 5.0, -0.5), Vec3::new(2.0, 6.0, 0.5));
        assert!(!shot.would_hit(&HitTarget::Box(high)), "the box is above it");

        let ball = Sphere::new(Vec3::new(1.0, 0.0, 0.0), 0.5);
        assert!(shot.would_hit(&HitTarget::Sphere(ball)), "the ball is in the way");
        let far = Sphere::new(Vec3::new(5.0, 0.0, 0.0), 0.5);
        assert!(!shot.would_hit(&HitTarget::Sphere(far)), "past this step's reach");

        shot.velocity = Vec3::ZERO;
        assert!(!shot.would_hit(&HitTarget::Sphere(ball)), "standing still hits nothing");
    }

    #[test]
    fn a_bounce_reflects_and_a_despawn_ends_it() {
        let _serial = serial();
        fresh();
        register_type(arrow());
        let mut shot = create("arrow", Vec3::ZERO).expect("declared");
        set_collision_behavior(&mut shot, CollisionBehavior::Bounce);
        set_velocity(&mut shot, Vec3::new(1.0, -1.0, 0.0));
        assert_eq!(shot.collide(Vec3::Y), Impact::Bounced);
        assert!(shot.velocity().approx_eq(Vec3::new(1.0, 1.0, 0.0), 1e-9));
        assert!(shot.is_alive());

        set_collision_behavior(&mut shot, CollisionBehavior::PassThrough);
        assert_eq!(shot.collide(Vec3::Y), Impact::PassedThrough);
        assert!(shot.is_alive());

        set_collision_behavior(&mut shot, CollisionBehavior::Despawn);
        assert_eq!(shot.collide(Vec3::Y), Impact::Despawned);
        assert!(!shot.is_alive());
        shot.despawn();
        assert!(!shot.step(1.0), "a dead projectile does not move");
    }

    #[test]
    fn a_lifetime_and_a_range_end_the_flight() {
        let _serial = serial();
        fresh();
        register_type(arrow());
        let mut shot = create("arrow", Vec3::ZERO).expect("declared");
        set_velocity(&mut shot, Vec3::new(1.0, 0.0, 0.0));
        set_gravity(&mut shot, 0.0);
        set_drag(&mut shot, 1.0);
        set_trail(&mut shot, true);
        set_lifetime(&mut shot, 2);
        assert_eq!(shot.max_lifetime, 2);
        assert!(shot.step(1.0));
        assert!(shot.is_alive());
        assert!(!shot.step(1.0), "the lifetime ran out");
        assert!(!shot.is_alive());
        assert_eq!(shot.trail().len(), 3, "the launch and two steps");

        let mut ranged = create("arrow", Vec3::ZERO).expect("declared");
        set_velocity(&mut ranged, Vec3::new(1.0, 0.0, 0.0));
        set_gravity(&mut ranged, 0.0);
        set_drag(&mut ranged, 1.0);
        set_range(&mut ranged, 0.5);
        set_lifetime(&mut ranged, 100);
        assert!(!ranged.step(1.0), "it went past its range in one step");
        assert!(!ranged.is_alive());
    }

    #[test]
    fn launching_sets_the_owner_and_the_speed() {
        let _serial = serial();
        fresh();
        register_type(arrow());
        let shot = launch("arrow", Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.0, 0.0, 1.0), "hello:archer")
            .expect("declared");
        assert_eq!(get_owner(&shot), Some("hello:archer"));
        assert!(get_velocity(&shot).approx_eq(Vec3::new(0.0, 0.0, 1.5), 1e-9));
        assert_eq!(shot.damage(), 2.0);
        assert!(launch("nothing", Vec3::ZERO, Vec3::X, "hello:archer").is_none());

        let mut shot = shot;
        set_owner(&mut shot, "hello:other");
        assert_eq!(get_owner(&shot), Some("hello:other"));
        set_impact_behavior(&mut shot, ImpactBehavior::Stick);
        assert_eq!(shot.impact, ImpactBehavior::Stick);
    }

    #[test]
    fn the_clients_world_and_store_are_refused_with_a_reason() {
        let _serial = serial();
        fresh();
        assert!(!live::spawn("arrow", Vec3::ZERO));
        assert!(!live::despawn("arrow"));
        assert!(live::get("arrow").is_none());
        assert!(live::get_position("arrow").is_none());
        assert!(live::all().is_empty());
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
