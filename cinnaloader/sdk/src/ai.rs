//! AI and mob behavior: the decision layer a mod owns, and the client's own AI, which is out
//! of reach.
//!
//! The client runs the mobs — its brains, its navigators and its senses live inside its world,
//! and reading them, replacing them or hooking their step is not something this host can do.
//! What a mod *can* own is the decision layer above that, and there is a real one here:
//!
//! * **Goals and behaviours.** A [`Goal`] carries a name, a priority, the conditions under
//!   which it applies and a [`TargetFilter`]. Goals are held per entity, ordered by priority
//!   with a documented tie-break, and a declared [`Behaviour`] is a state machine with named
//!   states and event-driven [`Transition`]s that can be started and stepped.
//! * **A selection loop.** [`select_target`] picks the nearest candidate a filter accepts,
//!   within its radius and, when asked, in line of sight; [`select_goal`] walks a goal list
//!   and answers which goal would run, on what, and why the others did not.
//! * **Navigation.** A [`Path`] is a list of points with a real total distance, a [`Navigator`]
//!   steps toward the next waypoint by `speed * dt` and advances on arrival, a
//!   [`StuckDetector`] fires when progress over a window stays below a threshold, and
//!   [`can_reach`] answers against a radius.
//! * **Mob memory and tracking.** A per-entity memory, and a [`MobTarget`] with a cooldown
//!   that ticks down and expires and a last-known position that survives losing sight.
//!
//! Refused, each with its own reason in [`why_unknown`]: reading the client's brain or current
//! goal, its navigator and the path it walks, its senses, anything that needs its world
//! ([`can_see`], [`can_hear`], [`get_nearby_entities`]), spawning or despawning a mob, and the
//! hooks that would drive its AI ([`register_brain_provider`], [`register_ai_step`] and the
//! three `modify_*`).
//!
//! ```
//! use cinnaloader::ai::{self, Candidate, Goal, TargetFilter};
//!
//! let chase = Goal::new("chase_sheep", 1)
//!     .condition("hungry")
//!     .filter(TargetFilter::new(8.0).allow("sheep"));
//! let sheep = Candidate::new("sheep-1", "sheep", (3.0, 0.0, 0.0));
//!
//! let chosen = ai::select_goal(&[chase], (0.0, 0.0, 0.0), &[sheep], &["hungry".to_owned()]);
//! assert_eq!(chosen.goal.as_deref(), Some("chase_sheep"));
//! assert_eq!(chosen.target.expect("a target").id, "sheep-1");
//! assert!(ai::get_brain().is_none(), "the client's brain is out of reach");
//! ```

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::{Mutex, OnceLock};

use crate::entities::distance;

/// How long a fresh [`MobTarget`] is held without being refreshed.
pub const DEFAULT_TARGET_COOLDOWN: u32 = 100;

/// A goal a mod declares: a name, a priority, the conditions under which it applies and the
/// target filter that decides what it acts on.
///
/// Lower priorities run first, which is the game's own convention, and two goals with the same
/// priority keep the order they were added in, because the list is kept with a stable sort.
#[derive(Debug, Clone, PartialEq)]
pub struct Goal {
    pub name: String,
    pub priority: i32,
    pub conditions: Vec<String>,
    pub filter: TargetFilter,
}

impl Goal {
    #[must_use]
    pub fn new(name: &str, priority: i32) -> Self {
        Self {
            name: name.to_owned(),
            priority,
            conditions: Vec::new(),
            filter: TargetFilter::default(),
        }
    }

    #[must_use]
    pub fn condition(mut self, condition: &str) -> Self {
        self.conditions.push(condition.to_owned());
        self
    }

    #[must_use]
    pub fn filter(mut self, filter: TargetFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Whether every condition this goal needs is active.
    #[must_use]
    pub fn applies(&self, active: &[String]) -> bool {
        self.conditions.iter().all(|condition| active.contains(condition))
    }

    /// The conditions that are not active, for a report.
    #[must_use]
    pub fn missing(&self, active: &[String]) -> Vec<String> {
        self.conditions
            .iter()
            .filter(|condition| !active.contains(condition))
            .cloned()
            .collect()
    }
}

impl fmt::Display for Goal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({}, {} conditions)", self.name, self.priority, self.conditions.len())
    }
}

/// Which entities a goal will act on: an allow list, a deny list, a radius and whether the
/// target has to be in line of sight. An empty allow list means every kind, and the deny list
/// always wins.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TargetFilter {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub radius: f64,
    pub line_of_sight: bool,
}

impl TargetFilter {
    #[must_use]
    pub fn new(radius: f64) -> Self {
        Self {
            radius: radius.max(0.0),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn allow(mut self, kind: &str) -> Self {
        self.allow.push(kind.to_owned());
        self
    }

    #[must_use]
    pub fn deny(mut self, kind: &str) -> Self {
        self.deny.push(kind.to_owned());
        self
    }

    #[must_use]
    pub fn line_of_sight(mut self, required: bool) -> Self {
        self.line_of_sight = required;
        self
    }

    #[must_use]
    pub fn accepts(&self, kind: &str) -> bool {
        if self.deny.iter().any(|denied| denied == kind) {
            return false;
        }
        self.allow.is_empty() || self.allow.iter().any(|allowed| allowed == kind)
    }
}

/// An entity the selection loop may choose, with where it is and whether the observer can see
/// it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub kind: String,
    pub position: (f64, f64, f64),
    pub visible: bool,
}

impl Candidate {
    #[must_use]
    pub fn new(id: &str, kind: &str, position: (f64, f64, f64)) -> Self {
        Self {
            id: id.to_owned(),
            kind: kind.to_owned(),
            position,
            visible: true,
        }
    }

