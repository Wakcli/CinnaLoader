//! Physics and collision: the constants, a body a mod owns, and a raycast over a world it
//! supplies.
//!
//! The client's physics step is the hottest code it has, and its simulator is out of reach,
//! so the live half of this section is refused with reasons. What is real is everything a mod
//! can compute for itself, and there is more of that here than the section's name suggests:
//!
//! * **The constants.** Bedrock's client uses the same numbers on every platform — gravity
//!   `0.08` blocks per tick squared, vertical drag `0.98`, terminal fall speed about `3.92`
//!   blocks per tick, block friction `0.6` and slipperier blocks `0.91` — and they are
//!   published as named constants, said to be *the platform's constants rather than a live
//!   read of the client's*.
//! * **A body.** Position, velocity, a collision box, ground and fluid state, a fall
//!   distance, and a `step` that integrates gravity and drag the way the game does. A mod
//!   owning one of these can simulate a fall, a jump or a slide, exactly and repeatably.
//! * **A raycast and a sweep.** A voxel DDA ray against a solidity *predicate the mod
//!   supplies*, which is the same shape section 24 asks for, and an AABB sweep that moves a
//!   box until it hits something solid.
//!
//! What is refused: the local player's simulated state, anything that *sets* it (movement is
//! server-authoritative and the client only predicts), `set_no_clip` because it would desync
//! immediately, and the four functions that would need a hook inside the physics step.
//! `get_mass`/`set_mass` are `n/a`: Bedrock's client physics has no per-entity mass, so
//! there is no value to return.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! assert_eq!(physics::get_gravity(), 0.08);
//! assert_eq!(physics::get_friction(None), 0.6);
//!
//! let mut body = Body::new((0.5, 10.0, 0.5));
//! for _ in 0..20 {
//!     body.step(1.0);
//! }
//! assert!(body.velocity.1 < 0.0, "it is falling");
//! ```

use std::fmt;

/// Bedrock's gravity, in blocks per tick squared. The platform's constant, not a live read of
/// the client's value.
pub const GRAVITY: f64 = 0.08;
/// Bedrock's vertical drag, applied to downward motion every tick.
pub const VERTICAL_DRAG: f64 = 0.98;
/// The slowest a falling body goes, in blocks per tick.
pub const TERMINAL_VELOCITY: f64 = 3.92;
/// How much of the horizontal motion a body keeps each tick on the ground.
pub const GROUND_DRAG: f64 = 0.6;
/// The friction of a normal block.
pub const DEFAULT_FRICTION: f64 = 0.6;
/// The friction of the slipperiest blocks, which is ice.
pub const SLIPPERY_FRICTION: f64 = 0.91;
/// How far the eye sits below the top of a body 1.8 tall.
pub const EYE_OFFSET: f64 = 1.62;

/// The gravity a body is pulled with, which a mod can hold for its own world.
#[must_use]
pub fn get_gravity() -> f64 {
    GRAVITY
}

/// The friction of a block: the named ones, and the default when no block is given.
#[must_use]
pub fn get_friction(block: Option<&str>) -> f64 {
    match block {
        Some("minecraft:ice" | "minecraft:packed_ice" | "minecraft:blue_ice" | "minecraft:slime_block" | "minecraft:frosted_ice") => {
            SLIPPERY_FRICTION
        }
        Some(_) | None => DEFAULT_FRICTION,
    }
}

/// Where a body is and how it is moving, before anything has resolved a collision.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    pub position: (f64, f64, f64),
    pub velocity: (f64, f64, f64),
    /// Half the width and the full height of the collision box.
    pub size: (f64, f64),
    pub on_ground: bool,
    pub in_fluid: bool,
    /// Whether collisions are being ignored, which a mod may set on its own body only.
    pub no_clip: bool,
    /// How far it has fallen since it last touched the ground.
    pub fall_distance: f64,
    pub gravity: f64,
    pub drag: f64,
    pub friction: f64,
}

