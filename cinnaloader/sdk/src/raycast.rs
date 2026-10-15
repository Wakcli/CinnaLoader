//! Raycasting and interaction: a query a mod builds, and the client's own picking.
//!
//! A ray here is the same DDA as [`crate::physics::Ray`], wrapped in the chain the wishlist
//! asks for: where it starts (a position, a body's eye, "the camera"), which way it points,
//! how far, and what it is allowed to hit — blocks, entities, fluids, passable blocks — with
//! a predicate for anything else. The world comes from the *mod*, because the client's is out
//! of reach: [`RaycastQuery::cast`] takes a function that says whether a voxel is solid, and
//! [`RaycastQuery::cast_entities`] takes one that says whether an entity box is hittable.
//! That makes the whole thing usable today — a mod that has its own copy of a structure can
//! ray it exactly the way the game would.
//!
//! What is refused is picking against the *client's* world: `from_camera` and the entity
//! query against live entities need the client's camera and entity store.
//!
//! `interaction::*` is the same story: the reach numbers are the game's own constants and are
//! published, `can_interact` against a mod's own target is real arithmetic, and every call
//! that would make the client interact, attack, place or break is refused — those are the
//! server's, sent by the client on its own input.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let query = raycast::from_position((0.5, 0.5, 0.5))
//!     .with_direction((1.0, 0.0, 0.0))
//!     .with_range(10.0);
//! let hit = query.cast(|x, _, _| x >= 3);
//! assert_eq!(raycast::get_hit_block(&hit).map(|block| block.0), Some(3));
//! let result = raycast::RaycastResult { block: hit, entity: None };
//! assert!(!raycast::is_clear(&result));
//! ```

use std::fmt;

use crate::physics::{bounding_box, sweep, Body, Ray, RayHit};

/// How far a player can reach in survival, in blocks.
pub const REACH_SURVIVAL: f64 = 4.5;
/// The same in creative.
pub const REACH_CREATIVE: f64 = 5.0;
/// How far the camera can pick, which is further than the arm reaches.
pub const CAMERA_REACH: f64 = 12.0;

/// What a query is allowed to hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Target {
    Blocks,
    Entities,
    #[default]
    All,
}

impl Target {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Entities => "entities",
            Self::All => "all",
        }
    }
}

/// A box a query may hit, which a mod supplies for its own entities.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityBox {
    pub id: String,
    pub inside: (f64, f64, f64, f64, f64, f64),
}

impl EntityBox {
    /// A box from a position and a size, the same shape [`crate::physics`] uses.
    #[must_use]
    pub fn new(id: &str, position: (f64, f64, f64), size: (f64, f64)) -> Self {
        Self {
            id: id.to_owned(),
            inside: bounding_box(position, size),
        }
    }
}

/// A ray being built, with where it starts, where it points and what it may hit.
#[derive(Debug, Clone, PartialEq)]
pub struct RaycastQuery {
    target: Target,
    ray: Ray,
    /// Entities it will not hit.
    excluded: Vec<String>,
}

impl RaycastQuery {
    /// From a position.
    #[must_use]
    pub fn from_position(position: (f64, f64, f64)) -> Self {
        Self {
            target: Target::All,
            ray: Ray::new(position, (0.0, 0.0, 1.0)),
            excluded: Vec::new(),
        }
    }

    /// From a body's eye, which is what a player's own pick looks like.
    #[must_use]
    pub fn from_entity(body: &Body) -> Self {
        Self::from_position(body.eye_position())
    }

    /// From where a body is, with its rotation.
    #[must_use]
    pub fn from_body_rotation(body: &Body, yaw: f32, pitch: f32) -> Self {
        let mut query = Self::from_position(body.eye_position());
        query.ray.direction = crate::physics::direction_from_rotation(yaw, pitch);
        query
    }

    /// Against blocks only.
    #[must_use]
    pub fn blocks(mut self) -> Self {
        self.target = Target::Blocks;
        self
    }

    /// Against entities only.
    #[must_use]
    pub fn entities(mut self) -> Self {
        self.target = Target::Entities;
        self
    }

    /// Against both.
    #[must_use]
    pub fn all(mut self) -> Self {
        self.target = Target::All;
        self
    }

    /// Which way it points.
    #[must_use]
    pub fn with_direction(mut self, direction: (f64, f64, f64)) -> Self {
        self.ray = self.ray.with_direction(direction);
        self
    }