    #[must_use]
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }
}

/// The nearest candidate a filter accepts, within its radius and, when the filter asks for it,
/// in line of sight. A tie goes to whichever candidate comes first in the list.
#[must_use]
pub fn select_target(
    origin: (f64, f64, f64),
    candidates: &[Candidate],
    filter: &TargetFilter,
) -> Option<Candidate> {
    let mut best: Option<(f64, &Candidate)> = None;
    for candidate in candidates {
        if !filter.accepts(&candidate.kind) {
            continue;
        }
        if filter.line_of_sight && !candidate.visible {
            continue;
        }
        let range = distance(origin, candidate.position);
        if range > filter.radius {
            continue;
        }
        let closer = match &best {
            Some((closest, _)) => range < *closest,
            None => true,
        };
        if closer {
            best = Some((range, candidate));
        }
    }
    best.map(|(_, candidate)| candidate.clone())
}

/// Which goal would run, on what target, and why the others did not.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GoalSelection {
    pub goal: Option<String>,
    pub target: Option<Candidate>,
    pub skipped: Vec<(String, String)>,
}

impl GoalSelection {
    /// Why a particular goal did not run, if it did not.
    #[must_use]
    pub fn reason_for(&self, goal: &str) -> Option<&str> {
        self.skipped
            .iter()
            .find(|(name, _)| name == goal)
            .map(|(_, why)| why.as_str())
    }
}

/// Walks a goal list against a set of candidates: the first goal, in the order given, whose
/// conditions are all active and whose filter finds a target is the one that runs, and every
/// other goal is reported skipped with its reason rather than stalling the list.
#[must_use]
pub fn select_goal(
    goals: &[Goal],
    origin: (f64, f64, f64),
    candidates: &[Candidate],
    active: &[String],
) -> GoalSelection {
    let mut skipped = Vec::new();
    let mut chosen: Option<(String, Candidate)> = None;
    for goal in goals {
        if let Some((running, _)) = &chosen {
            skipped.push((goal.name.clone(), format!("{running} is running first")));
            continue;
        }
        let missing = goal.missing(active);
        if !missing.is_empty() {
            let missing = missing.join(", ");
            skipped.push((goal.name.clone(), format!("conditions not met: {missing}")));
            continue;
        }
        match select_target(origin, candidates, &goal.filter) {
            Some(target) => chosen = Some((goal.name.clone(), target)),
            None => skipped.push((goal.name.clone(), "no target within radius".to_owned())),
        }
    }
    GoalSelection {
        goal: chosen.as_ref().map(|(name, _)| name.clone()),
        target: chosen.map(|(_, target)| target),
        skipped,
    }
}

/// A path: a list of points, walked in order, with the distance a body would cover along it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub points: Vec<(f64, f64, f64)>,
}

impl Path {
    #[must_use]
    pub fn new(points: Vec<(f64, f64, f64)>) -> Self {
        Self { points }
    }

    #[must_use]
    pub fn push(mut self, point: (f64, f64, f64)) -> Self {
        self.points.push(point);
        self
    }

    /// The total distance along the path. Folded from a positive zero, so an empty path totals
    /// `0.0` rather than the `-0.0` a plain float `sum()` would answer.
    #[must_use]
    pub fn length(&self) -> f64 {
        self.points
            .windows(2)
            .fold(0.0, |total, step| total + distance(step[0], step[1]))
    }

    #[must_use]
    pub fn end(&self) -> Option<(f64, f64, f64)> {
        self.points.last().copied()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Whether the whole path fits inside a radius.
    #[must_use]
    pub fn is_reachable(&self, radius: f64) -> bool {
        self.length() <= radius
    }
}

impl fmt::Display for Path {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} points, {:.2} long", self.points.len(), self.length())
    }
}

/// Fires when the distance moved over a window of steps stays below a threshold: a body pushing
/// against a wall.
#[derive(Debug, Clone, PartialEq)]
pub struct StuckDetector {
    pub window: usize,
    pub threshold: f64,
    history: VecDeque<f64>,
}

impl StuckDetector {
    #[must_use]
    pub fn new(window: usize, threshold: f64) -> Self {
        Self {
            window: window.max(1),
            threshold: threshold.max(0.0),
            history: VecDeque::new(),
        }
    }

    pub fn record(&mut self, moved: f64) {
        self.history.push_back(moved.max(0.0));
        while self.history.len() > self.window {
            self.history.pop_front();
        }
    }

    /// Whether the last full window of steps moved less than the threshold in total.
    #[must_use]
    pub fn is_stuck(&self) -> bool {
        self.history.len() >= self.window
            && self.history.iter().fold(0.0, |total, moved| total + moved) < self.threshold
    }

    pub fn reset(&mut self) {
        self.history.clear();
    }
}

/// One tick of navigation.
#[derive(Debug, Clone, PartialEq)]
pub struct NavStep {
    pub from: (f64, f64, f64),
    pub to: (f64, f64, f64),
    pub moved: f64,
    pub advanced: bool,
    pub finished: bool,
}

/// A body walking a path at a speed: the mod's own pathfinder output, driven one step at a
/// time. The client's navigator is refused; this one is a mod's.
#[derive(Debug, Clone, PartialEq)]
pub struct Navigator {
    pub position: (f64, f64, f64),
    pub path: Path,
    pub waypoint: usize,
    pub speed: f64,
    pub stuck: StuckDetector,
}