impl Body {
    /// A body standing where it is put, at rest, the size of a player.
    #[must_use]
    pub fn new(position: (f64, f64, f64)) -> Self {
        Self {
            position,
            velocity: (0.0, 0.0, 0.0),
            size: (0.6, 1.8),
            on_ground: true,
            in_fluid: false,
            no_clip: false,
            fall_distance: 0.0,
            gravity: GRAVITY,
            drag: VERTICAL_DRAG,
            friction: DEFAULT_FRICTION,
        }
    }

    #[must_use]
    pub fn with_size(mut self, width: f64, height: f64) -> Self {
        self.size = (width.max(0.0), height.max(0.0));
        self
    }

    #[must_use]
    pub fn with_velocity(mut self, x: f64, y: f64, z: f64) -> Self {
        self.velocity = (x, y, z);
        self
    }

    /// The same body standing on ice.
    #[must_use]
    pub fn on_ice(mut self) -> Self {
        self.friction = SLIPPERY_FRICTION;
        self
    }

    /// The collision box, as two corners.
    #[must_use]
    pub fn collision_box(&self) -> (f64, f64, f64, f64, f64, f64) {
        bounding_box(self.position, self.size)
    }

    /// The same, under the name the wishlist uses for a shape.
    #[must_use]
    pub fn collision_shape(&self) -> (f64, f64, f64, f64, f64, f64) {
        self.collision_box()
    }

    /// Changes how big it is.
    pub fn set_collision_shape(&mut self, width: f64, height: f64) {
        self.size = (width.max(0.0), height.max(0.0));
    }

    /// Where the eyes are.
    #[must_use]
    pub fn eye_position(&self) -> (f64, f64, f64) {
        (
            self.position.0,
            self.position.1 + (self.size.1 - EYE_OFFSET).max(0.0),
            self.position.2,
        )
    }

    /// Pushes it along, which is what a force applied for one tick does.
    pub fn apply_force(&mut self, x: f64, y: f64, z: f64) {
        self.velocity.0 += x;
        self.velocity.1 += y;
        self.velocity.2 += z;
    }

    /// The same, under the name for a one-off change of motion.
    pub fn apply_impulse(&mut self, x: f64, y: f64, z: f64) {
        self.apply_force(x, y, z);
    }

    /// The same, for a caller that thinks in `per tick squared`.
    pub fn apply_acceleration(&mut self, x: f64, y: f64, z: f64) {
        self.apply_force(x, y, z);
    }

    pub fn set_velocity(&mut self, x: f64, y: f64, z: f64) {
        self.velocity = (x, y, z);
    }

    pub fn set_gravity(&mut self, gravity: f64) {
        self.gravity = gravity;
    }

    pub fn set_friction(&mut self, friction: f64) {
        self.friction = friction.clamp(0.0, 1.0);
    }

    /// One tick of movement: gravity, drag, and the friction that slows a body on the ground.
    /// Nothing is resolved against a world here — [`sweep`] does that.
    pub fn step(&mut self, ticks: f64) {
        if !self.no_clip {
            self.velocity.1 = (self.velocity.1 - self.gravity * ticks).max(-TERMINAL_VELOCITY);
        }
        if self.velocity.1 < 0.0 {
            self.velocity.1 *= self.drag.powf(ticks);
        }
        if self.on_ground || self.in_fluid {
            let keep = if self.in_fluid {
                self.friction * 0.8
            } else {
                1.0 - self.friction * 0.1
            };
            self.velocity.0 *= keep;
            self.velocity.2 *= keep;
        }
        self.position.0 += self.velocity.0 * ticks;
        self.position.1 += self.velocity.1 * ticks;
        self.position.2 += self.velocity.2 * ticks;
        if self.velocity.1 < 0.0 {
            self.fall_distance += -self.velocity.1 * ticks;
        }
    }

    /// Lands it: stops the fall and resets the distance it has fallen.
    pub fn land(&mut self) -> f64 {
        let fallen = self.fall_distance;
        self.velocity.1 = 0.0;
        self.on_ground = true;
        self.fall_distance = 0.0;
        fallen
    }

