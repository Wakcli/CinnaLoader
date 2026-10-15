//! Camera: the camera a mod owns, and the one it will never touch.
//!
//! This host cannot move the client's camera — that needs the hooks the project records as
//! never. So a mod owns a real [`Camera`] instead, and everything here is arithmetic over
//! that value: position, a rotation in degrees (yaw, pitch, roll), the forward, up and right
//! basis they define, a field of view with clamping, near and far planes, a camera mode, a
//! zoom that narrows the field of view, a third-person distance, bobbing and roll, a
//! free-camera flag, shake that accumulates and decays, and a reset back to the defaults.
//!
//! The valuable part is the maths. [`Camera::view_matrix`] and
//! [`Camera::projection_matrix`] are real 4×4 matrices — **row-major, with column vectors**,
//! so a point transforms as `v' = M·v` and the translation sits in the last column — built
//! from the camera's own transform and its fov, viewport aspect, near and far planes.
//! [`Camera::world_to_screen`] projects through view×projection and maps NDC to pixels,
//! answering `None` for a point behind the camera; [`Camera::screen_to_world`] unprojects at
//! a given depth; [`Camera::frustum`] extracts the six planes from view×projection, and
//! [`Camera::is_visible`] tests a point or a box against them.
//!
//! What is refused is the *client's* camera: reading or setting its position, rotation, mode,
//! attached entity or field of view, the shake the client applies to it, and a modifier or
//! controller actually being called by the client's renderer. Each has its own sentence in
//! [`why_unknown`], and [`refusals`] lists them.
//!
//! ```
//! use cinnaloader::camera::{self, Camera};
//!
//! let mut cam = Camera::new();
//! cam.set_rotation(180.0, 0.0, 0.0);
//! let (sx, sy, depth) = cam.world_to_screen((0.0, 0.0, -10.0)).expect("in front");
//! let back = cam.screen_to_world(sx, sy, depth).expect("it unprojects");
//! assert!((back.2 + 10.0).abs() < 1e-2, "{back:?}");
//! assert!(cam.world_to_screen((0.0, 0.0, 10.0)).is_none(), "behind the camera");
//! assert!(!camera::set_client_fov(90.0), "the client's own camera is out of reach");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

pub use crate::models::Vec3;

/// The widest and narrowest field of view a camera may be given, in degrees.
pub const MIN_FOV: f32 = 1.0;
/// The upper bound of the same.
pub const MAX_FOV: f32 = 179.0;
/// The default vertical field of view, which is the game's own.
pub const DEFAULT_FOV: f32 = 70.0;
/// The default distance to the near plane.
pub const DEFAULT_NEAR: f32 = 0.05;
/// The default distance to the far plane.
pub const DEFAULT_FAR: f32 = 1000.0;
/// How long shake takes to fall to about a third of its amplitude, in milliseconds.
pub const SHAKE_DECAY_MS: f32 = 250.0;

const SHAKE_MINIMUM: f32 = 1e-4;
const SHAKE_RATE: f32 = 0.02;
const EPSILON: f32 = 1e-6;

/// A column-major-free 4×4 matrix: sixteen floats in **row-major** order, applied to column
/// vectors.
pub type Mat4 = [f32; 16];

/// How a mod's camera is positioned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CameraMode {
    #[default]
    FirstPerson,
    ThirdPerson,
    ThirdPersonFront,
    Fixed,
    Free,
    Spectator,
}

impl CameraMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FirstPerson => "first_person",
            Self::ThirdPerson => "third_person",
            Self::ThirdPersonFront => "third_person_front",
            Self::Fixed => "fixed",
            Self::Free => "free",
            Self::Spectator => "spectator",
        }
    }
}

impl fmt::Display for CameraMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One clipping plane, as `normal·point + distance`, positive on the inside.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Plane {
    pub normal: Vec3,
    pub distance: f32,
}

impl Plane {
    /// How far a point is on the inside of the plane.
    #[must_use]
    pub fn distance_to(&self, point: Vec3) -> f32 {
        self.normal.0 * point.0 + self.normal.1 * point.1 + self.normal.2 * point.2 + self.distance
    }
}

/// The six planes of a view frustum, in the order left, right, bottom, top, near, far.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    #[must_use]
    pub fn plane(&self, index: usize) -> Option<Plane> {
        self.planes.get(index).copied()
    }

    /// Whether a point is inside every plane.
    #[must_use]
    pub fn contains_point(&self, point: Vec3) -> bool {
        self.planes
            .iter()
            .all(|plane| plane.distance_to(point) >= -1e-3)
    }

    /// Whether a box is inside; tested at the corner furthest along each plane's normal, so a
    /// box straddling a plane still counts.
    #[must_use]
    pub fn contains_box(&self, low: Vec3, high: Vec3) -> bool {
        self.planes.iter().all(|plane| {
            let corner = (
                if plane.normal.0 >= 0.0 { high.0 } else { low.0 },
                if plane.normal.1 >= 0.0 { high.1 } else { low.1 },
                if plane.normal.2 >= 0.0 { high.2 } else { low.2 },
            );
            plane.distance_to(corner) >= 0.0
        })
    }
}