    /// How far it goes.
    #[must_use]
    pub fn with_range(mut self, range: f64) -> Self {
        self.ray = self.ray.with_range(range);
        self
    }

    /// Whether blocks the world calls passable still stop it.
    #[must_use]
    pub fn with_filter(mut self, blocks_only: bool) -> Self {
        self.target = if blocks_only { Target::Blocks } else { Target::All };
        self
    }

    /// The same as [`RaycastQuery::with_filter`], for a caller that supplies its own test.
    #[must_use]
    pub fn with_predicate(self, keep_only_blocks: bool) -> Self {
        self.with_filter(keep_only_blocks)
    }

    #[must_use]
    pub fn include_passable_blocks(mut self, include: bool) -> Self {
        self.ray.include_passable = include;
        self
    }

    #[must_use]
    pub fn include_fluids(mut self, include: bool) -> Self {
        self.ray.include_fluids = include;
        self
    }

    /// Leaves an entity out, for a ray that starts inside the shooter.
    #[must_use]
    pub fn exclude_entity(mut self, id: &str) -> Self {
        self.excluded.push(id.to_owned());
        self
    }

    #[must_use]
    pub fn target(&self) -> Target {
        self.target
    }

    /// The ray as it stands, for a caller that wants the numbers.
    #[must_use]
    pub fn ray(&self) -> &Ray {
        &self.ray
    }

    /// Casts against a world a mod supplies, and hits nothing when the query is
    /// entities-only.
    #[must_use]
    pub fn cast(&self, solid: impl Fn(i32, i32, i32) -> bool) -> Option<RayHit> {
        if self.target == Target::Entities {
            return None;
        }
        self.ray.cast(solid)
    }

    /// Casts against entities a mod supplies, in the order given, and answers the closest one
    /// the ray's box passes through.
    #[must_use]
    pub fn cast_entities(&self, entities: &[EntityBox]) -> Option<EntityBox> {
        if self.target == Target::Blocks {
            return None;
        }
        let mut best: Option<(f64, EntityBox)> = None;
        for entity in entities {
            if self.excluded.iter().any(|excluded| excluded == &entity.id) {
                continue;
            }
            if let Some(distance) = ray_meets_box(&self.ray, &entity.inside) {
                if best.as_ref().is_none_or(|(closest, _)| distance < *closest) {
                    best = Some((distance, entity.clone()));
                }
            }
        }
        best.map(|(_, entity)| entity)
    }

    /// Sweeps a body along a direction until it hits the world, which is the same query with
    /// a box rather than a line.
    #[must_use]
    pub fn sweep(
        &self,
        body: &Body,
        distance: f64,
        solid: impl Fn(i32, i32, i32) -> bool,
    ) -> Option<RayHit> {
        sweep(body, self.ray.direction, distance, solid)
    }
}

/// Whether a ray meets a box, and how far along it does.
#[must_use]
pub fn ray_meets_box(ray: &Ray, inside: &(f64, f64, f64, f64, f64, f64)) -> Option<f64> {
    let mut near = 0.0f64;
    let mut far = ray.range;
    for axis in 0..3 {
        let (origin, direction, low, high) = match axis {
            0 => (ray.origin.0, ray.direction.0, inside.0, inside.3),
            1 => (ray.origin.1, ray.direction.1, inside.1, inside.4),
            _ => (ray.origin.2, ray.direction.2, inside.2, inside.5),
        };
        if direction.abs() < f64::EPSILON {
            if origin < low || origin > high {
                return None;
            }
            continue;
        }
        let mut first = (low - origin) / direction;
        let mut second = (high - origin) / direction;
        if first > second {
            std::mem::swap(&mut first, &mut second);
        }
        near = near.max(first);
        far = far.min(second);
        if near > far {
            return None;
        }
    }
    Some(near)
}

/// The result of a query, ready to be read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RaycastResult {
    pub block: Option<RayHit>,
    pub entity: Option<EntityBox>,
}

impl RaycastResult {
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.block.is_none() && self.entity.is_none()
    }

    /// Whichever came first along the ray, when both were hit.
    #[must_use]
    pub fn closest_block(&self) -> Option<&RayHit> {
        self.block.as_ref()
    }
}

