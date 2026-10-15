//! Movement: the numbers a mod owns, and the input the client owns.
//!
//! Movement is where the client and the server disagree most often — the client predicts, the
//! server corrects — and this host can do neither. What it *can* do is hold a
//! [`MovementProfile`], a table of the speeds the game uses, take an [`Input`] a mod has and
//! turn it into a movement vector, and predict where a body would be after so many ticks,
//! which is exactly what a mod writing a pathfinder, a bot or a movement check needs.
//!
//! The numbers published here are the game's own defaults, said to be defaults rather than a
//! live read of the client's: walking `0.1` blocks per tick, sprinting `0.13`, sneaking
//! `0.03`, flying `0.05`, swimming `0.02`, a jump strength of `0.42`, a step height of `0.6`,
//! and the acceleration a player reaches their speed with.
//!
//! Refused, each with its reason: the live input vector, the player's own speeds and the
//! setters for them (they are server attributes), every `start_*`/`stop_*` action (the server
//! decides), the server's correction and authoritative position, and the four `register_*`
//! hooks, which would sit in the movement path.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! let input = Input { forward: true, sprint: true, ..Input::default() };
//! assert_eq!(input.vector(), (0.0, -1.0), "forward is negative z");
//! let profile = MovementProfile::default();
//! let moved = movement::predict(&profile, (0.0, 64.0, 0.0), &input, 10);
//! assert!(moved.2 < 0.0, "walking forward moves north");
//! ```

use std::fmt;

/// Walking speed, in blocks per tick.
pub const WALK_SPEED: f64 = 0.1;
/// Sprinting speed.
pub const SPRINT_SPEED: f64 = 0.13;
/// Sneaking speed.
pub const SNEAK_SPEED: f64 = 0.03;
/// Flying speed.
pub const FLY_SPEED: f64 = 0.05;
/// Swimming speed.
pub const SWIM_SPEED: f64 = 0.02;
/// How hard a jump pushes, which is what the game uses for a one-block jump and a bit.
pub const JUMP_STRENGTH: f64 = 0.42;
/// How tall a step a player can walk up.
pub const STEP_HEIGHT: f64 = 0.6;
/// How fast a player reaches their walking speed.
pub const ACCELERATION: f64 = 0.02;

/// How a player is getting about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MovementMode {
    #[default]
    Walking,
    Flying,
    Swimming,
    Climbing,
    Riding,
    Spectator,
}

impl MovementMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Walking => "walking",
            Self::Flying => "flying",
            Self::Swimming => "swimming",
            Self::Climbing => "climbing",
            Self::Riding => "riding",
            Self::Spectator => "spectator",
        }
    }

    /// The speed this mode moves at, from a profile.
    #[must_use]
    pub fn speed(self, profile: &MovementProfile) -> f64 {
        match self {
            Self::Walking => profile.speed,
            Self::Flying => profile.fly_speed,
            Self::Swimming => profile.swim_speed,
            Self::Climbing => profile.sneak_speed,
            Self::Riding => profile.speed * 1.5,
            Self::Spectator => profile.fly_speed,
        }
    }
}

impl fmt::Display for MovementMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a player is pressing, as a value. The live input is the client's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Input {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
    /// How far the look has turned since the last tick, in degrees.
    pub look_yaw: i32,
    pub look_pitch: i32,
}

impl Input {
    /// The movement vector this input means, as `(x, z)`, normalised the way the game
    /// normalises a diagonal so it is not faster than a straight line. Forward is `-z`.
    #[must_use]
    pub fn vector(&self) -> (f64, f64) {
        let forward = f64::from(i32::from(self.forward) - i32::from(self.backward));
        let strafe = f64::from(i32::from(self.right) - i32::from(self.left));
        let length = (forward * forward + strafe * strafe).sqrt();
        if length == 0.0 {
            return (0.0, 0.0);
        }
        (strafe / length, -forward / length)
    }