/// A camera a mod owns, with the transform and settings the maths needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    position: Vec3,
    rotation: (f32, f32, f32),
    fov: f32,
    near: f32,
    far: f32,
    zoom: f32,
    mode: CameraMode,
    entity: Option<String>,
    target: Option<String>,
    third_person_distance: f32,
    bobbing: bool,
    viewport: (u32, u32),
    shake: f32,
    shake_offset: Vec3,
    shake_time: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera {
    /// A camera at the origin, looking down +Z at 70°, with the game's own near and far.
    #[must_use]
    pub fn new() -> Self {
        Self {
            position: (0.0, 0.0, 0.0),
            rotation: (0.0, 0.0, 0.0),
            fov: DEFAULT_FOV,
            near: DEFAULT_NEAR,
            far: DEFAULT_FAR,
            zoom: 1.0,
            mode: CameraMode::FirstPerson,
            entity: None,
            target: None,
            third_person_distance: 4.0,
            bobbing: true,
            viewport: (1920, 1080),
            shake: 0.0,
            shake_offset: (0.0, 0.0, 0.0),
            shake_time: 0.0,
        }
    }

    #[must_use]
    pub fn position(&self) -> Vec3 {
        self.position
    }

    pub fn set_position(&mut self, x: f32, y: f32, z: f32) {
        self.position = (x, y, z);
    }

    #[must_use]
    pub fn rotation(&self) -> (f32, f32, f32) {
        self.rotation
    }

    pub fn set_rotation(&mut self, yaw: f32, pitch: f32, roll: f32) {
        self.rotation = (yaw, wrap_degrees(pitch).clamp(-90.0, 90.0), roll);
    }

    pub fn set_roll(&mut self, roll: f32) {
        self.rotation.2 = roll;
    }

    /// The forward direction the rotation points along.
    #[must_use]
    pub fn direction(&self) -> Vec3 {
        direction_from_rotation(self.rotation.0, self.rotation.1)
    }

    /// The up direction, with roll folded in.
    #[must_use]
    pub fn up_vector(&self) -> Vec3 {
        self.basis().1
    }

    /// The right direction, with roll folded in.
    #[must_use]
    pub fn right_vector(&self) -> Vec3 {
        self.basis().0
    }

    #[must_use]
    pub fn fov(&self) -> f32 {
        self.fov
    }

    pub fn set_fov(&mut self, fov: f32) {
        self.fov = fov.clamp(MIN_FOV, MAX_FOV);
    }

    /// The field of view the projection uses: the fov narrowed by the zoom.
    #[must_use]
    pub fn effective_fov(&self) -> f32 {
        (self.fov / self.zoom).clamp(MIN_FOV, MAX_FOV)
    }

    #[must_use]
    pub fn near_plane(&self) -> f32 {
        self.near
    }

    #[must_use]
    pub fn far_plane(&self) -> f32 {
        self.far
    }

    #[must_use]
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.max(0.01);
    }

    #[must_use]
    pub fn mode(&self) -> CameraMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: CameraMode) {
        self.mode = mode;
    }

    #[must_use]
    pub fn entity(&self) -> Option<String> {
        self.entity.clone()
    }

    pub fn set_entity(&mut self, id: &str) {
        self.entity = Some(id.to_owned());
    }

    #[must_use]
    pub fn target(&self) -> Option<String> {
        self.target.clone()
    }

    pub fn set_target(&mut self, id: &str) {
        self.target = Some(id.to_owned());
    }

    #[must_use]
    pub fn third_person_distance(&self) -> f32 {
        self.third_person_distance
    }

    pub fn set_third_person_distance(&mut self, distance: f32) {
        self.third_person_distance = distance.max(0.0);
    }

    #[must_use]
    pub fn bobbing(&self) -> bool {
        self.bobbing
    }

    pub fn set_bobbing(&mut self, bobbing: bool) {
        self.bobbing = bobbing;
    }

    #[must_use]
    pub fn is_free(&self) -> bool {
        self.mode == CameraMode::Free
    }

    pub fn set_free_camera(&mut self, free: bool) {
        self.mode = if free {
            CameraMode::Free
        } else if self.mode == CameraMode::Free {
            CameraMode::FirstPerson
        } else {
            self.mode
        };
    }

    #[must_use]
    pub fn viewport(&self) -> (u32, u32) {
        self.viewport
    }

    pub fn set_viewport(&mut self, width: u32, height: u32) {
        self.viewport = (width.max(1), height.max(1));
    }

    #[must_use]
    pub fn aspect(&self) -> f32 {
        self.viewport.0 as f32 / self.viewport.1 as f32
    }

    /// Adds to the shake amplitude, which accumulates.
    pub fn shake(&mut self, amount: f32) {
        self.shake = (self.shake + amount.abs()).max(0.0);
        self.recompute_shake();
    }

    /// Sets the shake amplitude outright.
    pub fn set_shake(&mut self, amount: f32) {
        self.shake = amount.abs();
        self.recompute_shake();
    }

    #[must_use]
    pub fn shake_amplitude(&self) -> f32 {
        self.shake
    }

    /// The offset shake currently puts on the camera's position.
    #[must_use]
    pub fn shake_offset(&self) -> Vec3 {
        self.shake_offset
    }

    /// Decays shake towards zero over a slice of time.
    pub fn advance(&mut self, dt_millis: u32) {
        if self.shake <= 0.0 {
            return;
        }
        self.shake_time += dt_millis as f32;
        self.shake *= (-(dt_millis as f32) / SHAKE_DECAY_MS).exp();
        if self.shake < SHAKE_MINIMUM {
            self.shake = 0.0;
        }
        self.recompute_shake();
    }

    /// Points the camera at a target and answers the rotation it set. A target straight up or
    /// down keeps the current yaw, since only the pitch is decided.
    pub fn look_at(&mut self, target: Vec3) -> (f32, f32, f32) {
        let delta = (
            target.0 - self.position.0,
            target.1 - self.position.1,
            target.2 - self.position.2,
        );
        let length = (delta.0 * delta.0 + delta.1 * delta.1 + delta.2 * delta.2).sqrt();
        if length < EPSILON {
            return self.rotation;
        }
        let horizontal = (delta.0 * delta.0 + delta.2 * delta.2).sqrt();
        let pitch = (-delta.1 / length).clamp(-1.0, 1.0).asin().to_degrees();
        let yaw = if horizontal < EPSILON {
            self.rotation.0
        } else {
            wrap_degrees((-delta.0).atan2(delta.2).to_degrees())
        };
        self.rotation = (yaw, pitch, self.rotation.2);
        self.rotation
    }

    /// The camera's world position with shake folded in.
    #[must_use]
    pub fn effective_position(&self) -> Vec3 {
        (
            self.position.0 + self.shake_offset.0,
            self.position.1 + self.shake_offset.1,
            self.position.2 + self.shake_offset.2,
        )
    }

    /// The world → camera-view matrix, row-major.
    #[must_use]
    pub fn view_matrix(&self) -> Mat4 {
        let (right, up, back) = self.basis();
        let pos = self.effective_position();
        [
            right.0, right.1, right.2, -dot(right, pos),
            up.0, up.1, up.2, -dot(up, pos),
            back.0, back.1, back.2, -dot(back, pos),
            0.0, 0.0, 0.0, 1.0,
        ]
    }

    /// The camera-view → clip matrix, row-major, a standard perspective.
    #[must_use]
    pub fn projection_matrix(&self) -> Mat4 {
        let aspect = self.aspect();
        let f = 1.0 / (self.effective_fov().to_radians() / 2.0).tan();
        let (near, far) = (self.near, self.far);
        let mut matrix = [0.0f32; 16];
        matrix[0] = f / aspect;
        matrix[5] = f;
        matrix[10] = -(far + near) / (far - near);
        matrix[11] = -2.0 * far * near / (far - near);
        matrix[14] = -1.0;
        matrix
    }

    /// World → clip, which is projection × view.
    #[must_use]
    pub fn view_projection(&self) -> Mat4 {
        mul(&self.projection_matrix(), &self.view_matrix())
    }

    /// Where a world point lands on screen, as `(x, y, depth)`. `None` when it is behind the
    /// camera. Depth is the normalised device z, the same value [`Camera::screen_to_world`]
    /// takes back.
    #[must_use]
    pub fn world_to_screen(&self, world: Vec3) -> Option<(f32, f32, f32)> {
        let clip = transform(&self.view_projection(), (world.0, world.1, world.2, 1.0));
        if clip.3 <= 0.0 {
            return None;
        }
        let ndc = (
            clip.0 / clip.3,
            clip.1 / clip.3,
            clip.2 / clip.3,
        );
        let (width, height) = (self.viewport.0 as f32, self.viewport.1 as f32);
        Some((
            (ndc.0 * 0.5 + 0.5) * width,
            (1.0 - (ndc.1 * 0.5 + 0.5)) * height,
            ndc.2,
        ))
    }

    /// The world point a screen position and a depth unproject to.
    #[must_use]
    pub fn screen_to_world(&self, x: f32, y: f32, depth: f32) -> Option<Vec3> {
        let inverse = invert(&self.view_projection())?;
        let (width, height) = (self.viewport.0 as f32, self.viewport.1 as f32);
        let ndc_x = x / width * 2.0 - 1.0;
        let ndc_y = 1.0 - y / height * 2.0;
        let world = transform(&inverse, (ndc_x, ndc_y, depth, 1.0));
        if world.3.abs() < EPSILON {
            return None;
        }
        Some((world.0 / world.3, world.1 / world.3, world.2 / world.3))
    }

    /// The six planes of view×projection.
    #[must_use]
    pub fn frustum(&self) -> Frustum {
        let matrix = self.view_projection();
        let row = |index: usize| {
            [
                matrix[index * 4],
                matrix[index * 4 + 1],
                matrix[index * 4 + 2],
                matrix[index * 4 + 3],
            ]
        };
        let (third, first, second) = (row(3), row(0), row(1));
        // An OpenGL-style clip volume: a point is inside when `-w ≤ x,y,z ≤ w`, which turns
        // each plane into a sum or difference of the matrix's last row.
        Frustum {
            planes: [
                plane_of(add4(third, first)),
                plane_of(sub4(third, first)),
                plane_of(add4(third, second)),
                plane_of(sub4(third, second)),
                plane_of(add4(third, row(2))),
                plane_of(sub4(third, row(2))),
            ],
        }
    }

    #[must_use]
    pub fn is_visible(&self, point: Vec3) -> bool {
        self.frustum().contains_point(point)
    }

    #[must_use]
    pub fn is_box_visible(&self, low: Vec3, high: Vec3) -> bool {
        self.frustum().contains_box(low, high)
    }

    /// Puts everything back the way [`Camera::new`] had it.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let forward = self.direction();
        let mut right = normalise(cross(forward, (0.0, 1.0, 0.0)));
        if dot(right, right) < EPSILON {
            right = normalise(cross(forward, (0.0, 0.0, 1.0)));
        }
        let mut up = cross(right, forward);
        if self.rotation.2 != 0.0 {
            let (sin, cos) = self.rotation.2.to_radians().sin_cos();
            right = normalise(add(scale(right, cos), scale(up, sin)));
            up = normalise(add(scale(up, cos), scale(right, -sin)));
        }
        (right, up, scale(forward, -1.0))
    }

    fn recompute_shake(&mut self) {
        if self.shake <= 0.0 {
            self.shake = 0.0;
            self.shake_offset = (0.0, 0.0, 0.0);
            return;
        }
        let phase = self.shake_time * SHAKE_RATE;
        self.shake_offset = (
            self.shake * phase.sin(),
            self.shake * (phase * 1.7 + 1.3).sin(),
            self.shake * (phase * 2.3 + 0.7).sin(),
        );
    }
}