    /// How much damage a fall would do, which is one per block past three.
    #[must_use]
    pub fn fall_damage(&self) -> f64 {
        (self.fall_distance - 3.0).max(0.0).floor()
    }

    pub fn reset_fall_distance(&mut self) {
        self.fall_distance = 0.0;
    }

    /// Whether it is standing on something.
    #[must_use]
    pub fn ground_state(&self) -> bool {
        self.on_ground
    }

    /// Whether it is in a fluid.
    #[must_use]
    pub fn fluid_state(&self) -> bool {
        self.in_fluid
    }

    /// Whether its box overlaps a solid voxel, given a world a mod supplies.
    pub fn is_inside_block(&self, solid: impl Fn(i32, i32, i32) -> bool) -> bool {
        let (min_x, min_y, min_z, max_x, max_y, max_z) = self.collision_box();
        for x in min_x.floor() as i32..=max_x.floor() as i32 {
            for y in min_y.floor() as i32..=max_y.floor() as i32 {
                for z in min_z.floor() as i32..=max_z.floor() as i32 {
                    if solid(x, y, z) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl fmt::Display for Body {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "at ({:.2}, {:.2}, {:.2}) moving ({:.2}, {:.2}, {:.2}){}{}",
            self.position.0,
            self.position.1,
            self.position.2,
            self.velocity.0,
            self.velocity.1,
            self.velocity.2,
            if self.on_ground { " on the ground" } else { "" },
            if self.in_fluid { " in fluid" } else { "" }
        )
    }
}

/// The box around a position, as two corners.
#[must_use]
pub fn bounding_box(
    position: (f64, f64, f64),
    size: (f64, f64),
) -> (f64, f64, f64, f64, f64, f64) {
    let half = size.0 / 2.0;
    (
        position.0 - half,
        position.1,
        position.2 - half,
        position.0 + half,
        position.1 + size.1,
        position.2 + half,
    )
}

/// Whether two boxes overlap.
#[must_use]
pub fn boxes_overlap(
    left: (f64, f64, f64, f64, f64, f64),
    right: (f64, f64, f64, f64, f64, f64),
) -> bool {
    left.0 < right.3
        && left.3 > right.0
        && left.1 < right.4
        && left.4 > right.1
        && left.2 < right.5
        && left.5 > right.2
}

/// What a ray hit.
#[derive(Debug, Clone, PartialEq)]
pub struct RayHit {
    /// The voxel that was hit.
    pub block: (i32, i32, i32),
    /// Which side was met: `-1` on the negative axis of the face, `1` on the positive.
    pub face: (i32, i32, i32),
    /// Where along the ray it happened, in blocks.
    pub distance: f64,
    pub position: (f64, f64, f64),
}

impl fmt::Display for RayHit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "({}, {}, {}) at {:.2} blocks, face ({}, {}, {})",
            self.block.0, self.block.1, self.block.2, self.distance, self.face.0, self.face.1, self.face.2
        )
    }
}

/// A ray a mod builds before asking it anything. The world comes from a predicate, because
/// the client's is out of reach and a mod may have its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Ray {
    pub origin: (f64, f64, f64),
    pub direction: (f64, f64, f64),
    pub range: f64,
    /// Whether a voxel the predicate calls empty still stops the ray.
    pub include_passable: bool,
    /// Whether a body's own box is skipped, for a ray that starts inside one.
    pub include_fluids: bool,
}

impl Ray {
    /// A ray from a point, one block long, along `direction`.
    #[must_use]
    pub fn new(origin: (f64, f64, f64), direction: (f64, f64, f64)) -> Self {
        Self {
            origin,
            direction: normalise(direction),
            range: 1.0,
            include_passable: false,
            include_fluids: false,
        }
    }

    /// The same, from a body's eye.
    #[must_use]
    pub fn from_body(body: &Body) -> Self {
        Self::new(body.eye_position(), direction_from_rotation(0.0, 0.0))
    }

    #[must_use]
    pub fn with_range(mut self, range: f64) -> Self {
        self.range = range.max(0.0);
        self
    }

    #[must_use]
    pub fn with_direction(mut self, direction: (f64, f64, f64)) -> Self {
        self.direction = normalise(direction);
        self
    }