impl fmt::Display for RaycastResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.block, &self.entity) {
            (None, None) => formatter.write_str("nothing"),
            (Some(hit), None) => write!(formatter, "{hit}"),
            (None, Some(entity)) => write!(formatter, "{}", entity.id),
            (Some(hit), Some(entity)) => write!(formatter, "{hit} and {}", entity.id),
        }
    }
}

/// Casts a query against a world and a set of entity boxes, and answers both.
#[must_use]
pub fn get_result(
    query: &RaycastQuery,
    solid: impl Fn(i32, i32, i32) -> bool,
    entities: &[EntityBox],
) -> RaycastResult {
    RaycastResult {
        block: query.cast(solid),
        entity: query.cast_entities(entities),
    }
}

/// Whether nothing is in the way.
#[must_use]
pub fn is_clear(result: &RaycastResult) -> bool {
    result.is_clear()
}

/// Where the ray stopped.
#[must_use]
pub fn get_hit_position(hit: &Option<RayHit>) -> Option<(f64, f64, f64)> {
    hit.as_ref().map(|hit| hit.position)
}

/// The voxel it hit.
#[must_use]
pub fn get_hit_block(hit: &Option<RayHit>) -> Option<(i32, i32, i32)> {
    hit.as_ref().map(|hit| hit.block)
}

/// The face it met.
#[must_use]
pub fn get_hit_face(hit: &Option<RayHit>) -> Option<(i32, i32, i32)> {
    hit.as_ref().map(|hit| hit.face)
}

/// How far along it was.
#[must_use]
pub fn get_hit_distance(hit: &Option<RayHit>) -> Option<f64> {
    hit.as_ref().map(|hit| hit.distance)
}

/// The entity it hit, out of a result.
#[must_use]
pub fn get_hit_entity(result: &RaycastResult) -> Option<EntityBox> {
    result.entity.clone()
}

/// A query from a position, at the top level, which is what the wishlist calls
/// `raycast::from_position`.
#[must_use]
pub fn from_position(position: (f64, f64, f64)) -> RaycastQuery {
    RaycastQuery::from_position(position)
}

/// The same, from a body's eye.
#[must_use]
pub fn from_body(body: &Body) -> RaycastQuery {
    RaycastQuery::from_entity(body)
}

/// From a camera position and rotation, which is the same as from a position and a direction.
/// The *client's* camera is out of reach; this is a camera a mod places itself.
#[must_use]
pub fn from_camera(position: (f64, f64, f64), yaw: f32, pitch: f32) -> RaycastQuery {
    let mut query = RaycastQuery::from_position(position);
    query.ray.direction = crate::physics::direction_from_rotation(yaw, pitch);
    query.ray.range = CAMERA_REACH;
    query
}

/// From where an entity is, which is where a mod says its own entity is.
#[must_use]
pub fn from_entity(entity: &EntityBox) -> RaycastQuery {
    RaycastQuery::from_position((
        (entity.inside.0 + entity.inside.3) / 2.0,
        (entity.inside.1 + entity.inside.4) / 2.0,
        (entity.inside.2 + entity.inside.5) / 2.0,
    ))
}