impl fmt::Display for Camera {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "at ({:.2}, {:.2}, {:.2}) yaw {:.1} pitch {:.1} fov {:.0} ({})",
            self.position.0,
            self.position.1,
            self.position.2,
            self.rotation.0,
            self.rotation.1,
            self.fov,
            self.mode
        )
    }
}

/// A direction from a yaw and a pitch in degrees, the way the rest of the crate does it.
#[must_use]
pub fn direction_from_rotation(yaw: f32, pitch: f32) -> Vec3 {
    let yaw = yaw.to_radians();
    let pitch = pitch.to_radians();
    let horizontal = pitch.cos();
    (-yaw.sin() * horizontal, -pitch.sin(), yaw.cos() * horizontal)
}

/// Wraps an angle in degrees into `(-180, 180]`.
#[must_use]
pub fn wrap_degrees(degrees: f32) -> f32 {
    let mut wrapped = degrees % 360.0;
    if wrapped > 180.0 {
        wrapped -= 360.0;
    }
    if wrapped <= -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// The point between two positions.
#[must_use]
pub fn lerp_position(from: Vec3, to: Vec3, t: f32) -> Vec3 {
    let t = t.clamp(0.0, 1.0);
    (
        mix(from.0, to.0, t),
        mix(from.1, to.1, t),
        mix(from.2, to.2, t),
    )
}

/// The rotation between two, taking the short way round on yaw and roll.
#[must_use]
pub fn lerp_rotation(
    from: (f32, f32, f32),
    to: (f32, f32, f32),
    t: f32,
) -> (f32, f32, f32) {
    let t = t.clamp(0.0, 1.0);
    (
        wrap_degrees(from.0 + wrap_degrees(to.0 - from.0) * t),
        mix(from.1, to.1, t).clamp(-90.0, 90.0),
        wrap_degrees(from.2 + wrap_degrees(to.2 - from.2) * t),
    )
}

fn mix(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a.0 * b.0 + a.1 * b.1 + a.2 * b.2
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    (
        a.1 * b.2 - a.2 * b.1,
        a.2 * b.0 - a.0 * b.2,
        a.0 * b.1 - a.1 * b.0,
    )
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    (a.0 + b.0, a.1 + b.1, a.2 + b.2)
}

fn scale(a: Vec3, s: f32) -> Vec3 {
    (a.0 * s, a.1 * s, a.2 * s)
}

fn normalise(a: Vec3) -> Vec3 {
    let length = dot(a, a).sqrt();
    if length < EPSILON {
        (0.0, 0.0, 0.0)
    } else {
        scale(a, 1.0 / length)
    }
}

fn mul(left: &Mat4, right: &Mat4) -> Mat4 {
    let mut out = [0.0f32; 16];
    for row in 0..4 {
        for column in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += left[row * 4 + k] * right[k * 4 + column];
            }
            out[row * 4 + column] = sum;
        }
    }
    out
}