impl Navigator {
    /// A body at `from`, walking `path` at `speed`, starting at the first point that is not
    /// where it already is.
    #[must_use]
    pub fn at(from: (f64, f64, f64), path: Path, speed: f64) -> Self {
        let waypoint = path.points.iter().position(|point| *point != from).unwrap_or(0);
        Self {
            position: from,
            path,
            waypoint,
            speed: speed.max(0.0),
            stuck: StuckDetector::new(20, 0.05),
        }
    }

    /// Moves by `speed * dt` toward the current waypoint, snapping to it and advancing when the
    /// budget reaches it.
    pub fn step(&mut self, dt: f64) -> NavStep {
        let from = self.position;
        let budget = (self.speed * dt).max(0.0);
        let Some(target) = self.path.points.get(self.waypoint).copied() else {
            self.stuck.record(0.0);
            return NavStep {
                from,
                to: from,
                moved: 0.0,
                advanced: false,
                finished: true,
            };
        };
        let remaining = distance(from, target);
        if remaining <= budget {
            self.position = target;
            self.waypoint += 1;
            self.stuck.record(remaining);
            NavStep {
                from,
                to: target,
                moved: remaining,
                advanced: true,
                finished: self.waypoint >= self.path.points.len(),
            }
        } else {
            let ratio = budget / remaining;
            self.position = (
                from.0 + (target.0 - from.0) * ratio,
                from.1 + (target.1 - from.1) * ratio,
                from.2 + (target.2 - from.2) * ratio,
            );
            self.stuck.record(budget);
            NavStep {
                from,
                to: self.position,
                moved: budget,
                advanced: false,
                finished: false,
            }
        }
    }

    /// Whether this body's progress over the stuck window is below its threshold.
    #[must_use]
    pub fn is_stuck(&self) -> bool {
        self.stuck.is_stuck()
    }

    /// Whether a point is within a radius of where the body is now.
    #[must_use]
    pub fn can_reach(&self, point: (f64, f64, f64), radius: f64) -> bool {
        distance(self.position, point) <= radius
    }
}

/// A mob's current target and how long it survives without being refreshed.
#[derive(Debug, Clone, PartialEq)]
pub struct MobTarget {
    pub id: String,
    pub kind: String,
    pub position: (f64, f64, f64),
    pub last_known: (f64, f64, f64),
    pub cooldown: u32,
    pub seen: bool,
}

impl MobTarget {
    #[must_use]
    pub fn new(id: &str, kind: &str, position: (f64, f64, f64)) -> Self {
        Self {
            id: id.to_owned(),
            kind: kind.to_owned(),
            position,
            last_known: position,
            cooldown: DEFAULT_TARGET_COOLDOWN,
            seen: true,
        }
    }

    #[must_use]
    pub fn with_cooldown(mut self, cooldown: u32) -> Self {
        self.cooldown = cooldown;
        self
    }

    /// Ticks the cooldown down and returns whether the target is still held.
    pub fn tick(&mut self, ticks: u32) -> bool {
        self.cooldown = self.cooldown.saturating_sub(ticks);
        self.cooldown > 0
    }

    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.cooldown == 0
    }

    /// Loses sight of the target: the live position stops being read, the last one is kept.
    pub fn lost(&mut self) {
        self.last_known = self.position;
        self.seen = false;
    }
}

/// One transition in a behaviour's state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub from: String,
    pub event: String,
    pub to: String,
}

/// A behaviour a mod declares: a state machine with named states and event-driven transitions.
/// It is the decision layer a mod runs; handing it to the client is refused.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Behaviour {
    pub name: String,
    pub initial: String,
    pub states: Vec<String>,
    pub transitions: Vec<Transition>,
}

impl Behaviour {
    #[must_use]
    pub fn new(name: &str, initial: &str) -> Self {
        Self {
            name: name.to_owned(),
            initial: initial.to_owned(),
            states: vec![initial.to_owned()],
            transitions: Vec::new(),
        }
    }

    #[must_use]
    pub fn state(mut self, name: &str) -> Self {
        if !self.states.iter().any(|state| state == name) {
            self.states.push(name.to_owned());
        }
        self
    }

    #[must_use]
    pub fn transition(mut self, from: &str, event: &str, to: &str) -> Self {
        self.transitions.push(Transition {
            from: from.to_owned(),
            event: event.to_owned(),
            to: to.to_owned(),
        });
        self
    }

    /// Where an event takes a state, if the behaviour has a transition for it.
    #[must_use]
    pub fn next_state(&self, from: &str, event: &str) -> Option<&str> {
        self.transitions
            .iter()
            .find(|transition| transition.from == from && transition.event == event)
            .map(|transition| transition.to.as_str())
    }
}

/// A behaviour, running: where it is now and how many transitions it has taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateMachine {
    pub behaviour: String,
    pub current: String,
    pub steps: u64,
}

impl StateMachine {
    #[must_use]
    pub fn start(behaviour: &Behaviour) -> Option<Self> {
        if behaviour.initial.is_empty() {
            return None;
        }
        Some(Self {
            behaviour: behaviour.name.clone(),
            current: behaviour.initial.clone(),
            steps: 0,
        })
    }

    /// Fires an event, moving to the next state when the behaviour has a transition for it.
    pub fn fire(&mut self, behaviour: &Behaviour, event: &str) -> bool {
        match behaviour.next_state(&self.current, event) {
            Some(to) => {
                self.current = to.to_owned();
                self.steps += 1;
                true
            }
            None => false,
        }
    }
}

/// A memory type a mod declares, and the kind of value it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryType {
    pub name: String,
    pub value: String,
}