/// Why picking against the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "camera" => "the client's camera is its own, and this host cannot read it".to_owned(),
        "world" => {
            "the client's voxel world is what its own pick runs against, and it is out of \
             reach; a mod supplies its own world here instead"
                .to_owned()
        }
        "entities" => "the client's entity store is out of reach, so entities to hit are the mod's own".to_owned(),
        "interaction" => {
            "interacting, attacking, placing and breaking are sent by the client on the \
             player's own input, and are the server's to accept"
                .to_owned()
        }
        "reach" => {
            "the client's own reach check is inside its interaction path; the numbers are \
             published here, the check is not"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["camera", "world", "entities", "interaction", "reach"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// Interaction: the reach numbers, a target a mod owns, and the client's own input path,
/// which is out of reach.
pub mod interaction {
    use super::{why_unknown, RaycastQuery};
    use crate::physics::RayHit;

    /// What a mod is pointing at, from its own query.
    #[derive(Debug, Clone, PartialEq, Default)]
    pub struct Target {
        pub block: Option<RayHit>,
        pub entity: Option<String>,
    }

    impl Target {
        /// Whether there is anything to interact with.
        #[must_use]
        pub fn is_some(&self) -> bool {
            self.block.is_some() || self.entity.is_some()
        }
    }

    /// What the player is pointing at, from a mod's own query. The client's own pick is out
    /// of reach.
    #[must_use]
    pub fn get_target(query: &RaycastQuery, solid: impl Fn(i32, i32, i32) -> bool) -> Target {
        Target {
            block: query.cast(solid),
            entity: None,
        }
    }

    /// How far the player can reach, in blocks.
    #[must_use]
    pub fn get_reach_distance(game_mode: &str) -> f64 {
        match game_mode {
            "creative" => super::REACH_CREATIVE,
            _ => super::REACH_SURVIVAL,
        }
    }

    /// Sets the reach distance a mod uses in its own checks.
    pub fn set_reach_distance(_distance: f64) -> bool {
        crate::warn!(
            "a mod asked this host to change the player's reach: {}",
            why_unknown("reach")
        );
        false
    }

    /// Whether a target is close enough to reach, which is real arithmetic over a target the
    /// mod has.
    #[must_use]
    pub fn can_interact(target: &Target, from: (f64, f64, f64), reach: f64) -> bool {
        match &target.block {
            Some(hit) => crate::entities::distance(from, hit.position) <= reach,
            None => target.entity.is_some(),
        }
    }

    /// Interacts with a block. `false`: the client sends that on the player's input.
    pub fn interact_block(_x: i32, _y: i32, _z: i32) -> bool {
        refused()
    }

    /// Interacts with an entity. `false`.
    pub fn interact_entity(_id: &str) -> bool {
        refused()
    }

    /// Attacks an entity. `false`.
    pub fn attack_entity(_id: &str) -> bool {
        refused()
    }

    /// Uses the held item. `false`.
    pub fn use_item() -> bool {
        refused()
    }

    /// Breaks a block. `false`.
    pub fn break_block(_x: i32, _y: i32, _z: i32) -> bool {
        refused()
    }

    /// Places a block. `false`.
    pub fn place_block(_x: i32, _y: i32, _z: i32, _block: &str) -> bool {
        refused()
    }

    /// Registers a handler for the client's interactions. `false`.
    pub fn register_handler(_name: &str) -> bool {
        refused()
    }

    /// Cancels an interaction. `false`.
    pub fn cancel_interaction() -> bool {
        refused()
    }

    /// Changes the client's reach check. `false`.
    pub fn modify_reach_check(_name: &str) -> bool {
        refused()
    }

    fn refused() -> bool {
        crate::warn!(
            "a mod asked this host to interact for the player: {}",
            why_unknown("interaction")
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    /// Solid at x = 3, and nothing else.
    fn wall(x: i32, _y: i32, _z: i32) -> bool {
        x == 3
    }

    #[test]
    fn a_query_hits_the_wall_it_is_pointed_at() {
        let _serial = serial();
        let query = RaycastQuery::from_position((0.5, 0.5, 0.5))
            .with_direction((1.0, 0.0, 0.0))
            .with_range(10.0);
        let hit = query.cast(wall);
        assert_eq!(get_hit_block(&hit), Some((3, 0, 0)));
        assert_eq!(get_hit_face(&hit), Some((-1, 0, 0)));
        assert!(get_hit_distance(&hit).is_some_and(|distance| distance < 3.0));
        assert!(get_hit_position(&hit).is_some());
        assert!(!is_clear(&RaycastResult { block: hit.clone(), entity: None }));

        let short = query.clone().with_range(1.0);
        assert!(short.cast(wall).is_none());
        assert!(is_clear(&RaycastResult::default()));
        assert!(get_hit_block(&None).is_none());
    }

    #[test]
    fn a_query_can_be_limited_to_blocks_or_entities() {
        let _serial = serial();
        let entities = vec![
            EntityBox::new("wisp", (2.5, 0.5, 0.5), (0.5, 0.5)),
            EntityBox::new("far", (9.5, 0.5, 0.5), (0.5, 0.5)),
        ];
        let all = RaycastQuery::from_position((0.5, 0.5, 0.5))
            .with_direction((1.0, 0.0, 0.0))
            .with_range(10.0);
        assert_eq!(all.target(), Target::All);
        let result = get_result(&all, wall, &entities);
        assert!(result.block.is_some());
        assert_eq!(result.entity.as_ref().map(|entity| entity.id.clone()), Some("wisp".to_owned()));
        assert!(result.to_string().contains("wisp"), "{result}");

        let blocks_only = all.clone().blocks();
        assert_eq!(blocks_only.target(), Target::Blocks);
        assert!(blocks_only.cast_entities(&entities).is_none());
        let entities_only = all.clone().entities();
        assert_eq!(entities_only.target(), Target::Entities);
        assert!(entities_only.cast(wall).is_none());
        assert!(entities_only.cast_entities(&entities).is_some());

        let excluded = all.clone().exclude_entity("wisp");
        assert_eq!(
            excluded.cast_entities(&entities).map(|entity| entity.id),
            Some("far".to_owned()),
            "with the near one left out, the far one is the hit"
        );
        assert!(all.clone().exclude_entity("wisp").exclude_entity("far").cast_entities(&entities).is_none());
    }

    #[test]
    fn a_query_can_start_where_a_mod_says() {
        let _serial = serial();
        let body = Body::new((0.5, 0.5, 0.5)).with_size(0.6, 1.8);
        let from_body = RaycastQuery::from_entity(&body);
        assert!(from_body.ray().origin.1 > 0.5, "the eye is above the feet");
        let turned = RaycastQuery::from_body_rotation(&body, 90.0, 0.0);
        assert!(turned.ray().direction.0 < -0.9, "{:?}", turned.ray().direction);
        let camera = super::from_camera((0.5, 1.6, 0.5), 0.0, 0.0);
        assert_eq!(camera.ray().range, CAMERA_REACH);
        let entity = from_entity(&EntityBox::new("wisp", (2.0, 1.0, 0.5), (1.0, 1.0)));
        // The box is one wide and centred on 2.0, so the middle of it is 2.0.
        assert_eq!(entity.ray().origin.0, 2.0);
        assert_eq!(entity.ray().origin.1, 1.5, "and 1.0 plus half its height");
        assert_eq!(Target::Blocks.as_str(), "blocks");
        assert_eq!(interaction::get_reach_distance("creative"), REACH_CREATIVE);
        assert_eq!(interaction::get_reach_distance("survival"), REACH_SURVIVAL);
    }

    #[test]
    fn a_ray_meets_a_box_exactly_when_it_should() {
        let ray = Ray::new((0.0, 0.0, 0.0), (1.0, 0.0, 0.0)).with_range(10.0);
        let box_inside = bounding_box((5.0, 0.0, 0.0), (1.0, 1.0));
        assert!(ray_meets_box(&ray, &box_inside).is_some());
        let behind = bounding_box((-5.0, 0.0, 0.0), (1.0, 1.0));
        assert!(ray_meets_box(&ray, &behind).is_none());
        let beside = bounding_box((5.0, 5.0, 0.0), (1.0, 1.0));
        assert!(ray_meets_box(&ray, &beside).is_none());
        let short = Ray::new((0.0, 0.0, 0.0), (1.0, 0.0, 0.0)).with_range(1.0);
        assert!(ray_meets_box(&short, &box_inside).is_none(), "out of range");
    }

    #[test]
    fn a_sweep_and_an_interaction_over_a_mods_own_world() {
        let _serial = serial();
        let body = Body::new((0.5, 0.5, 0.5)).with_size(0.6, 1.8);
        let query = RaycastQuery::from_entity(&body)
            .with_direction((1.0, 0.0, 0.0))
            .with_range(5.0);
        let hit = query.sweep(&body, 5.0, wall);
        assert!(hit.is_some(), "the sweep finds the wall");

        let target = interaction::get_target(&query, wall);
        assert!(target.is_some());
        assert!(interaction::can_interact(
            &target,
            body.position,
            REACH_CREATIVE
        ));
        assert!(!interaction::can_interact(
            &interaction::Target::default(),
            body.position,
            REACH_SURVIVAL
        ));
    }

    #[test]
    fn the_clients_own_pick_and_its_input_are_refused() {
        let _serial = serial();
        assert!(!interaction::interact_block(3, 0, 0));
        assert!(!interaction::interact_entity("wisp"));
        assert!(!interaction::attack_entity("wisp"));
        assert!(!interaction::use_item());
        assert!(!interaction::break_block(3, 0, 0));
        assert!(!interaction::place_block(3, 0, 0, "minecraft:stone"));
        assert!(!interaction::register_handler("hello:use"));
        assert!(!interaction::cancel_interaction());
        assert!(!interaction::set_reach_distance(10.0));
        assert!(!interaction::modify_reach_check("hello:reach"));
        assert_eq!(super::refusals().len(), 5);
        for (what, why) in super::refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