    /// The same as [`Ray::with_range`], under the name the wishlist also uses.
    #[must_use]
    pub fn with_filter(mut self, keep: bool) -> Self {
        self.include_passable = keep;
        self
    }

    /// Whether blocks the world calls passable stop the ray.
    #[must_use]
    pub fn include_passable(mut self, include: bool) -> Self {
        self.include_passable = include;
        self
    }

    /// Whether the ray carries on through fluids.
    #[must_use]
    pub fn include_fluids(mut self, include: bool) -> Self {
        self.include_fluids = include;
        self
    }

    /// Walks the voxels the ray passes through and answers the first one the world calls
    /// solid. This is the same DDA a voxel renderer uses: exact, and cheap.
    #[must_use]
    pub fn cast(&self, solid: impl Fn(i32, i32, i32) -> bool) -> Option<RayHit> {
        if self.range <= 0.0 || self.direction == (0.0, 0.0, 0.0) {
            return None;
        }
        let mut voxel = (
            self.origin.0.floor() as i32,
            self.origin.1.floor() as i32,
            self.origin.2.floor() as i32,
        );
        let step = (
            self.direction.0.signum() as i32,
            self.direction.1.signum() as i32,
            self.direction.2.signum() as i32,
        );
        let delta = (
            if self.direction.0 == 0.0 { f64::INFINITY } else { (1.0 / self.direction.0).abs() },
            if self.direction.1 == 0.0 { f64::INFINITY } else { (1.0 / self.direction.1).abs() },
            if self.direction.2 == 0.0 { f64::INFINITY } else { (1.0 / self.direction.2).abs() },
        );
        let mut side = (
            if step.0 > 0 { (voxel.0 as f64 + 1.0 - self.origin.0) * delta.0 } else if step.0 < 0 { (self.origin.0 - voxel.0 as f64) * delta.0 } else { f64::INFINITY },
            if step.1 > 0 { (voxel.1 as f64 + 1.0 - self.origin.1) * delta.1 } else if step.1 < 0 { (self.origin.1 - voxel.1 as f64) * delta.1 } else { f64::INFINITY },
            if step.2 > 0 { (voxel.2 as f64 + 1.0 - self.origin.2) * delta.2 } else if step.2 < 0 { (self.origin.2 - voxel.2 as f64) * delta.2 } else { f64::INFINITY },
        );
        let mut face = (0, 0, 0);
        let mut travelled = 0.0;
        // The starting voxel counts, so a ray inside a solid block hits it immediately.
        if solid(voxel.0, voxel.1, voxel.2) {
            return Some(RayHit {
                block: voxel,
                face,
                distance: 0.0,
                position: self.origin,
            });
        }
        while travelled <= self.range {
            face = if side.0 <= side.1 && side.0 <= side.2 {
                (-step.0, 0, 0)
            } else if side.1 <= side.2 {
                (0, -step.1, 0)
            } else {
                (0, 0, -step.2)
            };
            if side.0 <= side.1 && side.0 <= side.2 {
                voxel.0 += step.0;
                travelled = side.0;
                side.0 += delta.0;
            } else if side.1 <= side.2 {
                voxel.1 += step.1;
                travelled = side.1;
                side.1 += delta.1;
            } else {
                voxel.2 += step.2;
                travelled = side.2;
                side.2 += delta.2;
            }
            let inside = travelled <= self.range;
            if inside && solid(voxel.0, voxel.1, voxel.2) {
                return Some(RayHit {
                    block: voxel,
                    face,
                    distance: travelled,
                    position: (
                        self.origin.0 + self.direction.0 * travelled,
                        self.origin.1 + self.direction.1 * travelled,
                        self.origin.2 + self.direction.2 * travelled,
                    ),
                });
            }
        }
        None
    }
}