impl MemoryType {
    #[must_use]
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_owned(),
            value: value.to_owned(),
        }
    }
}

/// A sensor a mod declares and the range it works over.
#[derive(Debug, Clone, PartialEq)]
pub struct Sensor {
    pub name: String,
    pub range: f64,
}

impl Sensor {
    #[must_use]
    pub fn new(name: &str, range: f64) -> Self {
        Self {
            name: name.to_owned(),
            range: range.max(0.0),
        }
    }
}

/// A schedule a mod declares: a named list of activities.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schedule {
    pub name: String,
    pub activities: Vec<String>,
}

impl Schedule {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            activities: Vec::new(),
        }
    }

    #[must_use]
    pub fn activity(mut self, activity: &str) -> Self {
        self.activities.push(activity.to_owned());
        self
    }
}

#[derive(Debug, Clone, Default)]
struct EntityAi {
    goals: Vec<Goal>,
    memories: BTreeMap<String, String>,
    target: Option<MobTarget>,
    navigator: Option<Navigator>,
}

fn entities() -> &'static Mutex<BTreeMap<String, EntityAi>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<String, EntityAi>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with<T>(body: impl FnOnce(&mut BTreeMap<String, EntityAi>) -> T) -> T {
    let mut guard = entities().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

#[derive(Debug, Clone, Default)]
struct Declarations {
    goals: BTreeMap<String, Goal>,
    behaviours: BTreeMap<String, Behaviour>,
    sensors: BTreeMap<String, Sensor>,
    memory_types: BTreeMap<String, MemoryType>,
    schedules: BTreeMap<String, Schedule>,
}

fn declarations() -> &'static Mutex<Declarations> {
    static REGISTRY: OnceLock<Mutex<Declarations>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Declarations::default()))
}