fn transform(matrix: &Mat4, vector: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (x, y, z, w) = vector;
    (
        matrix[0] * x + matrix[1] * y + matrix[2] * z + matrix[3] * w,
        matrix[4] * x + matrix[5] * y + matrix[6] * z + matrix[7] * w,
        matrix[8] * x + matrix[9] * y + matrix[10] * z + matrix[11] * w,
        matrix[12] * x + matrix[13] * y + matrix[14] * z + matrix[15] * w,
    )
}

fn invert(matrix: &Mat4) -> Option<Mat4> {
    let mut a = [[0.0f32; 4]; 4];
    let mut inv = [[0.0f32; 4]; 4];
    for (row, values) in a.iter_mut().enumerate() {
        for (column, value) in values.iter_mut().enumerate() {
            *value = matrix[row * 4 + column];
        }
        inv[row][row] = 1.0;
    }
    for column in 0..4 {
        let mut pivot = column;
        for (row, values) in a.iter().enumerate().skip(column + 1) {
            if values[column].abs() > a[pivot][column].abs() {
                pivot = row;
            }
        }
        if a[pivot][column].abs() < 1e-9 {
            return None;
        }
        a.swap(column, pivot);
        inv.swap(column, pivot);
        let divisor = a[column][column];
        for (value, identity) in a[column].iter_mut().zip(inv[column].iter_mut()) {
            *value /= divisor;
            *identity /= divisor;
        }
        let pivot_row = a[column];
        let identity_row = inv[column];
        for row in 0..4 {
            if row == column {
                continue;
            }
            let factor = a[row][column];
            if factor == 0.0 {
                continue;
            }
            for (value, pivot_value) in a[row].iter_mut().zip(pivot_row.iter()) {
                *value -= factor * pivot_value;
            }
            for (value, pivot_value) in inv[row].iter_mut().zip(identity_row.iter()) {
                *value -= factor * pivot_value;
            }
        }
    }
    let mut out = [0.0f32; 16];
    for (row, values) in inv.iter().enumerate() {
        for (column, value) in values.iter().enumerate() {
            out[row * 4 + column] = *value;
        }
    }
    Some(out)
}