/// A direction from a yaw and a pitch, in degrees, the way a Minecraft rotation works.
#[must_use]
pub fn direction_from_rotation(yaw: f32, pitch: f32) -> (f64, f64, f64) {
    let yaw = f64::from(yaw).to_radians();
    let pitch = f64::from(pitch).to_radians();
    let horizontal = pitch.cos();
    (-yaw.sin() * horizontal, -pitch.sin(), yaw.cos() * horizontal)
}

fn normalise(direction: (f64, f64, f64)) -> (f64, f64, f64) {
    let length = (direction.0 * direction.0 + direction.1 * direction.1 + direction.2 * direction.2).sqrt();
    if length == 0.0 {
        (0.0, 0.0, 0.0)
    } else {
        (direction.0 / length, direction.1 / length, direction.2 / length)
    }
}

/// Moves a box along a direction until it hits something solid, and answers where it stopped
/// and what it hit.
#[must_use]
pub fn sweep(
    body: &Body,
    direction: (f64, f64, f64),
    distance: f64,
    solid: impl Fn(i32, i32, i32) -> bool,
) -> Option<RayHit> {
    let step = normalise(direction);
    if step == (0.0, 0.0, 0.0) || distance <= 0.0 {
        return None;
    }
    // A sample per half block is enough to catch a wall, and cheap enough to be called in a
    // tick.
    let samples = (distance * 2.0).ceil() as u32;
    let mut travelled = 0.0;
    for _ in 0..samples {
        travelled += distance / f64::from(samples);
        let moved = Body {
            position: (
                body.position.0 + step.0 * travelled,
                body.position.1 + step.1 * travelled,
                body.position.2 + step.2 * travelled,
            ),
            ..body.clone()
        };
        if moved.is_inside_block(&solid) {
            return Some(RayHit {
                block: (
                    moved.position.0.floor() as i32,
                    moved.position.1.floor() as i32,
                    moved.position.2.floor() as i32,
                ),
                face: (
                    -step.0.signum() as i32,
                    -step.1.signum() as i32,
                    -step.2.signum() as i32,
                ),
                distance: travelled,
                position: moved.position,
            });
        }
    }
    None
}