    /// Whether the player is asking to move at all.
    #[must_use]
    pub fn is_moving(&self) -> bool {
        self.vector() != (0.0, 0.0)
    }

    /// Whether the player is sneaking, which the game treats as a modifier rather than a key.
    #[must_use]
    pub fn is_sneaking(&self) -> bool {
        self.sneak
    }

    /// Whether the player is sprinting.
    #[must_use]
    pub fn is_sprinting(&self) -> bool {
        self.sprint
    }
}

impl fmt::Display for Input {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (x, z) = self.vector();
        write!(formatter, "({x:.2}, {z:.2})")
    }
}

/// The speeds and limits a mod moves a body with.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementProfile {
    pub speed: f64,
    pub acceleration: f64,
    pub jump_strength: f64,
    pub step_height: f64,
    pub fly_speed: f64,
    pub sprint_speed: f64,
    pub swim_speed: f64,
    pub sneak_speed: f64,
    pub mode: MovementMode,
    pub flight_allowed: bool,
    pub noclip_allowed: bool,
    pub predictions_enabled: bool,
}

impl Default for MovementProfile {
    fn default() -> Self {
        Self {
            speed: WALK_SPEED,
            acceleration: ACCELERATION,
            jump_strength: JUMP_STRENGTH,
            step_height: STEP_HEIGHT,
            fly_speed: FLY_SPEED,
            sprint_speed: SPRINT_SPEED,
            swim_speed: SWIM_SPEED,
            sneak_speed: SNEAK_SPEED,
            mode: MovementMode::Walking,
            flight_allowed: false,
            noclip_allowed: false,
            predictions_enabled: true,
        }
    }
}

impl MovementProfile {
    /// A profile with the game's defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The speed this profile would move at for an input: sneaking beats walking, sprinting
    /// beats both, and the mode decides when nobody is pressing anything.
    #[must_use]
    pub fn speed_for(&self, input: &Input) -> f64 {
        if !input.is_moving() {
            return 0.0;
        }
        if input.is_sneaking() {
            return self.sneak_speed;
        }
        match self.mode {
            MovementMode::Flying | MovementMode::Spectator => self.fly_speed,
            MovementMode::Swimming => self.swim_speed,
            MovementMode::Riding => self.speed * 1.5,
            _ if input.is_sprinting() => self.sprint_speed,
            _ => self.speed,
        }
    }

    /// How high a jump from here would go, in blocks, with the profile's jump strength and
    /// Bedrock's gravity.
    #[must_use]
    pub fn jump_height(&self) -> f64 {
        let gravity = crate::physics::GRAVITY;
        // v^2 / 2g, with the drag ignored: what a player actually clears.
        (self.jump_strength * self.jump_strength) / (2.0 * gravity)
    }

    /// Whether a step of that height can be walked up.
    #[must_use]
    pub fn can_step_up(&self, height: f64) -> bool {
        height <= self.step_height
    }

    /// Whether flight is allowed with this profile.
    #[must_use]
    pub fn is_flight_allowed(&self) -> bool {
        self.flight_allowed
    }

    /// Whether no-clip is allowed, which is a spectator's privilege.
    #[must_use]
    pub fn is_noclip_allowed(&self) -> bool {
        self.noclip_allowed
    }
}

impl fmt::Display for MovementProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} at {:.2} (sprint {:.2}, sneak {:.2}, fly {:.2}, jump {:.0})",
            self.mode, self.speed, self.sprint_speed, self.sneak_speed, self.fly_speed, self.jump_strength
        )
    }
}

/// One tick of movement a profile produces: where a body goes and how fast.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementStep {
    pub from: (f64, f64, f64),
    pub to: (f64, f64, f64),
    pub velocity: (f64, f64, f64),
    pub vertical: f64,
    pub mode: MovementMode,
}

impl MovementStep {
    /// How far it moved.
    #[must_use]
    pub fn distance(&self) -> f64 {
        crate::entities::distance(self.from, self.to)
    }
}