fn add4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn sub4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]]
}

fn plane_of(coefficients: [f32; 4]) -> Plane {
    let length = (coefficients[0] * coefficients[0]
        + coefficients[1] * coefficients[1]
        + coefficients[2] * coefficients[2])
        .sqrt();
    if length < 1e-9 {
        return Plane::default();
    }
    Plane {
        normal: (
            coefficients[0] / length,
            coefficients[1] / length,
            coefficients[2] / length,
        ),
        distance: coefficients[3] / length,
    }
}

fn camera_state() -> &'static Mutex<Camera> {
    static CAMERA: OnceLock<Mutex<Camera>> = OnceLock::new();
    CAMERA.get_or_init(|| Mutex::new(Camera::new()))
}

fn with<T>(body: impl FnOnce(&mut Camera) -> T) -> T {
    let mut guard = camera_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The current camera, as a value.
#[must_use]
pub fn get() -> Camera {
    with(|camera| camera.clone())
}

#[must_use]
pub fn get_position() -> Vec3 {
    with(|camera| camera.position())
}

pub fn set_position(x: f32, y: f32, z: f32) {
    with(|camera| camera.set_position(x, y, z));
}

#[must_use]
pub fn get_rotation() -> (f32, f32, f32) {
    with(|camera| camera.rotation())
}

pub fn set_rotation(yaw: f32, pitch: f32, roll: f32) {
    with(|camera| camera.set_rotation(yaw, pitch, roll));
}

#[must_use]
pub fn get_direction() -> Vec3 {
    with(|camera| camera.direction())
}

#[must_use]
pub fn get_up_vector() -> Vec3 {
    with(|camera| camera.up_vector())
}

#[must_use]
pub fn get_right_vector() -> Vec3 {
    with(|camera| camera.right_vector())
}

#[must_use]
pub fn get_fov() -> f32 {
    with(|camera| camera.fov())
}

pub fn set_fov(fov: f32) {
    with(|camera| camera.set_fov(fov));
}

#[must_use]
pub fn get_near_plane() -> f32 {
    with(|camera| camera.near_plane())
}

#[must_use]
pub fn get_far_plane() -> f32 {
    with(|camera| camera.far_plane())
}

#[must_use]
pub fn get_view_matrix() -> Mat4 {
    with(|camera| camera.view_matrix())
}

#[must_use]
pub fn get_projection_matrix() -> Mat4 {
    with(|camera| camera.projection_matrix())
}

#[must_use]
pub fn get_camera_mode() -> CameraMode {
    with(|camera| camera.mode())
}

pub fn set_camera_mode(mode: CameraMode) {
    with(|camera| camera.set_mode(mode));
}

#[must_use]
pub fn get_entity() -> Option<String> {
    with(|camera| camera.entity())
}

pub fn set_entity(id: &str) {
    with(|camera| camera.set_entity(id));
}

#[must_use]
pub fn get_target() -> Option<String> {
    with(|camera| camera.target())
}

pub fn set_target(id: &str) {
    with(|camera| camera.set_target(id));
}

#[must_use]
pub fn get_zoom() -> f32 {
    with(|camera| camera.zoom())
}

pub fn set_zoom(zoom: f32) {
    with(|camera| camera.set_zoom(zoom));
}

pub fn shake(amount: f32) {
    with(|camera| camera.shake(amount));
}

pub fn set_shake(amount: f32) {
    with(|camera| camera.set_shake(amount));
}

#[must_use]
pub fn get_shake() -> f32 {
    with(|camera| camera.shake_amplitude())
}

#[must_use]
pub fn shake_offset() -> Vec3 {
    with(|camera| camera.shake_offset())
}

pub fn advance(dt_millis: u32) {
    with(|camera| camera.advance(dt_millis));
}

pub fn look_at(target: Vec3) -> (f32, f32, f32) {
    with(|camera| camera.look_at(target))
}

#[must_use]
pub fn world_to_screen(world: Vec3) -> Option<(f32, f32, f32)> {
    with(|camera| camera.world_to_screen(world))
}

#[must_use]
pub fn screen_to_world(x: f32, y: f32, depth: f32) -> Option<Vec3> {
    with(|camera| camera.screen_to_world(x, y, depth))
}

#[must_use]
pub fn get_frustum() -> Frustum {
    with(|camera| camera.frustum())
}

#[must_use]
pub fn is_visible(point: Vec3) -> bool {
    with(|camera| camera.is_visible(point))
}

#[must_use]
pub fn is_box_visible(low: Vec3, high: Vec3) -> bool {
    with(|camera| camera.is_box_visible(low, high))
}

pub fn set_third_person_distance(distance: f32) {
    with(|camera| camera.set_third_person_distance(distance));
}

#[must_use]
pub fn get_third_person_distance() -> f32 {
    with(|camera| camera.third_person_distance())
}

pub fn set_bobbing(bobbing: bool) {
    with(|camera| camera.set_bobbing(bobbing));
}

pub fn set_roll(roll: f32) {
    with(|camera| camera.set_roll(roll));
}

pub fn set_free_camera(free: bool) {
    with(|camera| camera.set_free_camera(free));
}

/// Puts the current camera back to its defaults.
pub fn reset() {
    with(|camera| camera.reset());
}

pub fn set_viewport(width: u32, height: u32) {
    with(|camera| camera.set_viewport(width, height));
}

fn modifiers() -> &'static Mutex<BTreeMap<String, String>> {
    static MODIFIERS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    MODIFIERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn controllers() -> &'static Mutex<BTreeMap<String, String>> {
    static CONTROLLERS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    CONTROLLERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Records that a mod ships a camera modifier. Nothing here calls it: the client's renderer
/// would, and it is out of reach.
pub fn register_modifier(name: &str) -> bool {
    let mut guard = modifiers().lock().unwrap_or_else(|error| error.into_inner());
    guard
        .insert(name.to_owned(), crate::metadata::id())
        .is_none()
}

/// Forgets one.
pub fn unregister_modifier(name: &str) -> bool {
    let mut guard = modifiers().lock().unwrap_or_else(|error| error.into_inner());
    guard.remove(name).is_some()
}

/// The modifiers a mod registered.
#[must_use]
pub fn get_modifiers() -> Vec<String> {
    let guard = modifiers().lock().unwrap_or_else(|error| error.into_inner());
    guard.keys().cloned().collect()
}

/// Records that a mod ships a camera controller, which the client's camera path would drive.
/// Nothing here calls it.
pub fn register_camera_controller(name: &str) -> bool {
    let mut guard = controllers().lock().unwrap_or_else(|error| error.into_inner());
    guard
        .insert(name.to_owned(), crate::metadata::id())
        .is_none()
}

/// The controllers a mod registered.
#[must_use]
pub fn get_camera_controllers() -> Vec<String> {
    let guard = controllers().lock().unwrap_or_else(|error| error.into_inner());
    guard.keys().cloned().collect()
}

/// Why a question about the client's camera has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "position" => {
            "the client's camera position is its own, interpolated from the local player's \
             state"
                .to_owned()
        }
        "rotation" => {
            "the client's camera rotation is its own, taken from the player's look input"
                .to_owned()
        }
        "mode" => {
            "which person the client's camera is in is the client's own view setting".to_owned()
        }
        "entity" => {
            "the entity the client's camera is attached to belongs to the client's camera"
                .to_owned()
        }
        "fov" => "the client's field of view comes from its own video settings".to_owned(),
        "shake" => {
            "the shake the client applies to its camera happens inside the client's own \
             frame"
                .to_owned()
        }
        "modifier" => {
            "a camera modifier would be called by the client's camera as it builds its view; \
             nothing here calls one"
                .to_owned()
        }
        "controller" => {
            "a camera controller would be driven by the client's camera path; nothing here \
             calls one"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["position", "rotation", "mode", "entity", "fov", "shake", "modifier", "controller"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused<T: Default>(what: &str) -> T {
    crate::warn!(
        "a mod asked this host about the client's camera: {}",
        why_unknown(what)
    );
    T::default()
}

/// The client's camera position. `None`.
#[must_use]
pub fn client_position() -> Option<Vec3> {
    refused("position")
}

/// The client's camera rotation. `None`.
#[must_use]
pub fn client_rotation() -> Option<(f32, f32, f32)> {
    refused("rotation")
}

/// The client's camera mode. `None`.
#[must_use]
pub fn client_mode() -> Option<CameraMode> {
    refused("mode")
}

/// The entity the client's camera is attached to. `None`.
#[must_use]
pub fn client_entity() -> Option<String> {
    refused("entity")
}

/// The client's field of view. `None`.
#[must_use]
pub fn client_fov() -> Option<f32> {
    refused("fov")
}

/// The shake the client is applying. `None`.
#[must_use]
pub fn client_shake() -> Option<f32> {
    refused("shake")
}

/// Moves the client's camera. `false`.
pub fn set_client_position(_x: f32, _y: f32, _z: f32) -> bool {
    refused("position")
}

/// Turns the client's camera. `false`.
pub fn set_client_rotation(_yaw: f32, _pitch: f32, _roll: f32) -> bool {
    refused("rotation")
}

/// Sets the client's camera mode. `false`.
pub fn set_client_mode(_mode: CameraMode) -> bool {
    refused("mode")
}

/// Sets the client's field of view. `false`.
pub fn set_client_fov(_fov: f32) -> bool {
    refused("fov")
}

/// Shakes the client's camera. `false`.
pub fn shake_client(_amount: f32) -> bool {
    refused("shake")
}

/// Runs a registered modifier on the client's camera. `false`.
pub fn call_modifier(_name: &str) -> bool {
    refused("modifier")
}

/// Runs a registered controller on the client's camera. `false`.
pub fn call_controller(_name: &str) -> bool {
    refused("controller")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        reset();
        modifiers().lock().unwrap().clear();
        controllers().lock().unwrap().clear();
    }

    #[test]
    fn a_ninety_degree_yaw_gives_a_right_handed_basis() {
        let _serial = serial();
        let mut cam = Camera::new();
        cam.set_rotation(90.0, 0.0, 0.0);
        let direction = cam.direction();
        assert!((direction.0 + 1.0).abs() < 1e-5, "{direction:?}");
        assert!(direction.1.abs() < 1e-5 && direction.2.abs() < 1e-5, "{direction:?}");
        let right = cam.right_vector();
        assert!(right.0.abs() < 1e-5 && right.1.abs() < 1e-5, "{right:?}");
        assert!((right.2 + 1.0).abs() < 1e-5, "{right:?}");
        let up = cam.up_vector();
        assert!((up.1 - 1.0).abs() < 1e-5, "{up:?}");
        assert!(dot(direction, right).abs() < 1e-5, "the basis is orthogonal");
        let recomputed = cross(right, direction);
        assert!((recomputed.1 - 1.0).abs() < 1e-5, "{recomputed:?}");
    }

    #[test]
    fn look_at_points_at_the_target_and_handles_straight_up() {
        let _serial = serial();
        let mut cam = Camera::new();
        cam.set_position(0.0, 0.0, 0.0);
        cam.look_at((0.0, 0.0, -1.0));
        let direction = cam.direction();
        assert!((direction.2 + 1.0).abs() < 1e-4, "{direction:?}");
        assert!(direction.0.abs() < 1e-4, "{direction:?}");
        assert!((cam.rotation().0 - 180.0).abs() < 1e-3, "{:?}", cam.rotation());

        cam.look_at((0.0, 5.0, 0.0));
        let direction = cam.direction();
        assert!(direction.1 > 0.999, "straight up: {direction:?}");
        assert!(direction.0.abs() < 1e-4 && direction.2.abs() < 1e-4, "{direction:?}");
        assert!((cam.rotation().1 + 90.0).abs() < 1e-3, "{:?}", cam.rotation());
    }

    #[test]
    fn a_point_in_front_projects_and_one_behind_does_not() {
        let _serial = serial();
        let mut cam = Camera::new();
        cam.set_rotation(180.0, 0.0, 0.0);
        cam.set_position(0.0, 0.0, 0.0);
        let (center_x, center_y, _) = cam.world_to_screen((0.0, 0.0, -10.0)).expect("in front");
        assert!((center_x - 960.0).abs() < 1e-2, "{center_x}");
        assert!((center_y - 540.0).abs() < 1e-2, "{center_y}");
        assert!(cam.world_to_screen((0.0, 0.0, 10.0)).is_none(), "behind the camera");

        let right = cam.world_to_screen((10.0, 0.0, -10.0)).expect("in front");
        assert!(right.0 > 960.0, "x grows to the right: {right:?}");
        let above = cam.world_to_screen((0.0, 10.0, -10.0)).expect("in front");
        assert!(above.1 < 540.0, "y grows downward on screen: {above:?}");
    }

    #[test]
    fn a_point_projects_and_unprojects_to_itself() {
        let _serial = serial();
        let mut cam = Camera::new();
        cam.set_rotation(180.0, 0.0, 0.0);
        cam.set_position(3.0, 4.0, 5.0);
        let world = (7.0, 6.0, -5.0);
        let (x, y, depth) = cam.world_to_screen(world).expect("in front");
        let back = cam.screen_to_world(x, y, depth).expect("it unprojects");
        assert!((back.0 - world.0).abs() < 1e-2, "{back:?}");
        assert!((back.1 - world.1).abs() < 1e-2, "{back:?}");
        assert!((back.2 - world.2).abs() < 1e-2, "{back:?}");
    }

    #[test]
    fn the_frustum_keeps_what_is_inside_and_rejects_what_is_not() {
        let _serial = serial();
        let mut cam = Camera::new();
        cam.set_rotation(180.0, 0.0, 0.0);
        cam.set_position(0.0, 0.0, 0.0);
        let frustum = cam.frustum();
        assert_eq!(frustum.planes.len(), 6);
        assert!(frustum.contains_point((0.0, 0.0, -10.0)));
        assert!(!frustum.contains_point((0.0, 0.0, 10.0)), "behind the near plane");
        assert!(!frustum.contains_point((100.0, 0.0, -1.0)), "outside the field of view");
        assert!(cam.is_visible((0.0, 0.0, -10.0)));
        assert!(frustum.contains_box((-1.0, -1.0, -11.0), (1.0, 1.0, -9.0)));
        assert!(!frustum.contains_box((90.0, -1.0, -11.0), (110.0, 1.0, -9.0)));
    }

    #[test]
    fn shake_accumulates_and_decays_to_zero() {
        let _serial = serial();
        let mut cam = Camera::new();
        cam.shake(1.0);
        cam.shake(0.5);
        assert!((cam.shake_amplitude() - 1.5).abs() < 1e-6, "{}", cam.shake_amplitude());
        cam.set_shake(1.0);
        let mut steps = 0;
        while cam.shake_amplitude() > 0.0 && steps < 1000 {
            cam.advance(100);
            steps += 1;
        }
        assert_eq!(cam.shake_amplitude(), 0.0);
        assert_eq!(cam.shake_offset(), (0.0, 0.0, 0.0));
        assert!(steps < 1000, "decay took {steps} steps");
    }

    #[test]
    fn lerp_takes_the_short_way_round_on_yaw() {
        let _serial = serial();
        let rotation = lerp_rotation((350.0, 0.0, 0.0), (10.0, 0.0, 0.0), 0.5);
        assert!(rotation.0.abs() < 1e-3, "through zero, not through 180: {rotation:?}");
        let rotation = lerp_rotation((0.0, 0.0, 0.0), (90.0, 45.0, 0.0), 0.5);
        assert!((rotation.0 - 45.0).abs() < 1e-3, "{rotation:?}");
        assert!((rotation.1 - 22.5).abs() < 1e-3, "{rotation:?}");
        let position = lerp_position((0.0, 0.0, 0.0), (10.0, 20.0, 30.0), 0.25);
        assert_eq!(position, (2.5, 5.0, 7.5));
    }

    #[test]
    fn zoom_narrows_the_view_and_reset_puts_it_back() {
        let _serial = serial();
        let mut cam = Camera::new();
        cam.set_rotation(180.0, 0.0, 0.0);
        assert_eq!(cam.mode(), CameraMode::FirstPerson);
        cam.set_mode(CameraMode::ThirdPerson);
        assert_eq!(cam.mode(), CameraMode::ThirdPerson);
        cam.set_free_camera(true);
        assert_eq!(cam.mode(), CameraMode::Free);
        cam.set_free_camera(false);
        assert_eq!(cam.mode(), CameraMode::FirstPerson);
        cam.set_third_person_distance(6.0);
        assert_eq!(cam.third_person_distance(), 6.0);
        cam.set_bobbing(false);
        assert!(!cam.bobbing());
        cam.set_roll(30.0);
        assert_eq!(cam.rotation().2, 30.0);

        let mut zoomed = cam.clone();
        zoomed.set_zoom(2.0);
        assert!((zoomed.effective_fov() - 35.0).abs() < 1e-3, "{}", zoomed.effective_fov());
        let plain_x = cam.world_to_screen((10.0, 0.0, -10.0)).expect("in front").0;
        let zoomed_x = zoomed.world_to_screen((10.0, 0.0, -10.0)).expect("in front").0;
        assert!(zoomed_x > plain_x, "the zoom pushes the point out: {zoomed_x} > {plain_x}");

        cam.reset();
        assert_eq!(cam.position(), (0.0, 0.0, 0.0));
        assert_eq!(cam.rotation(), (0.0, 0.0, 0.0));
        assert_eq!(cam.mode(), CameraMode::FirstPerson);
        assert_eq!(cam.fov(), DEFAULT_FOV);
        assert_eq!(cam.near_plane(), DEFAULT_NEAR);
        assert_eq!(cam.far_plane(), DEFAULT_FAR);
    }

    #[test]
    fn the_process_wide_camera_and_registrations_are_real_data() {
        let _serial = serial();
        fresh();
        reset();
        set_position(1.0, 2.0, 3.0);
        set_rotation(180.0, 0.0, 0.0);
        assert_eq!(get_position(), (1.0, 2.0, 3.0));
        assert_eq!(get().position(), (1.0, 2.0, 3.0));
        assert!(get_view_matrix() != [0.0; 16]);
        assert_ne!(get_projection_matrix(), [0.0; 16]);
        assert!(is_visible((1.0, 2.0, -7.0)));
        assert!(world_to_screen((1.0, 2.0, -7.0)).is_some());
        set_entity("minecraft:pig");
        assert_eq!(get_entity(), Some("minecraft:pig".to_owned()));
        set_target("minecraft:cow");
        assert_eq!(get_target(), Some("minecraft:cow".to_owned()));

        assert!(register_modifier("hello:cam"));
        assert!(!register_modifier("hello:cam"), "it replaces");
        assert!(unregister_modifier("hello:cam"));
        assert!(!unregister_modifier("hello:cam"));
        assert!(register_camera_controller("hello:orbit"));
        assert!(get_camera_controllers().contains(&"hello:orbit".to_owned()));
        reset();
    }

    #[test]
    fn every_refusal_answers_false_or_none_with_a_reason() {
        let _serial = serial();
        assert!(client_position().is_none());
        assert!(client_rotation().is_none());
        assert!(client_mode().is_none());
        assert!(client_entity().is_none());
        assert!(client_fov().is_none());
        assert!(client_shake().is_none());
        assert!(!set_client_position(0.0, 0.0, 0.0));
        assert!(!set_client_rotation(0.0, 0.0, 0.0));
        assert!(!set_client_mode(CameraMode::ThirdPerson));
        assert!(!set_client_fov(90.0));
        assert!(!shake_client(1.0));
        assert!(!call_modifier("hello:cam"));
        assert!(!call_controller("hello:orbit"));
        assert_eq!(refusals().len(), 8);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }
}