fn with_decl<T>(body: impl FnOnce(&mut Declarations) -> T) -> T {
    let mut guard = declarations().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// An entity's goals, ordered by priority and, for equal priorities, by the order they were
/// added.
#[must_use]
pub fn get_goals(entity: &str) -> Vec<Goal> {
    with(|registry| registry.get(entity).map(|ai| ai.goals.clone()).unwrap_or_default())
}

/// Adds a goal to an entity's list, creating the entity's AI state if this is its first. The
/// list stays ordered: lower priority first, equal priorities in the order they were added. A
/// name is added once.
pub fn add_goal(entity: &str, goal: Goal) -> bool {
    with(|registry| {
        let ai = registry.entry(entity.to_owned()).or_default();
        if ai.goals.iter().any(|held| held.name == goal.name) {
            return false;
        }
        ai.goals.push(goal);
        ai.goals.sort_by_key(|held| held.priority);
        true
    })
}

/// Removes a goal from an entity's list by name, keeping the order of the rest.
pub fn remove_goal(entity: &str, name: &str) -> bool {
    with(|registry| match registry.get_mut(entity) {
        Some(ai) => {
            let before = ai.goals.len();
            ai.goals.retain(|held| held.name != name);
            ai.goals.len() != before
        }
        None => false,
    })
}

/// Replaces the goal an entity already has under this name, keeping the list ordered.
pub fn replace_goal(entity: &str, goal: Goal) -> bool {
    with(|registry| match registry.get_mut(entity) {
        Some(ai) if ai.goals.iter().any(|held| held.name == goal.name) => {
            for held in &mut ai.goals {
                if held.name == goal.name {
                    *held = goal.clone();
                }
            }
            ai.goals.sort_by_key(|held| held.priority);
            true
        }
        _ => false,
    })
}

/// An entity's memory, as key and value.
#[must_use]
pub fn get_memories(entity: &str) -> BTreeMap<String, String> {
    with(|registry| registry.get(entity).map(|ai| ai.memories.clone()).unwrap_or_default())
}

/// One of an entity's memories.
#[must_use]
pub fn get_memory(entity: &str, key: &str) -> Option<String> {
    with(|registry| registry.get(entity).and_then(|ai| ai.memories.get(key).cloned()))
}

/// Sets a memory on an entity, creating its AI state if this is its first. `true` when the key
/// was new.
pub fn set_memory(entity: &str, key: &str, value: &str) -> bool {
    with(|registry| {
        registry
            .entry(entity.to_owned())
            .or_default()
            .memories
            .insert(key.to_owned(), value.to_owned())
            .is_none()
    })
}

/// Forgets one of an entity's memories.
pub fn clear_memory(entity: &str, key: &str) -> bool {
    with(|registry| match registry.get_mut(entity) {
        Some(ai) => ai.memories.remove(key).is_some(),
        None => false,
    })
}

/// An entity's tracked target.
#[must_use]
pub fn get_target(entity: &str) -> Option<MobTarget> {
    with(|registry| registry.get(entity).and_then(|ai| ai.target.clone()))
}

/// Tracks a target for an entity, so a mod can hold one and let its cooldown run.
pub fn set_target(entity: &str, target: MobTarget) -> bool {
    with(|registry| {
        registry.entry(entity.to_owned()).or_default().target = Some(target);
        true
    })
}

/// Drops an entity's tracked target.
pub fn clear_target(entity: &str) -> bool {
    with(|registry| match registry.get_mut(entity) {
        Some(ai) => ai.target.take().is_some(),
        None => false,
    })
}

/// Ticks a tracked target's cooldown and clears it once it expires. `true` while it is held.
pub fn tick_target(entity: &str, ticks: u32) -> bool {
    with(|registry| {
        let Some(ai) = registry.get_mut(entity) else {
            return false;
        };
        let Some(target) = ai.target.as_mut() else {
            return false;
        };
        let held = target.tick(ticks);
        if !held {
            ai.target = None;
        }
        held
    })
}

/// An entity's active path, or `None` when it has none. This is the mod's own path, not the
/// client's.
#[must_use]
pub fn get_path(entity: &str) -> Option<Path> {
    with(|registry| {
        registry
            .get(entity)
            .and_then(|ai| ai.navigator.as_ref())
            .filter(|navigator| !navigator.path.is_empty())
            .map(|navigator| navigator.path.clone())
    })
}

/// A path through the given points: the total distance is the sum of its segments.
#[must_use]
pub fn create_path(points: Vec<(f64, f64, f64)>) -> Path {
    Path::new(points)
}

/// Empties an entity's active path but keeps where it is. `true` if it had a path.
pub fn cancel_path(entity: &str) -> bool {
    with(|registry| match registry.get_mut(entity).and_then(|ai| ai.navigator.as_mut()) {
        Some(navigator) => {
            let had = !navigator.path.is_empty();
            navigator.path = Path::default();
            navigator.waypoint = 0;
            had
        }
        None => false,
    })
}

/// Puts an entity on a path, starting at the path's first point. An empty path is refused.
pub fn follow_path(entity: &str, path: Path) -> bool {
    let Some(start) = path.points.first().copied() else {
        return false;
    };
    with(|registry| {
        let ai = registry.entry(entity.to_owned()).or_default();
        let speed = ai
            .navigator
            .as_ref()
            .map_or(crate::movement::WALK_SPEED, |navigator| navigator.speed);
        ai.navigator = Some(Navigator::at(start, path, speed));
        true
    })
}

/// Sends an entity straight to a point at a speed, replacing any path it was on.
pub fn navigate_to(entity: &str, from: (f64, f64, f64), to: (f64, f64, f64), speed: f64) -> bool {
    if from == to {
        return false;
    }
    with(|registry| {
        let ai = registry.entry(entity.to_owned()).or_default();
        ai.navigator = Some(Navigator::at(from, Path::new(vec![from, to]), speed));
        true
    })
}

/// Stops an entity navigating by dropping its navigator.
pub fn stop_navigation(entity: &str) -> bool {
    with(|registry| match registry.get_mut(entity) {
        Some(ai) => ai.navigator.take().is_some(),
        None => false,
    })
}

/// Whether an entity is within a radius of a point.
#[must_use]
pub fn can_reach(entity: &str, point: (f64, f64, f64), radius: f64) -> bool {
    with(|registry| {
        registry
            .get(entity)
            .and_then(|ai| ai.navigator.as_ref())
            .is_some_and(|navigator| navigator.can_reach(point, radius))
    })
}

/// Declares a goal a mod ships.
pub fn register_goal(goal: Goal) -> bool {
    with_decl(|registry| registry.goals.insert(goal.name.clone(), goal).is_none())
}

/// A declared goal, by name.
#[must_use]
pub fn get_registered_goal(name: &str) -> Option<Goal> {
    with_decl(|registry| registry.goals.get(name).cloned())
}

/// Every declared goal, in name order.
#[must_use]
pub fn registered_goals() -> Vec<Goal> {
    with_decl(|registry| registry.goals.values().cloned().collect())
}

/// Declares a behaviour a mod ships.
pub fn register_behavior(behaviour: Behaviour) -> bool {
    with_decl(|registry| {
        registry
            .behaviours
            .insert(behaviour.name.clone(), behaviour)
            .is_none()
    })
}

/// A declared behaviour, by name.
#[must_use]
pub fn get_behaviour(name: &str) -> Option<Behaviour> {
    with_decl(|registry| registry.behaviours.get(name).cloned())
}

/// Every declared behaviour, in name order.
#[must_use]
pub fn registered_behaviours() -> Vec<Behaviour> {
    with_decl(|registry| registry.behaviours.values().cloned().collect())
}

/// Declares a sensor a mod ships.
pub fn register_sensor(sensor: Sensor) -> bool {
    with_decl(|registry| registry.sensors.insert(sensor.name.clone(), sensor).is_none())
}

/// A declared sensor, by name.
#[must_use]
pub fn get_sensor(name: &str) -> Option<Sensor> {
    with_decl(|registry| registry.sensors.get(name).cloned())
}

/// Every declared sensor, in name order.
#[must_use]
pub fn registered_sensors() -> Vec<Sensor> {
    with_decl(|registry| registry.sensors.values().cloned().collect())
}

/// Declares a memory type a mod ships.
pub fn register_memory_type(memory_type: MemoryType) -> bool {
    with_decl(|registry| {
        registry
            .memory_types
            .insert(memory_type.name.clone(), memory_type)
            .is_none()
    })
}

/// A declared memory type, by name.
#[must_use]
pub fn get_memory_type(name: &str) -> Option<MemoryType> {
    with_decl(|registry| registry.memory_types.get(name).cloned())
}

/// Every declared memory type, in name order.
#[must_use]
pub fn registered_memory_types() -> Vec<MemoryType> {
    with_decl(|registry| registry.memory_types.values().cloned().collect())
}

/// Declares a schedule a mod ships.
pub fn register_schedule(schedule: Schedule) -> bool {
    with_decl(|registry| {
        registry
            .schedules
            .insert(schedule.name.clone(), schedule)
            .is_none()
    })
}

/// A declared schedule, by name.
#[must_use]
pub fn get_schedule(name: &str) -> Option<Schedule> {
    with_decl(|registry| registry.schedules.get(name).cloned())
}

/// Every declared schedule, in name order.
#[must_use]
pub fn registered_schedules() -> Vec<Schedule> {
    with_decl(|registry| registry.schedules.values().cloned().collect())
}

/// Why a question about the client's mob AI has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "brain" => {
            "a mob's brain, its current goal and what it is thinking live inside the client's \
             world; reading them, or replacing the brain provider, needs a hook this host does \
             not have"
                .to_owned()
        }
        "navigation" => {
            "the client's navigator and the path it is walking are its own; a mod can build a \
             path for itself, and this host cannot read the client's"
                .to_owned()
        }
        "senses" => {
            "sight and hearing are the client's senses, resolved against a world this host \
             cannot see"
                .to_owned()
        }
        "world" => {
            "seeing, hearing and finding nearby entities need the client's world and its entity \
             store, neither of which is reachable from here"
                .to_owned()
        }
        "spawn" => {
            "spawning and despawning a mob are the server's; the client only draws what it is \
             told, and this host cannot ask it to send the request"
                .to_owned()
        }
        "hook" => {
            "these sit inside the client's AI step or replace its decision making, which is \
             where hooks have crashed this client before"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about mob AI.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["brain", "navigation", "senses", "world", "spawn", "hook"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refuse(what: &str) {
    crate::warn!("a mod asked this host about the client's mob AI: {}", why_unknown(what));
}

/// The client's brain. `None`, always: [`why_unknown`] says why.
pub type BrainHandle = ();

/// An entity's brain, which only the client has. `None`.
#[must_use]
pub fn get_brain() -> Option<BrainHandle> {
    refuse("brain");
    None
}

/// The client's navigator. `None`.
pub type NavigationHandle = ();

/// An entity's navigator, which only the client has. `None`.
#[must_use]
pub fn get_navigation() -> Option<NavigationHandle> {
    refuse("navigation");
    None
}

/// An entity's senses. Empty.
#[must_use]
pub fn get_senses() -> Vec<(String, f64)> {
    refuse("senses");
    Vec::new()
}

/// Whether one entity can see another. `false`: it needs the client's world.
pub fn can_see(_observer: &str, _target: &str) -> bool {
    refuse("world");
    false
}

/// Whether one entity can hear another. `false`: it needs the client's world.
pub fn can_hear(_observer: &str, _target: &str) -> bool {
    refuse("world");
    false
}

/// The entities near a point, which only the client's store has. Empty.
#[must_use]
pub fn get_nearby_entities(_origin: (f64, f64, f64), _radius: f64) -> Vec<Candidate> {
    refuse("world");
    Vec::new()
}

/// Registers a brain provider with the client. `false`.
pub fn register_brain_provider(_name: &str) -> bool {
    refuse("hook");
    false
}

/// Registers a step in the client's AI loop. `false`.
pub fn register_ai_step(_name: &str) -> bool {
    refuse("hook");
    false
}

/// Replaces how the client selects targets. `false`.
pub fn modify_target_selection(_name: &str) -> bool {
    refuse("hook");
    false
}

/// Replaces how the client navigates. `false`.
pub fn modify_navigation(_name: &str) -> bool {
    refuse("hook");
    false
}

/// Replaces how the client pathfinds. `false`.
pub fn modify_pathfinding(_name: &str) -> bool {
    refuse("hook");
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|registry| registry.clear());
        with_decl(|registry| *registry = Declarations::default());
    }

    #[test]
    fn the_nearest_valid_target_wins_and_a_tie_goes_to_the_first() {
        let origin = (0.0, 0.0, 0.0);
        let filter = TargetFilter::new(8.0).allow("sheep");
        let candidates = vec![
            Candidate::new("near", "sheep", (3.0, 0.0, 0.0)),
            Candidate::new("far", "sheep", (5.0, 0.0, 0.0)),
        ];
        assert_eq!(
            select_target(origin, &candidates, &filter).map(|c| c.id),
            Some("near".to_owned())
        );

        let tie = vec![
            Candidate::new("first", "sheep", (0.0, 0.0, 3.0)),
            Candidate::new("second", "sheep", (3.0, 0.0, 0.0)),
        ];
        assert_eq!(
            select_target(origin, &tie, &filter).map(|c| c.id),
            Some("first".to_owned())
        );

        let filtered = vec![
            Candidate::new("cow", "cow", (1.0, 0.0, 0.0)),
            Candidate::new("sheep", "sheep", (4.0, 0.0, 0.0)),
        ];
        assert_eq!(
            select_target(origin, &filtered, &filter).map(|c| c.id),
            Some("sheep".to_owned())
        );

        let outside = vec![Candidate::new("away", "sheep", (10.0, 0.0, 0.0))];
        assert!(select_target(origin, &outside, &filter).is_none(), "outside the radius");

        let hidden = vec![Candidate::new("hidden", "sheep", (1.0, 0.0, 0.0)).visible(false)];
        let los = TargetFilter::new(8.0).allow("sheep").line_of_sight(true);
        assert!(select_target(origin, &hidden, &los).is_none(), "not in line of sight");
        assert!(select_target(origin, &hidden, &filter).is_some(), "sight not required");
        assert!(!filter.accepts("cow"));
        assert!(TargetFilter::new(4.0).deny("cow").accepts("sheep"));
        assert!(!TargetFilter::new(4.0).deny("cow").accepts("cow"));
    }

    #[test]
    fn a_goal_that_cannot_run_is_skipped_with_a_reason_and_the_next_runs() {
        let origin = (0.0, 0.0, 0.0);
        let candidates = vec![Candidate::new("sheep", "sheep", (3.0, 0.0, 0.0))];
        let goals = vec![
            Goal::new("flee", 0).condition("hurt"),
            Goal::new("chase", 1)
                .condition("hungry")
                .filter(TargetFilter::new(8.0).allow("sheep")),
            Goal::new("wander", 2),
        ];
        let active = vec!["hungry".to_owned()];
        let chosen = select_goal(&goals, origin, &candidates, &active);
        assert_eq!(chosen.goal.as_deref(), Some("chase"));
        assert_eq!(chosen.target.as_ref().map(|c| c.id.as_str()), Some("sheep"));
        assert_eq!(chosen.reason_for("flee"), Some("conditions not met: hurt"));
        assert_eq!(chosen.reason_for("wander"), Some("chase is running first"));
        assert_eq!(chosen.skipped.len(), 2);

        let no_prey = select_goal(&goals, origin, &[], &active);
        assert_eq!(no_prey.goal, None);
        assert_eq!(no_prey.reason_for("chase"), Some("no target within radius"));
    }

    #[test]
    fn a_goal_list_stays_ordered_across_an_add_and_a_remove() {
        let _serial = serial();
        fresh();
        assert!(add_goal("wolf", Goal::new("wander", 5)));
        assert!(add_goal("wolf", Goal::new("chase", 1)));
        assert!(add_goal("wolf", Goal::new("leap", 1)));
        assert!(add_goal("wolf", Goal::new("flee", 0)));
        let names: Vec<String> = get_goals("wolf").into_iter().map(|goal| goal.name).collect();
        assert_eq!(names, vec!["flee", "chase", "leap", "wander"], "priority, then insertion");
        assert!(!add_goal("wolf", Goal::new("chase", 0)), "a name is added once");

        assert!(remove_goal("wolf", "leap"));
        let names: Vec<String> = get_goals("wolf").into_iter().map(|goal| goal.name).collect();
        assert_eq!(names, vec!["flee", "chase", "wander"]);

        assert!(replace_goal("wolf", Goal::new("wander", 0)));
        let names: Vec<String> = get_goals("wolf").into_iter().map(|goal| goal.name).collect();
        assert_eq!(names, vec!["flee", "wander", "chase"]);
        assert!(!remove_goal("wolf", "leap"));
        assert!(!replace_goal("wolf", Goal::new("nothing", 0)));
        assert_eq!(get_goals("nobody"), Vec::new());
    }

    #[test]
    fn declared_goals_behaviours_sensors_types_and_schedules_read_back() {
        let _serial = serial();
        fresh();
        assert!(register_goal(Goal::new("chase", 1)));
        assert!(!register_goal(Goal::new("chase", 1)), "a name is declared once");
        assert_eq!(get_registered_goal("chase").map(|goal| goal.priority), Some(1));
        assert_eq!(registered_goals().len(), 1);

        let hunt = Behaviour::new("hunt", "idle")
            .state("chase")
            .transition("idle", "sees_prey", "chase")
            .transition("chase", "lost_prey", "idle");
        assert!(register_behavior(hunt.clone()));
        assert_eq!(get_behaviour("hunt").map(|behaviour| behaviour.states.len()), Some(2));
        let mut machine = StateMachine::start(&hunt).expect("a behaviour starts");
        assert_eq!(machine.current, "idle");
        assert!(machine.fire(&hunt, "sees_prey"));
        assert_eq!(machine.current, "chase");
        assert!(!machine.fire(&hunt, "sees_prey"), "no transition from chase on that event");
        assert!(machine.fire(&hunt, "lost_prey"));
        assert_eq!(machine.current, "idle");
        assert_eq!(machine.steps, 2);
        assert!(StateMachine::start(&Behaviour::default()).is_none());

        assert!(register_sensor(Sensor::new("sight", 16.0)));
        assert!(register_memory_type(MemoryType::new("home", "position")));
        assert!(register_schedule(Schedule::new("day").activity("work").activity("rest")));
        assert_eq!(registered_sensors().len(), 1);
        assert_eq!(registered_memory_types().len(), 1);
        assert_eq!(registered_schedules().len(), 1);
        assert_eq!(get_sensor("sight").map(|sensor| sensor.range), Some(16.0));
        assert!(get_memory_type("home").is_some());
        assert!(get_schedule("day").is_some());

        assert!(set_memory("wolf", "home", "(0,64,0)"));
        assert_eq!(get_memory("wolf", "home"), Some("(0,64,0)".to_owned()));
        assert_eq!(get_memories("wolf").len(), 1);
        assert!(!set_memory("wolf", "home", "(0,64,0)"), "the key was already there");
        assert!(clear_memory("wolf", "home"));
        assert_eq!(get_memory("wolf", "home"), None);
        assert!(!clear_memory("wolf", "home"));
    }

    #[test]
    fn a_step_moves_by_speed_times_dt_and_advances_on_arrival() {
        let mut navigator =
            Navigator::at((0.0, 0.0, 0.0), Path::new(vec![(0.0, 0.0, 0.0), (4.0, 0.0, 0.0)]), 2.0);
        assert_eq!(navigator.waypoint, 1, "the start is not a waypoint");
        let step = navigator.step(1.0);
        assert_eq!(step.moved, 2.0, "speed * dt");
        assert_eq!(step.to, (2.0, 0.0, 0.0));
        assert!(!step.advanced);

        let step = navigator.step(2.0);
        assert_eq!(step.moved, 2.0, "the remaining distance, not the budget");
        assert_eq!(step.to, (4.0, 0.0, 0.0));
        assert!(step.advanced, "it reached the waypoint");
        assert!(step.finished);
        assert_eq!(navigator.waypoint, 2);

        let step = navigator.step(1.0);
        assert!(step.finished);
        assert_eq!(step.moved, 0.0, "past the end, nothing moves");
    }

    #[test]
    fn a_stuck_detector_fires_below_a_progress_threshold() {
        let mut stuck = StuckDetector::new(3, 0.5);
        assert!(!stuck.is_stuck(), "no history yet");
        stuck.record(0.1);
        stuck.record(0.1);
        assert!(!stuck.is_stuck(), "the window is not full");
        stuck.record(0.1);
        assert!(stuck.is_stuck(), "0.3 over three steps is under 0.5");

        let mut moving = StuckDetector::new(3, 0.5);
        moving.record(0.5);
        moving.record(0.5);
        moving.record(0.5);
        assert!(!moving.is_stuck(), "1.5 over three steps is not stuck");

        let mut navigator = Navigator::at(
            (0.0, 0.0, 0.0),
            Path::new(vec![(0.0, 0.0, 0.0), (100.0, 0.0, 0.0)]),
            0.01,
        );
        navigator.stuck = StuckDetector::new(3, 0.5);
        for _ in 0..3 {
            navigator.step(1.0);
        }
        assert!(navigator.is_stuck(), "tiny steps against a far waypoint");
    }

    #[test]
    fn a_targets_cooldown_ticks_down_and_its_last_known_position_survives() {
        let mut target = MobTarget::new("sheep-1", "sheep", (5.0, 64.0, 0.0)).with_cooldown(3);
        assert!(!target.is_expired());
        assert!(target.tick(2), "two of three ticks");
        assert!(!target.is_expired());
        assert!(!target.tick(1), "the cooldown is gone");
        assert!(target.is_expired());

        let mut lost = MobTarget::new("sheep-1", "sheep", (5.0, 64.0, 0.0)).with_cooldown(10);
        lost.position = (6.0, 64.0, 0.0);
        lost.lost();
        assert!(!lost.seen);
        assert_eq!(lost.last_known, (6.0, 64.0, 0.0), "where it was last seen");
        assert!(!lost.is_expired(), "losing sight is not the cooldown running out");
    }

    #[test]
    fn a_path_knows_its_length_and_a_tracked_entity_keeps_its_path_and_reach() {
        let _serial = serial();
        fresh();
        let path = create_path(vec![(0.0, 0.0, 0.0), (3.0, 0.0, 0.0), (3.0, 4.0, 0.0)]);
        assert_eq!(path.length(), 7.0);
        assert!(path.is_reachable(7.0));
        assert!(!path.is_reachable(6.9));
        assert_eq!(path.end(), Some((3.0, 4.0, 0.0)));
        assert!(!path.is_empty());

        let empty = Path::new(Vec::new());
        assert_eq!(empty.length(), 0.0);
        assert!(empty.length().is_sign_positive(), "an empty total is +0, not -0");
        assert!(empty.is_reachable(0.0));
        assert!(empty.is_empty());

        assert!(follow_path("wolf", path.clone()));
        assert_eq!(get_path("wolf"), Some(path));
        assert!(can_reach("wolf", (3.0, 4.0, 0.0), 5.0));
        assert!(!can_reach("wolf", (3.0, 4.0, 0.0), 4.0));
        assert!(!follow_path("wolf", Path::default()), "an empty path is refused");
        assert!(cancel_path("wolf"));
        assert_eq!(get_path("wolf"), None);
        assert!(!cancel_path("wolf"));

        assert!(navigate_to("wolf", (0.0, 0.0, 0.0), (10.0, 0.0, 0.0), 2.0));
        assert!(get_path("wolf").is_some());
        assert!(!navigate_to("wolf", (1.0, 1.0, 1.0), (1.0, 1.0, 1.0), 1.0));
        assert!(stop_navigation("wolf"));
        assert!(!stop_navigation("wolf"));
        assert!(!can_reach("wolf", (0.0, 0.0, 0.0), 1.0), "no navigator, no reach");

        assert!(set_target("wolf", MobTarget::new("sheep", "sheep", (1.0, 0.0, 0.0))));
        assert_eq!(get_target("wolf").map(|target| target.id), Some("sheep".to_owned()));
        assert!(!tick_target("wolf", 100), "the default cooldown runs out");
        assert_eq!(get_target("wolf"), None);
        assert!(!tick_target("wolf", 1), "nothing tracked");
        assert!(!clear_target("wolf"));
        assert!(set_target("wolf", MobTarget::new("sheep", "sheep", (1.0, 0.0, 0.0))));
        assert!(clear_target("wolf"));
        assert!(!clear_target("wolf"));
    }

    #[test]
    fn the_clients_mob_ai_is_refused_with_its_own_reason() {
        let _serial = serial();
        assert!(get_brain().is_none());
        assert!(get_navigation().is_none());
        assert!(get_senses().is_empty());
        assert!(!can_see("wolf", "sheep"));
        assert!(!can_hear("wolf", "sheep"));
        assert!(get_nearby_entities((0.0, 0.0, 0.0), 16.0).is_empty());
        assert!(!register_brain_provider("hello:brain"));
        assert!(!register_ai_step("hello:step"));
        assert!(!modify_target_selection("hello:targets"));
        assert!(!modify_navigation("hello:nav"));
        assert!(!modify_pathfinding("hello:path"));
        assert_eq!(refusals().len(), 6);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert_eq!(why_unknown("nonsense"), "nonsense is not something this host can see");
    }
}