/// Where a body ends up after `ticks` of this input, which is the prediction a mod can make
/// for itself: the game's own prediction is the client's.
#[must_use]
pub fn predict(
    profile: &MovementProfile,
    from: (f64, f64, f64),
    input: &Input,
    ticks: u32,
) -> (f64, f64, f64) {
    let (x, z) = input.vector();
    let speed = profile.speed_for(input);
    let mut position = from;
    if profile.mode == MovementMode::Flying || profile.mode == MovementMode::Spectator {
        position.0 += x * speed * f64::from(ticks);
        position.2 += z * speed * f64::from(ticks);
        // Rising and diving do not need horizontal input, so they use the mode's own speed.
        let vertical = profile.mode.speed(profile);
        if input.jump {
            position.1 += vertical * f64::from(ticks);
        }
        if input.sneak {
            position.1 -= vertical * f64::from(ticks);
        }
        return position;
    }
    position.0 += x * speed * f64::from(ticks);
    position.2 += z * speed * f64::from(ticks);
    if input.jump && profile.mode != MovementMode::Riding {
        // A jump is one impulse, then gravity: the height is the profile's, and it comes back
        // down over the ticks that follow.
        let flight_ticks = (profile.jump_strength / crate::physics::GRAVITY).max(1.0);
        let steps = flight_ticks * 2.0;
        let net = f64::from(ticks) / steps;
        if net < 1.0 {
            position.1 += profile.jump_height() * net * 2.0;
        }
    }
    position
}

/// Every tick of movement this input would produce, for a mod that wants to see the path.
#[must_use]
pub fn steps(
    profile: &MovementProfile,
    from: (f64, f64, f64),
    input: &Input,
    ticks: u32,
) -> Vec<MovementStep> {
    let mut position = from;
    let mut walked = Vec::new();
    let (x, z) = input.vector();
    let speed = profile.speed_for(input);
    for _ in 0..ticks {
        let next = (
            position.0 + x * speed,
            position.1,
            position.2 + z * speed,
        );
        walked.push(MovementStep {
            from: position,
            to: next,
            velocity: (x * speed, 0.0, z * speed),
            vertical: 0.0,
            mode: profile.mode,
        });
        position = next;
    }
    walked
}