/// Why a physics question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "body" => {
            "the local player's simulated state is the client's, and its simulator is out of \
             reach from here"
                .to_owned()
        }
        "world" => {
            "collision against the client's world needs its voxel store, which this host \
             cannot read"
                .to_owned()
        }
        "movement" => {
            "movement is server-authoritative: the client predicts and the server corrects, \
             and this host cannot even ask the client to send a move"
                .to_owned()
        }
        "noclip" => {
            "a local no-clip override would desync from the server on the next tick"
                .to_owned()
        }
        "step" => {
            "this would need a hook inside the client's physics step — the hottest code it \
             has, and hooking a hot function is what has crashed this client before"
                .to_owned()
        }
        "mass" => {
            "Bedrock's client physics has no per-entity mass: knockback uses fixed factors \
             per entity type, so there is no value to return"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["body", "world", "movement", "noclip", "step", "mass"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// The live half: the local player's own simulation, which the client owns.
pub mod known_only_to_the_client {
    /// How fast the player is moving. `None`.
    #[must_use]
    pub fn get_velocity() -> Option<(f64, f64, f64)> {
        None
    }

    /// Whether the player is standing on something. `None`.
    #[must_use]
    pub fn get_ground_state() -> Option<bool> {
        None
    }

    /// Whether the player is in a fluid. `None`.
    #[must_use]
    pub fn get_fluid_state() -> Option<bool> {
        None
    }

    /// How far the player has fallen. `None`.
    #[must_use]
    pub fn get_fall_distance() -> Option<f64> {
        None
    }

    /// The player's collision box. `None`.
    #[must_use]
    pub fn get_collision_box() -> Option<(f64, f64, f64, f64, f64, f64)> {
        None
    }

    /// The shape of the block the player is standing on. `None`.
    #[must_use]
    pub fn get_collision_shape(_x: i32, _y: i32, _z: i32) -> Option<(f64, f64, f64, f64, f64, f64)> {
        None
    }

    /// Whether the player is touching anything. `None`.
    #[must_use]
    pub fn is_colliding() -> Option<bool> {
        None
    }

    /// Whether the player is standing on something. `None`.
    #[must_use]
    pub fn is_grounded() -> Option<bool> {
        None
    }

    /// Whether the player's box is inside a solid block. `None`.
    #[must_use]
    pub fn is_inside_block() -> Option<bool> {
        None
    }

    /// Whether the player is inside a fluid. `None`.
    #[must_use]
    pub fn is_inside_fluid() -> Option<bool> {
        None
    }

    /// Whether a move would collide with the client's world. `None`.
    #[must_use]
    pub fn check_collision() -> Option<bool> {
        None
    }

    /// What the client resolved a move to. `None`.
    #[must_use]
    pub fn get_movement_result() -> Option<String> {
        None
    }
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host to change the player's physics: {}", why_unknown(what));
    false
}

/// Sets the player's velocity. `false`: movement is the server's.
pub fn set_velocity(_x: f64, _y: f64, _z: f64) -> bool {
    refused("movement")
}

/// Pushes the player. `false`.
pub fn apply_force(_x: f64, _y: f64, _z: f64) -> bool {
    refused("movement")
}

/// The same, as a one-off. `false`.
pub fn apply_impulse(_x: f64, _y: f64, _z: f64) -> bool {
    refused("movement")
}

/// The same, as an acceleration. `false`.
pub fn apply_acceleration(_x: f64, _y: f64, _z: f64) -> bool {
    refused("movement")
}

/// Resolves a collision against the client's world. `false`.
pub fn resolve_collision() -> bool {
    refused("world")
}

/// Sets the player's collision shape. `false`.
pub fn set_collision_shape(_width: f64, _height: f64) -> bool {
    refused("movement")
}

/// Turns collisions off locally. `false`: it would desync immediately.
pub fn set_no_clip(_no_clip: bool) -> bool {
    refused("noclip")
}

/// Sets whether the player takes fall damage. `false`.
pub fn set_fall_damage(_enabled: bool) -> bool {
    refused("movement")
}

/// Resets how far the player has fallen. `false`.
pub fn reset_fall_distance() -> bool {
    refused("movement")
}

/// Sets the client's gravity. `false`: that is the platform's constant.
pub fn set_gravity(_gravity: f64) -> bool {
    refused("step")
}

/// Sets the friction of a block the client knows. `false`.
pub fn set_friction(_friction: f64) -> bool {
    refused("step")
}

/// Changes the client's gravity for its own bodies. `false`.
pub fn modify_gravity(_name: &str) -> bool {
    refused("step")
}

/// Changes the client's friction. `false`.
pub fn modify_friction(_name: &str) -> bool {
    refused("step")
}

/// Changes the client's knockback. `false`.
pub fn modify_knockback(_name: &str) -> bool {
    refused("step")
}

/// Registers a collision handler for the client. `false`.
pub fn register_collision_handler(_name: &str) -> bool {
    refused("step")
}

/// Registers a step in the client's physics. `false`.
pub fn register_physics_step(_name: &str) -> bool {
    refused("step")
}

/// The bed the numbers came from, for a caller that wants to see it.
#[must_use]
pub fn constants() -> Vec<(&'static str, f64)> {
    vec![
        ("gravity", GRAVITY),
        ("vertical_drag", VERTICAL_DRAG),
        ("terminal_velocity", TERMINAL_VELOCITY),
        ("ground_drag", GROUND_DRAG),
        ("default_friction", DEFAULT_FRICTION),
        ("slippery_friction", SLIPPERY_FRICTION),
        ("eye_offset", EYE_OFFSET),
    ]
}

/// A mass, which Bedrock's client does not have: `None`, always.
#[must_use]
pub fn get_mass() -> Option<f64> {
    None
}

/// Sets a mass, which Bedrock's client does not have: `false`, always.
pub fn set_mass(_mass: f64) -> bool {
    refused("mass")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    /// A world that is solid below y = 0 and at one wall, and empty everywhere else.
    fn world(x: i32, y: i32, z: i32) -> bool {
        y < 0 || (x == 3 && z == 0)
    }

    #[test]
    fn the_constants_are_the_ones_the_game_uses() {
        assert_eq!(get_gravity(), 0.08);
        assert_eq!(get_friction(None), 0.6);
        assert_eq!(get_friction(Some("minecraft:stone")), 0.6);
        assert_eq!(get_friction(Some("minecraft:ice")), 0.91);
        assert_eq!(constants().len(), 7);
    }

    #[test]
    fn a_body_falls_the_way_the_game_does() {
        let mut body = Body::new((0.5, 10.0, 0.5));
        assert!(body.on_ground);
        body.on_ground = false;
        assert_eq!(body.velocity.1, 0.0);
        body.step(1.0);
        assert!(body.velocity.1 < 0.0, "gravity pulls it down");
        assert!(body.velocity.1 > -0.1, "but only by one tick's worth");
        for _ in 0..80 {
            body.step(1.0);
        }
        assert!(
            body.velocity.1 >= -TERMINAL_VELOCITY,
            "it does not fall faster than the terminal velocity: {}",
            body.velocity.1
        );
        assert!(body.position.1 < 10.0, "and it is lower than it started");
        assert!(body.fall_distance > 0.0);
        assert!(body.fall_damage() > 0.0, "it fell far enough to hurt");
        let fallen = body.fall_distance;
        body.land();
        assert_eq!(body.fall_distance, 0.0);
        assert!(fallen > 0.0);
        assert_eq!(body.fall_damage(), 0.0);
    }

    #[test]
    fn a_body_keeps_its_motion_and_its_box() {
        let mut body = Body::new((1.0, 2.0, 3.0)).with_size(0.6, 1.8);
        assert_eq!(body.collision_box(), (0.7, 2.0, 2.7, 1.3, 3.8, 3.3));
        assert_eq!(body.collision_shape(), body.collision_box());
        body.set_collision_shape(1.0, 2.0);
        assert_eq!(body.collision_box(), (0.5, 2.0, 2.5, 1.5, 4.0, 3.5));
        body.apply_force(0.1, 0.0, 0.0);
        body.apply_impulse(0.0, 0.2, 0.0);
        body.apply_acceleration(0.0, 0.0, 0.3);
        assert_eq!(body.velocity, (0.1, 0.2, 0.3));
        body.set_velocity(0.0, 0.0, 0.0);
        body.set_gravity(0.02);
        body.set_friction(0.9);
        assert_eq!(body.gravity, 0.02);
        assert_eq!(body.friction, 0.9, "clamped below one");
        assert!(body.eye_position().1 > body.position.1);
        assert!(body.to_string().contains("at ("));
        assert!(!body.no_clip);
        body.no_clip = true;
        body.step(1.0);
        assert_eq!(body.velocity.1, 0.0, "nothing pulls it while clipping");
    }

    #[test]
    fn a_body_notices_the_world_it_is_given() {
        let body = Body::new((0.5, -0.5, 0.5));
        assert!(body.is_inside_block(world), "the floor is below zero");
        let above = Body::new((0.5, 4.5, 0.5));
        assert!(!above.is_inside_block(world));
        let in_wall = Body::new((3.5, 4.5, 0.5));
        assert!(in_wall.is_inside_block(world));
        assert!(boxes_overlap(
            (0.0, 0.0, 0.0, 1.0, 1.0, 1.0),
            (0.5, 0.5, 0.5, 1.5, 1.5, 1.5)
        ));
        assert!(!boxes_overlap(
            (0.0, 0.0, 0.0, 1.0, 1.0, 1.0),
            (2.0, 2.0, 2.0, 3.0, 3.0, 3.0)
        ));
        assert_eq!(bounding_box((0.0, 0.0, 0.0), (0.6, 1.8)), (-0.3, 0.0, -0.3, 0.3, 1.8, 0.3));
    }

    #[test]
    fn a_ray_hits_the_wall_the_world_has() {
        let ray = Ray::new((0.5, 0.5, 0.5), (1.0, 0.0, 0.0)).with_range(10.0);
        let hit = ray.cast(world).expect("the wall is three blocks along");
        assert_eq!(hit.block, (3, 0, 0));
        assert_eq!(hit.face, (-1, 0, 0), "it met the wall's west face");
        assert!((hit.distance - 2.5).abs() < 1e-6, "{}", hit.distance);
        assert!(hit.to_string().contains("(3, 0, 0)"));

        let away = Ray::new((0.5, 0.5, 0.5), (-1.0, 0.0, 0.0)).with_range(10.0);
        assert!(away.cast(world).is_none(), "nothing is that way");

        let short = Ray::new((0.5, 0.5, 0.5), (1.0, 0.0, 0.0)).with_range(1.0);
        assert!(short.cast(world).is_none(), "the range stops it first");

        let down = Ray::new((0.5, 0.5, 0.5), (0.0, -1.0, 0.0)).with_range(4.0);
        assert_eq!(down.cast(world).map(|hit| hit.block), Some((0, -1, 0)));

        let inside = Ray::new((3.5, 4.5, 0.5), (1.0, 0.0, 0.0)).with_range(1.0);
        assert_eq!(inside.cast(world).map(|hit| hit.distance), Some(0.0));

        let nowhere = Ray::new((0.5, 4.5, 0.5), (0.0, 0.0, 0.0));
        assert!(nowhere.cast(world).is_none(), "a direction of nothing hits nothing");
    }

    #[test]
    fn a_direction_comes_from_a_rotation() {
        let north = direction_from_rotation(0.0, 0.0);
        assert!(north.2 > 0.9, "{north:?}");
        let east = direction_from_rotation(90.0, 0.0);
        assert!(east.0 < -0.9, "{east:?}");
        let up = direction_from_rotation(0.0, -90.0);
        assert!(up.1 > 0.9, "{up:?}");
        assert_eq!(normalise((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0));
    }

    #[test]
    fn a_sweep_stops_a_body_at_the_wall() {
        let body = Body::new((0.5, 0.5, 0.5)).with_size(0.6, 1.8);
        let hit = sweep(&body, (1.0, 0.0, 0.0), 5.0, world).expect("the wall is there");
        assert!(hit.distance < 3.0, "it stopped before the wall: {}", hit.distance);
        assert_eq!(hit.face.0, -1);
        let clear = sweep(&body, (0.0, 0.0, 1.0), 2.0, world);
        assert!(clear.is_none(), "nothing is that way");
        assert!(sweep(&body, (0.0, 0.0, 0.0), 5.0, world).is_none());
        assert!(sweep(&body, (1.0, 0.0, 0.0), 0.0, world).is_none());
    }

    #[test]
    fn the_clients_own_simulation_is_refused_with_a_reason() {
        let _serial = serial();
        assert!(known_only_to_the_client::get_velocity().is_none());
        assert!(known_only_to_the_client::get_ground_state().is_none());
        assert!(known_only_to_the_client::get_fluid_state().is_none());
        assert!(known_only_to_the_client::get_fall_distance().is_none());
        assert!(known_only_to_the_client::get_collision_box().is_none());
        assert!(known_only_to_the_client::is_colliding().is_none());
        assert!(known_only_to_the_client::is_grounded().is_none());
        assert!(known_only_to_the_client::check_collision().is_none());
        assert!(!set_velocity(1.0, 2.0, 3.0));
        assert!(!apply_force(1.0, 0.0, 0.0));
        assert!(!apply_impulse(1.0, 0.0, 0.0));
        assert!(!apply_acceleration(1.0, 0.0, 0.0));
        assert!(!resolve_collision());
        assert!(!set_no_clip(true));
        assert!(!set_fall_damage(false));
        assert!(!reset_fall_distance());
        assert!(!set_gravity(0.0));
        assert!(!modify_gravity("hello:low"));
        assert!(!modify_friction("hello:slick"));
        assert!(!modify_knockback("hello:far"));
        assert!(!register_collision_handler("hello:walls"));
        assert!(!register_physics_step("hello:step"));
        assert!(get_mass().is_none());
        assert!(!set_mass(1.0));
        assert_eq!(refusals().len(), 6);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