/// Why a movement question about the player has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "input" => "what the player is pressing is the client's input, which this host cannot read".to_owned(),
        "speed" => {
            "a player's speeds are attributes the server sets and the client applies; this \
             host cannot read them, and setting them is the server's"
                .to_owned()
        }
        "action" => {
            "jumping, sprinting, sneaking and flying are the server's: the client sends them \
             on the player's own input, and this host cannot even ask it to"
                .to_owned()
        }
        "correction" => "the server's correction and the authoritative position are the client's, from its connection".to_owned(),
        "prediction" => "the client's own prediction is inside its movement path; a mod can predict for itself, and this is that".to_owned(),
        "hook" => {
            "these would sit inside the client's movement path, which is hot and where hooks \
             have crashed this client before"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach about movement.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["input", "speed", "action", "correction", "prediction", "hook"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about the player's movement: {}", why_unknown(what));
    false
}

/// The live input vector. `None`.
#[must_use]
pub fn get_input() -> Option<Input> {
    None
}

/// The live input as a vector. `None`.
#[must_use]
pub fn get_input_vector() -> Option<(f64, f64)> {
    None
}

/// The player's walking speed. `None`: it is a server attribute.
#[must_use]
pub fn get_speed() -> Option<f64> {
    None
}

/// Sets the player's walking speed. `false`.
pub fn set_speed(_speed: f64) -> bool {
    refused("speed")
}

/// The player's acceleration. `None`.
#[must_use]
pub fn get_acceleration() -> Option<f64> {
    None
}

/// Sets the player's acceleration. `false`.
pub fn set_acceleration(_acceleration: f64) -> bool {
    refused("speed")
}

/// The player's jump strength. `None`.
#[must_use]
pub fn get_jump_strength() -> Option<f64> {
    None
}

/// Sets the player's jump strength. `false`.
pub fn set_jump_strength(_strength: f64) -> bool {
    refused("speed")
}

/// The player's step height. `None`.
#[must_use]
pub fn get_step_height() -> Option<f64> {
    None
}

/// Sets the player's step height. `false`.
pub fn set_step_height(_height: f64) -> bool {
    refused("speed")
}

/// The player's flying speed. `None`.
#[must_use]
pub fn get_fly_speed() -> Option<f64> {
    None
}

/// Sets the player's flying speed. `false`.
pub fn set_fly_speed(_speed: f64) -> bool {
    refused("speed")
}

/// The player's sprinting speed. `None`.
#[must_use]
pub fn get_sprint_speed() -> Option<f64> {
    None
}

/// Sets the player's sprinting speed. `false`.
pub fn set_sprint_speed(_speed: f64) -> bool {
    refused("speed")
}

/// The player's swimming speed. `None`.
#[must_use]
pub fn get_swim_speed() -> Option<f64> {
    None
}

/// Sets the player's swimming speed. `false`.
pub fn set_swim_speed(_speed: f64) -> bool {
    refused("speed")
}

/// The player's sneaking speed. `None`.
#[must_use]
pub fn get_sneak_speed() -> Option<f64> {
    None
}

/// Sets the player's sneaking speed. `false`.
pub fn set_sneak_speed(_speed: f64) -> bool {
    refused("speed")
}

/// Makes the player jump. `false`.
pub fn jump() -> bool {
    refused("action")
}

/// Makes the player sprint. `false`.
pub fn start_sprinting() -> bool {
    refused("action")
}

/// Stops the player sprinting. `false`.
pub fn stop_sprinting() -> bool {
    refused("action")
}

/// Makes the player sneak. `false`.
pub fn start_sneaking() -> bool {
    refused("action")
}

/// Stops the player sneaking. `false`.
pub fn stop_sneaking() -> bool {
    refused("action")
}

/// Makes the player fly. `false`.
pub fn start_flying() -> bool {
    refused("action")
}

/// Stops the player flying. `false`.
pub fn stop_flying() -> bool {
    refused("action")
}

/// Allows or forbids flight. `false`: the server decides.
pub fn set_flight_allowed(_allowed: bool) -> bool {
    refused("action")
}

/// Allows or forbids no-clip. `false`: it would desync immediately.
pub fn set_noclip_allowed(_allowed: bool) -> bool {
    refused("action")
}

/// Sets how the player is getting about. `false`.
pub fn set_movement_mode(_mode: MovementMode) -> bool {
    refused("action")
}

/// Registers a movement modifier. `false`.
pub fn register_movement_modifier(_name: &str) -> bool {
    refused("hook")
}

/// Registers an input handler. `false`.
pub fn register_input_handler(_name: &str) -> bool {
    refused("hook")
}

/// Registers a collision modifier. `false`.
pub fn register_collision_modifier(_name: &str) -> bool {
    refused("hook")
}

/// The client's own movement prediction. `None`.
#[must_use]
pub fn get_movement_prediction() -> Option<(f64, f64, f64)> {
    None
}

/// Turns the client's prediction on or off. `false`.
pub fn set_movement_prediction(_enabled: bool) -> bool {
    refused("prediction")
}

/// The last correction the server sent. `None`.
#[must_use]
pub fn get_server_correction() -> Option<(f64, f64, f64)> {
    None
}

/// The position the server says the player is at. `None`.
#[must_use]
pub fn get_authoritative_position() -> Option<(f64, f64, f64)> {
    None
}

/// Where the client thinks the player is. `None`.
#[must_use]
pub fn get_predicted_position() -> Option<(f64, f64, f64)> {
    None
}

/// The game's own movement numbers, for a caller that wants to see them.
#[must_use]
pub fn constants() -> Vec<(&'static str, f64)> {
    vec![
        ("walk", WALK_SPEED),
        ("sprint", SPRINT_SPEED),
        ("sneak", SNEAK_SPEED),
        ("fly", FLY_SPEED),
        ("swim", SWIM_SPEED),
        ("jump_strength", JUMP_STRENGTH),
        ("step_height", STEP_HEIGHT),
        ("acceleration", ACCELERATION),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn an_input_turns_into_a_movement_vector() {
        let _serial = serial();
        assert_eq!(Input::default().vector(), (0.0, 0.0));
        assert!(!Input::default().is_moving());
        assert_eq!(
            Input {
                forward: true,
                ..Input::default()
            }
            .vector(),
            (0.0, -1.0),
            "forward is -z"
        );
        assert_eq!(
            Input {
                backward: true,
                ..Input::default()
            }
            .vector(),
            (0.0, 1.0)
        );
        assert_eq!(
            Input {
                left: true,
                ..Input::default()
            }
            .vector(),
            (-1.0, 0.0)
        );
        assert_eq!(
            Input {
                right: true,
                ..Input::default()
            }
            .vector(),
            (1.0, 0.0)
        );
        // Pressing two keys does not make a player faster: the vector is normalised.
        let diagonal = Input {
            forward: true,
            right: true,
            ..Input::default()
        }
        .vector();
        let length = (diagonal.0 * diagonal.0 + diagonal.1 * diagonal.1).sqrt();
        assert!((length - 1.0).abs() < 1e-9, "{diagonal:?}");
        // Opposite keys cancel out.
        assert_eq!(
            Input {
                forward: true,
                backward: true,
                ..Input::default()
            }
            .vector(),
            (0.0, 0.0)
        );
        assert!(Input {
            sprint: true,
            ..Input::default()
        }
        .is_sprinting());
        assert!(Input {
            sneak: true,
            ..Input::default()
        }
        .is_sneaking());
    }

    #[test]
    fn a_profile_picks_the_speed_the_input_asks_for() {
        let _serial = serial();
        let profile = MovementProfile::new();
        assert_eq!(profile.speed_for(&Input::default()), 0.0, "standing still");
        let walking = Input {
            forward: true,
            ..Input::default()
        };
        assert_eq!(profile.speed_for(&walking), WALK_SPEED);
        let sprinting = Input {
            sprint: true,
            ..walking
        };
        assert_eq!(profile.speed_for(&sprinting), SPRINT_SPEED);
        let sneaking = Input {
            sneak: true,
            ..walking
        };
        assert_eq!(profile.speed_for(&sneaking), SNEAK_SPEED, "sneaking beats sprinting");

        let mut flying = profile.clone();
        flying.mode = MovementMode::Flying;
        assert_eq!(flying.speed_for(&walking), FLY_SPEED);
        let mut swimming = profile.clone();
        swimming.mode = MovementMode::Swimming;
        assert_eq!(swimming.speed_for(&walking), SWIM_SPEED);
        assert_eq!(MovementMode::Walking.speed(&profile), WALK_SPEED);
        assert_eq!(MovementMode::Riding.speed(&profile), WALK_SPEED * 1.5);
        assert!(profile.to_string().contains("walking"), "{}", profile);
    }

    #[test]
    fn a_jump_clears_about_a_block_and_a_quarter() {
        let _serial = serial();
        let profile = MovementProfile::new();
        let height = profile.jump_height();
        assert!(height > 1.0 && height < 1.5, "{height}");
        assert!(profile.can_step_up(0.5));
        assert!(profile.can_step_up(STEP_HEIGHT));
        assert!(!profile.can_step_up(0.7));
        assert!(!profile.is_flight_allowed());
        assert!(!profile.is_noclip_allowed());
    }

    #[test]
    fn a_prediction_moves_the_body_the_way_the_speed_says() {
        let _serial = serial();
        let profile = MovementProfile::new();
        let walking = Input {
            forward: true,
            ..Input::default()
        };
        let moved = predict(&profile, (0.0, 64.0, 0.0), &walking, 10);
        assert!((moved.2 + 1.0).abs() < 1e-9, "ten ticks of 0.1: {moved:?}");
        assert_eq!(moved.0, 0.0);
        assert_eq!(moved.1, 64.0);

        let sprinting = Input {
            sprint: true,
            ..walking
        };
        let faster = predict(&profile, (0.0, 64.0, 0.0), &sprinting, 10);
        assert!(faster.2 < moved.2, "sprinting gets further");

        let standing = predict(&profile, (3.0, 64.0, 3.0), &Input::default(), 10);
        assert_eq!(standing, (3.0, 64.0, 3.0), "standing still goes nowhere");

        let jumping = Input {
            jump: true,
            ..walking
        };
        let jumped = predict(&profile, (0.0, 64.0, 0.0), &jumping, 2);
        assert!(jumped.1 > 64.0, "a jump rises: {jumped:?}");

        let mut flying = profile.clone();
        flying.mode = MovementMode::Flying;
        let up = predict(
            &flying,
            (0.0, 64.0, 0.0),
            &Input {
                jump: true,
                ..Input::default()
            },
            5,
        );
        assert!(up.1 > 64.0, "flying with jump rises: {up:?}");
        let down = predict(
            &flying,
            (0.0, 64.0, 0.0),
            &Input {
                sneak: true,
                ..Input::default()
            },
            5,
        );
        assert!(down.1 < 64.0, "flying with sneak descends");
    }

    #[test]
    fn the_path_is_available_tick_by_tick() {
        let _serial = serial();
        let profile = MovementProfile::new();
        let walking = Input {
            forward: true,
            ..Input::default()
        };
        let walked = steps(&profile, (0.0, 64.0, 0.0), &walking, 3);
        assert_eq!(walked.len(), 3);
        assert!((walked[0].distance() - 0.1).abs() < 1e-9);
        assert_eq!(walked[2].to.2, predict(&profile, (0.0, 64.0, 0.0), &walking, 3).2);
        assert_eq!(walked[0].mode, MovementMode::Walking);
    }

    #[test]
    fn the_clients_movement_is_refused_with_its_own_reason() {
        let _serial = serial();
        assert!(get_input().is_none());
        assert!(get_input_vector().is_none());
        assert!(get_speed().is_none());
        assert!(get_acceleration().is_none());
        assert!(get_jump_strength().is_none());
        assert!(get_step_height().is_none());
        assert!(get_fly_speed().is_none());
        assert!(get_sprint_speed().is_none());
        assert!(get_swim_speed().is_none());
        assert!(get_sneak_speed().is_none());
        assert!(get_movement_prediction().is_none());
        assert!(get_server_correction().is_none());
        assert!(get_authoritative_position().is_none());
        assert!(get_predicted_position().is_none());
        assert!(!set_speed(1.0));
        assert!(!set_acceleration(1.0));
        assert!(!set_jump_strength(1.0));
        assert!(!set_step_height(1.0));
        assert!(!set_fly_speed(1.0));
        assert!(!set_sprint_speed(1.0));
        assert!(!set_swim_speed(1.0));
        assert!(!set_sneak_speed(1.0));
        assert!(!jump());
        assert!(!start_sprinting());
        assert!(!stop_sprinting());
        assert!(!start_sneaking());
        assert!(!stop_sneaking());
        assert!(!start_flying());
        assert!(!stop_flying());
        assert!(!set_flight_allowed(true));
        assert!(!set_noclip_allowed(true));
        assert!(!set_movement_mode(MovementMode::Flying));
        assert!(!register_movement_modifier("hello:swift"));
        assert!(!register_input_handler("hello:keys"));
        assert!(!register_collision_modifier("hello:slick"));
        assert!(!set_movement_prediction(false));
        assert_eq!(constants().len(), 8);
        assert_eq!(refusals().len(), 6);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
