//! Math and geometry: vectors, matrices, rotations, curves, noise, the RNG and intersections.
//!
//! This section has **no client-owned surface**: it is arithmetic a mod runs on numbers it
//! already has, so nothing here is refused and nothing pretends to be. Every value is real.
//!
//! * **Vectors.** [`Vec2`], [`Vec3`], [`Vec4`], [`IVec2`], [`IVec3`] with the operators and the
//!   free functions [`dot`], [`cross`], [`normalize`], [`distance`], [`angle_between`],
//!   [`direction_to`], [`reflect`], [`refract`], [`project`] and [`closest_point`]. A
//!   zero-length [`normalize`] answers the zero vector rather than dividing by zero.
//! * **A 4×4 matrix.** [`Matrix4`] (and [`Matrix3`]) is **row-major with column vectors**, so a
//!   point transforms as `v' = M·v` and translation sits in the last column. Translation,
//!   rotation, scale, transpose, determinant and a real [`Matrix4::inverse`] that answers
//!   `None` for a singular matrix. [`Matrix4::transform_point`] moves a point, while
//!   [`Matrix4::transform_direction`] and [`Matrix4::transform_vector`] ignore the translation.
//! * **Rotations.** [`EulerAngles`] in degrees converts both ways with a direction vector —
//!   yaw grows clockwise and `yaw = 90°` faces `-X`, the crate's convention — plus
//!   [`wrap_angle`] into `(-180, 180]`, [`normalize_angle`] into `[0, 360)` and
//!   [`lerp_angle`] taking the short way across the wrap (`350° → 10°` passes through `0°`).
//!   [`Quaternion`] multiplies, conjugates, rotates and [`Quaternion::slerp`]s along the
//!   shortest arc.
//! * **Curves.** [`clamp`], [`lerp`], [`inverse_lerp`], [`remap`]/[`map_range`], [`smoothstep`],
//!   [`step`] and a documented easing list driven by [`ease`] over [`Easing`] (linear, quad,
//!   cubic, sine, back, bounce and elastic variants).
//! * **Noise.** [`value_noise_2d`] and [`gradient_noise_2d`] on a unit integer lattice — a
//!   hash of the lattice point and the seed, smoothstep-interpolated — deterministic for a
//!   seed, in `[-1, 1]` and continuous, with [`fbm_2d`] layering octaves.
//! * **An RNG.** [`Rng`] is splitmix64 with `next_u32`, `next_u64`, `next_f64`, `range`,
//!   `integer` and `chance`; [`seeded_random`] reproduces a sequence exactly, and [`random`]
//!   and friends draw from a process-wide seeded generator.
//! * **Intersections.** ray–sphere ([`intersect_ray`]), ray–box ([`intersect_aabb`]),
//!   ray–plane ([`intersect_plane`]), ray–triangle ([`intersect_triangle`]), segment–segment
//!   in 2D ([`intersect_segments_2d`]) and sphere–sphere ([`intersect_sphere`]), each a real
//!   [`RayHit`] with a point and a normal or nothing.
//! * **Boxes.** [`Aabb`] union, intersection, contains, expand, centre and size.
//!
//! ```
//! use cinnaloader::math::{self, Mat4, Vec3};
//!
//! assert_eq!(math::lerp(0.0, 10.0, 0.25), 2.5);
//! assert_eq!(math::clamp(7.0, 0.0, 3.0), 3.0);
//! assert_eq!(math::wrap_angle(350.0), -10.0);
//!
//! let a = Vec3::new(1.0, 0.0, 0.0);
//! let b = Vec3::new(0.0, 1.0, 0.0);
//! assert_eq!(a.cross(b), Vec3::new(0.0, 0.0, 1.0));
//!
//! let m = Mat4::translation(1.0, 2.0, 3.0) * Mat4::scale(2.0, 2.0, 2.0);
//! let inverse = m.inverse().expect("non-singular");
//! assert!(m.multiply(&inverse).approx_equal(&Mat4::identity(), 1e-9));
//! assert!(Mat4::scale(0.0, 1.0, 1.0).inverse().is_none(), "singular");
//! ```

use std::ops::{Add, Mul, Neg, Sub};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Every tolerance in this module is stated against this epsilon.
pub const EPSILON: f64 = 1e-9;

/// A 2D vector of `f64`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    #[must_use]
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    #[must_use]
    pub fn cross(self, other: Self) -> f64 {
        self.x * other.y - self.y * other.x
    }

    #[must_use]
    pub fn length(self) -> f64 {
        self.x.hypot(self.y)
    }

    #[must_use]
    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    #[must_use]
    pub fn normalize(self) -> Self {
        let length = self.length();
        if length < EPSILON {
            Self::ZERO
        } else {
            Self::new(self.x / length, self.y / length)
        }
    }

    /// Rotated a quarter turn anticlockwise.
    #[must_use]
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    #[must_use]
    pub fn distance(self, other: Self) -> f64 {
        (self - other).length()
    }

    #[must_use]
    pub fn lerp(self, other: Self, t: f64) -> Self {
        Self::new(lerp(self.x, other.x, t), lerp(self.y, other.y, t))
    }

    #[must_use]
    pub fn angle_between(self, other: Self) -> f64 {
        let denom = self.length() * other.length();
        if denom < EPSILON {
            0.0
        } else {
            (self.dot(other) / denom).clamp(-1.0, 1.0).acos()
        }
    }

    #[must_use]
    pub fn approx_eq(self, other: Self, epsilon: f64) -> bool {
        (self.x - other.x).abs() <= epsilon && (self.y - other.y).abs() <= epsilon
    }
}

impl Add for Vec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f64> for Vec2 {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Mul<Vec2> for f64 {
    type Output = Vec2;

    fn mul(self, rhs: Vec2) -> Vec2 {
        rhs * self
    }
}

impl Neg for Vec2 {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

/// A 3D vector of `f64`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };
    pub const ONE: Self = Self { x: 1.0, y: 1.0, z: 1.0 };
    pub const X: Self = Self { x: 1.0, y: 0.0, z: 0.0 };
    pub const Y: Self = Self { x: 0.0, y: 1.0, z: 0.0 };
    pub const Z: Self = Self { x: 0.0, y: 0.0, z: 1.0 };
    pub const UP: Self = Self { x: 0.0, y: 1.0, z: 0.0 };

    #[must_use]
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub fn splat(value: f64) -> Self {
        Self::new(value, value, value)
    }

    #[must_use]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    #[must_use]
    pub fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    #[must_use]
    pub fn length(self) -> f64 {
        self.length_squared().sqrt()
    }

    #[must_use]
    pub fn length_squared(self) -> f64 {
        self.dot(self)
    }

    #[must_use]
    pub fn normalize(self) -> Self {
        let length = self.length();
        if length < EPSILON {
            Self::ZERO
        } else {
            self * (1.0 / length)
        }
    }

    #[must_use]
    pub fn distance(self, other: Self) -> f64 {
        (self - other).length()
    }

    #[must_use]
    pub fn distance_squared(self, other: Self) -> f64 {
        (self - other).length_squared()
    }

    #[must_use]
    pub fn lerp(self, other: Self, t: f64) -> Self {
        Self::new(
            lerp(self.x, other.x, t),
            lerp(self.y, other.y, t),
            lerp(self.z, other.z, t),
        )
    }

    #[must_use]
    pub fn angle_between(self, other: Self) -> f64 {
        let denom = self.length() * other.length();
        if denom < EPSILON {
            0.0
        } else {
            (self.dot(other) / denom).clamp(-1.0, 1.0).acos()
        }
    }

    #[must_use]
    pub fn approx_eq(self, other: Self, epsilon: f64) -> bool {
        (self.x - other.x).abs() <= epsilon
            && (self.y - other.y).abs() <= epsilon
            && (self.z - other.z).abs() <= epsilon
    }

    #[must_use]
    pub fn min(self, other: Self) -> Self {
        Self::new(self.x.min(other.x), self.y.min(other.y), self.z.min(other.z))
    }

    #[must_use]
    pub fn max(self, other: Self) -> Self {
        Self::new(self.x.max(other.x), self.y.max(other.y), self.z.max(other.z))
    }

    #[must_use]
    pub fn abs(self) -> Self {
        Self::new(self.x.abs(), self.y.abs(), self.z.abs())
    }

    #[must_use]
    pub fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
}

impl Add for Vec3 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl Mul<Vec3> for f64 {
    type Output = Vec3;

    fn mul(self, rhs: Vec3) -> Vec3 {
        rhs * self
    }
}

impl Neg for Vec3 {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// A 4D vector of `f64`, for matrices and homogeneous coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec4 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Vec4 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 0.0 };

    #[must_use]
    pub fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Self { x, y, z, w }
    }

    #[must_use]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    #[must_use]
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    #[must_use]
    pub fn normalize(self) -> Self {
        let length = self.length();
        if length < EPSILON {
            Self::ZERO
        } else {
            self * (1.0 / length)
        }
    }

    #[must_use]
    pub fn trinion(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }
}

impl Add for Vec4 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z, self.w + rhs.w)
    }
}

impl Sub for Vec4 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z, self.w - rhs.w)
    }
}

impl Mul<f64> for Vec4 {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs, self.w * rhs)
    }
}

/// A 2D vector of `i32`, for grid and pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct IVec2 {
    pub x: i32,
    pub y: i32,
}

impl IVec2 {
    #[must_use]
    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

impl Add for IVec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for IVec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

/// A 3D vector of `i32`, for a block coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct IVec3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl IVec3 {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}

impl Add for IVec3 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for IVec3 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

/// A rotation in degrees: yaw around the vertical, then pitch, then roll.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EulerAngles {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

impl EulerAngles {
    #[must_use]
    pub fn new(yaw: f64, pitch: f64, roll: f64) -> Self {
        Self { yaw, pitch, roll }
    }

    /// The direction the angles point along, yaw clockwise with `90°` facing `-X`.
    #[must_use]
    pub fn to_direction(self) -> Vec3 {
        let yaw = self.yaw.to_radians();
        let pitch = self.pitch.to_radians();
        let horizontal = pitch.cos();
        Vec3::new(
            -yaw.sin() * horizontal,
            -pitch.sin(),
            yaw.cos() * horizontal,
        )
    }

    /// The inverse of [`EulerAngles::to_direction`], with roll set to zero.
    #[must_use]
    pub fn from_direction(direction: Vec3) -> Self {
        let n = direction.normalize();
        if n == Vec3::ZERO {
            return Self::default();
        }
        let yaw = (-n.x).atan2(n.z).to_degrees();
        let pitch = (-n.y).clamp(-1.0, 1.0).asin().to_degrees();
        Self::new(yaw, pitch, 0.0)
    }

    #[must_use]
    pub fn wrapped(self) -> Self {
        Self::new(wrap_angle(self.yaw), self.pitch, wrap_angle(self.roll))
    }

    #[must_use]
    pub fn to_quaternion(self) -> Quaternion {
        Quaternion::from_euler(self)
    }
}

/// A unit quaternion for a rotation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Quaternion {
    #[must_use]
    pub fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Self { x, y, z, w }
    }

    #[must_use]
    pub fn identity() -> Self {
        Self::new(0.0, 0.0, 0.0, 1.0)
    }

    #[must_use]
    pub fn from_axis_angle(axis: Vec3, radians: f64) -> Self {
        let n = axis.normalize();
        let half = radians / 2.0;
        let (sin, cos) = half.sin_cos();
        Self::new(n.x * sin, n.y * sin, n.z * sin, cos)
    }

    /// From euler degrees, the same direction convention as [`EulerAngles::to_direction`].
    #[must_use]
    pub fn from_euler(angles: EulerAngles) -> Self {
        let yaw = Quaternion::from_axis_angle(Vec3::Y, -angles.yaw.to_radians());
        let pitch = Quaternion::from_axis_angle(Vec3::X, angles.pitch.to_radians());
        let roll = Quaternion::from_axis_angle(Vec3::Z, angles.roll.to_radians());
        yaw.multiply(pitch).multiply(roll).normalize()
    }

    #[must_use]
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    #[must_use]
    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    #[must_use]
    pub fn conjugate(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, self.w)
    }

    #[must_use]
    pub fn normalize(self) -> Self {
        let length = self.length();
        if length < EPSILON {
            Self::identity()
        } else {
            Self::new(self.x / length, self.y / length, self.z / length, self.w / length)
        }
    }

    #[must_use]
    pub fn multiply(self, other: Self) -> Self {
        let (a, b) = (self, other);
        Self::new(
            a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
            a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
            a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
            a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        )
    }

    /// Rotates a vector by this quaternion.
    #[must_use]
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let uv = u.cross(v);
        v + u.cross(uv) * 2.0 + uv * (2.0 * self.w)
    }

    /// The shortest-arc spherical interpolation to `other`.
    #[must_use]
    pub fn slerp(self, other: Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        let mut to = other;
        let mut cos = self.dot(other);
        if cos < 0.0 {
            to = -to;
            cos = -cos;
        }
        if cos > 0.9995 {
            return Self::new(
                lerp(self.x, to.x, t),
                lerp(self.y, to.y, t),
                lerp(self.z, to.z, t),
                lerp(self.w, to.w, t),
            )
            .normalize();
        }
        let theta = cos.clamp(-1.0, 1.0).acos();
        let sin = theta.sin();
        let a = ((1.0 - t) * theta).sin() / sin;
        let b = (t * theta).sin() / sin;
        Self::new(
            self.x * a + to.x * b,
            self.y * a + to.y * b,
            self.z * a + to.z * b,
            self.w * a + to.w * b,
        )
        .normalize()
    }

    /// Back to euler degrees; the yaw and pitch invert [`EulerAngles`], the roll follows the
    /// rotated `+X`.
    #[must_use]
    pub fn to_euler(self) -> EulerAngles {
        let forward = self.rotate(Vec3::Z);
        let angles = EulerAngles::from_direction(forward);
        let right = self.rotate(Vec3::X);
        let flat = Vec3::new(right.x, 0.0, right.z);
        let roll = if flat.length() < EPSILON {
            0.0
        } else {
            let base = Vec3::new(angles.yaw.to_radians().cos(), 0.0, -angles.yaw.to_radians().sin());
            f64::atan2(base.cross(flat).y, base.dot(flat)).to_degrees()
        };
        EulerAngles::new(angles.yaw, angles.pitch, roll)
    }

    #[must_use]
    pub fn approx_eq(self, other: Self, epsilon: f64) -> bool {
        (self.x - other.x).abs() <= epsilon
            && (self.y - other.y).abs() <= epsilon
            && (self.z - other.z).abs() <= epsilon
            && (self.w - other.w).abs() <= epsilon
    }
}

impl Neg for Quaternion {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, -self.w)
    }
}

/// A 3×3 matrix, row-major with column vectors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix3(pub [f64; 9]);

impl Default for Matrix3 {
    fn default() -> Self {
        Self::identity()
    }
}

impl Matrix3 {
    #[must_use]
    pub fn identity() -> Self {
        Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
    }

    #[must_use]
    pub fn from_rows(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Self {
        Self([a[0], a[1], a[2], b[0], b[1], b[2], c[0], c[1], c[2]])
    }

    #[must_use]
    pub fn multiply(&self, other: &Self) -> Self {
        let mut out = [0.0; 9];
        for row in 0..3 {
            for column in 0..3 {
                let mut sum = 0.0;
                for k in 0..3 {
                    sum += self.0[row * 3 + k] * other.0[k * 3 + column];
                }
                out[row * 3 + column] = sum;
            }
        }
        Self(out)
    }

    #[must_use]
    pub fn transpose(&self) -> Self {
        let mut out = [0.0; 9];
        for row in 0..3 {
            for column in 0..3 {
                out[row * 3 + column] = self.0[column * 3 + row];
            }
        }
        Self(out)
    }

    #[must_use]
    pub fn determinant(&self) -> f64 {
        let m = &self.0;
        m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6])
    }

    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        let m = &self.0;
        let det = self.determinant();
        if det.abs() < EPSILON {
            return None;
        }
        let inv = [
            (m[4] * m[8] - m[5] * m[7]) / det,
            (m[2] * m[7] - m[1] * m[8]) / det,
            (m[1] * m[5] - m[2] * m[4]) / det,
            (m[5] * m[6] - m[3] * m[8]) / det,
            (m[0] * m[8] - m[2] * m[6]) / det,
            (m[2] * m[3] - m[0] * m[5]) / det,
            (m[3] * m[7] - m[4] * m[6]) / det,
            (m[1] * m[6] - m[0] * m[7]) / det,
            (m[0] * m[4] - m[1] * m[3]) / det,
        ];
        Some(Self(inv))
    }

    #[must_use]
    pub fn transform(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.0[0] * v.x + self.0[1] * v.y + self.0[2] * v.z,
            self.0[3] * v.x + self.0[4] * v.y + self.0[5] * v.z,
            self.0[6] * v.x + self.0[7] * v.y + self.0[8] * v.z,
        )
    }
}

impl Mul for Matrix3 {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        self.multiply(&rhs)
    }
}

/// A 4×4 matrix, row-major with column vectors: a point transforms as `v' = M·v` and the
/// translation sits in the last column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix4(pub [f64; 16]);

/// A short name for [`Matrix4`].
pub type Mat4 = Matrix4;

impl Default for Matrix4 {
    fn default() -> Self {
        Self::identity()
    }
}

impl Matrix4 {
    #[must_use]
    pub fn identity() -> Self {
        let mut m = [0.0; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Self(m)
    }

    #[must_use]
    pub fn translation(x: f64, y: f64, z: f64) -> Self {
        let mut m = Self::identity();
        m.0[3] = x;
        m.0[7] = y;
        m.0[11] = z;
        m
    }

    #[must_use]
    pub fn scale(x: f64, y: f64, z: f64) -> Self {
        let mut m = Self::identity();
        m.0[0] = x;
        m.0[5] = y;
        m.0[10] = z;
        m
    }

    #[must_use]
    pub fn rotation_x(radians: f64) -> Self {
        let (sin, cos) = radians.sin_cos();
        let mut m = Self::identity();
        m.0[5] = cos;
        m.0[6] = -sin;
        m.0[9] = sin;
        m.0[10] = cos;
        m
    }

    #[must_use]
    pub fn rotation_y(radians: f64) -> Self {
        let (sin, cos) = radians.sin_cos();
        let mut m = Self::identity();
        m.0[0] = cos;
        m.0[2] = sin;
        m.0[8] = -sin;
        m.0[10] = cos;
        m
    }

    #[must_use]
    pub fn rotation_z(radians: f64) -> Self {
        let (sin, cos) = radians.sin_cos();
        let mut m = Self::identity();
        m.0[0] = cos;
        m.0[1] = -sin;
        m.0[4] = sin;
        m.0[5] = cos;
        m
    }

    #[must_use]
    pub fn rotation_axis(axis: Vec3, radians: f64) -> Self {
        Self::from_quaternion(Quaternion::from_axis_angle(axis, radians))
    }

    /// From euler degrees, matching [`EulerAngles`] and [`Quaternion::from_euler`].
    #[must_use]
    pub fn from_euler(angles: EulerAngles) -> Self {
        Self::from_quaternion(Quaternion::from_euler(angles))
    }

    #[must_use]
    pub fn from_quaternion(q: Quaternion) -> Self {
        let q = q.normalize();
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        let mut m = [0.0; 16];
        m[0] = 1.0 - 2.0 * (y * y + z * z);
        m[1] = 2.0 * (x * y - z * w);
        m[2] = 2.0 * (x * z + y * w);
        m[4] = 2.0 * (x * y + z * w);
        m[5] = 1.0 - 2.0 * (x * x + z * z);
        m[6] = 2.0 * (y * z - x * w);
        m[8] = 2.0 * (x * z - y * w);
        m[9] = 2.0 * (y * z + x * w);
        m[10] = 1.0 - 2.0 * (x * x + y * y);
        m[15] = 1.0;
        Self(m)
    }

    #[must_use]
    pub fn multiply(&self, other: &Self) -> Self {
        let mut out = [0.0; 16];
        for row in 0..4 {
            for column in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += self.0[row * 4 + k] * other.0[k * 4 + column];
                }
                out[row * 4 + column] = sum;
            }
        }
        Self(out)
    }

    #[must_use]
    pub fn transpose(&self) -> Self {
        let mut out = [0.0; 16];
        for row in 0..4 {
            for column in 0..4 {
                out[row * 4 + column] = self.0[column * 4 + row];
            }
        }
        Self(out)
    }

    #[must_use]
    pub fn determinant(&self) -> f64 {
        let mut a = [
            [self.0[0], self.0[1], self.0[2], self.0[3]],
            [self.0[4], self.0[5], self.0[6], self.0[7]],
            [self.0[8], self.0[9], self.0[10], self.0[11]],
            [self.0[12], self.0[13], self.0[14], self.0[15]],
        ];
        let mut det = 1.0;
        for column in 0..4 {
            let mut pivot = column;
            for (row, values) in a.iter().enumerate().skip(column + 1) {
                if values[column].abs() > a[pivot][column].abs() {
                    pivot = row;
                }
            }
            if a[pivot][column].abs() < EPSILON {
                return 0.0;
            }
            if pivot != column {
                a.swap(pivot, column);
                det = -det;
            }
            det *= a[column][column];
            let divisor = a[column][column];
            let pivot_row = a[column];
            for row in a.iter_mut().skip(column + 1) {
                let factor = row[column] / divisor;
                for (value, pivot_value) in row.iter_mut().zip(pivot_row.iter()).skip(column) {
                    *value -= factor * pivot_value;
                }
            }
        }
        det
    }

    /// The inverse, or `None` when the matrix is singular.
    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        let mut a = [
            [self.0[0], self.0[1], self.0[2], self.0[3]],
            [self.0[4], self.0[5], self.0[6], self.0[7]],
            [self.0[8], self.0[9], self.0[10], self.0[11]],
            [self.0[12], self.0[13], self.0[14], self.0[15]],
        ];
        let mut inv = [[0.0f64; 4]; 4];
        for (row, values) in inv.iter_mut().enumerate() {
            values[row] = 1.0;
        }
        for column in 0..4 {
            let mut pivot = column;
            for (row, values) in a.iter().enumerate().skip(column + 1) {
                if values[column].abs() > a[pivot][column].abs() {
                    pivot = row;
                }
            }
            if a[pivot][column].abs() < EPSILON {
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
                for (value, pivot_value) in a[row].iter_mut().zip(pivot_row.iter()) {
                    *value -= factor * pivot_value;
                }
                for (value, pivot_value) in inv[row].iter_mut().zip(identity_row.iter()) {
                    *value -= factor * pivot_value;
                }
            }
        }
        let mut out = [0.0; 16];
        for (row, values) in inv.iter().enumerate() {
            for (column, value) in values.iter().enumerate() {
                out[row * 4 + column] = *value;
            }
        }
        Some(Self(out))
    }

    /// A point: the translation applies.
    #[must_use]
    pub fn transform_point(&self, v: Vec3) -> Vec3 {
        let w = self.0[12] * v.x + self.0[13] * v.y + self.0[14] * v.z + self.0[15];
        let x = self.0[0] * v.x + self.0[1] * v.y + self.0[2] * v.z + self.0[3];
        let y = self.0[4] * v.x + self.0[5] * v.y + self.0[6] * v.z + self.0[7];
        let z = self.0[8] * v.x + self.0[9] * v.y + self.0[10] * v.z + self.0[11];
        if w.abs() > EPSILON && (w - 1.0).abs() > EPSILON {
            Vec3::new(x / w, y / w, z / w)
        } else {
            Vec3::new(x, y, z)
        }
    }

    /// A direction: the translation is ignored, only the linear part applies.
    #[must_use]
    pub fn transform_direction(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.0[0] * v.x + self.0[1] * v.y + self.0[2] * v.z,
            self.0[4] * v.x + self.0[5] * v.y + self.0[6] * v.z,
            self.0[8] * v.x + self.0[9] * v.y + self.0[10] * v.z,
        )
    }

    /// A direction under the name the wishlist also uses.
    #[must_use]
    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        self.transform_direction(v)
    }

    #[must_use]
    pub fn approx_equal(&self, other: &Self, epsilon: f64) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .all(|(a, b)| (a - b).abs() <= epsilon)
    }
}

impl Mul for Matrix4 {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self {
        self.multiply(&rhs)
    }
}

impl Mul<Vec4> for Matrix4 {
    type Output = Vec4;

    fn mul(self, rhs: Vec4) -> Vec4 {
        Vec4::new(
            self.0[0] * rhs.x + self.0[1] * rhs.y + self.0[2] * rhs.z + self.0[3] * rhs.w,
            self.0[4] * rhs.x + self.0[5] * rhs.y + self.0[6] * rhs.z + self.0[7] * rhs.w,
            self.0[8] * rhs.x + self.0[9] * rhs.y + self.0[10] * rhs.z + self.0[11] * rhs.w,
            self.0[12] * rhs.x + self.0[13] * rhs.y + self.0[14] * rhs.z + self.0[15] * rhs.w,
        )
    }
}

/// An axis-aligned box between two corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    #[must_use]
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self {
            min: min.min(max),
            max: min.max(max),
        }
    }

    #[must_use]
    pub fn from_center_size(center: Vec3, size: Vec3) -> Self {
        Self::new(center - size * 0.5, center + size * 0.5)
    }

    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        Self::new(self.min.min(other.min), self.max.max(other.max))
    }

    /// The overlap, or `None` when the boxes only touch or miss.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let min = self.min.max(other.min);
        let max = self.max.min(other.max);
        if min.x > max.x || min.y > max.y || min.z > max.z {
            None
        } else {
            Some(Self { min, max })
        }
    }

    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }

    /// Grown outward by `amount` on every side.
    #[must_use]
    pub fn expand(&self, amount: f64) -> Self {
        let grow = Vec3::splat(amount);
        Self {
            min: self.min - grow,
            max: self.max + grow,
        }
    }

    #[must_use]
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    #[must_use]
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    #[must_use]
    pub fn volume(&self) -> f64 {
        let size = self.size();
        size.x * size.y * size.z
    }

    #[must_use]
    pub fn surface_area(&self) -> f64 {
        let size = self.size();
        2.0 * (size.x * size.y + size.y * size.z + size.z * size.x)
    }
}

/// A sphere.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f64,
}

impl Sphere {
    #[must_use]
    pub fn new(center: Vec3, radius: f64) -> Self {
        Self {
            center,
            radius: radius.abs(),
        }
    }

    #[must_use]
    pub fn contains(&self, point: Vec3) -> bool {
        self.center.distance_squared(point) <= self.radius * self.radius
    }

    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        let reach = self.radius + other.radius;
        self.center.distance_squared(other.center) <= reach * reach
    }

    /// Distance from the surface, negative when the point is inside.
    #[must_use]
    pub fn distance_to(&self, point: Vec3) -> f64 {
        self.center.distance(point) - self.radius
    }
}

/// A plane, `normal·point + distance = 0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: Vec3,
    pub distance: f64,
}

impl Plane {
    #[must_use]
    pub fn from_point_normal(point: Vec3, normal: Vec3) -> Self {
        let n = normal.normalize();
        Self {
            normal: n,
            distance: -n.dot(point),
        }
    }

    /// Positive on the normal's side.
    #[must_use]
    pub fn distance_to(&self, point: Vec3) -> f64 {
        self.normal.dot(point) + self.distance
    }

    #[must_use]
    pub fn project(&self, point: Vec3) -> Vec3 {
        point - self.normal * self.distance_to(point)
    }
}

/// A ray with an origin and a normalised direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

impl Ray {
    #[must_use]
    pub fn new(origin: Vec3, direction: Vec3) -> Self {
        Self {
            origin,
            direction: direction.normalize(),
        }
    }

    #[must_use]
    pub fn at(&self, distance: f64) -> Vec3 {
        self.origin + self.direction * distance
    }
}

/// A triangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triangle {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
}

impl Triangle {
    #[must_use]
    pub fn new(a: Vec3, b: Vec3, c: Vec3) -> Self {
        Self { a, b, c }
    }

    #[must_use]
    pub fn normal(&self) -> Vec3 {
        (self.b - self.a).cross(self.c - self.a).normalize()
    }

    #[must_use]
    pub fn area(&self) -> f64 {
        (self.b - self.a).cross(self.c - self.a).length() * 0.5
    }
}

/// An oriented box: a centre, half-extents along its own axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Obb {
    pub center: Vec3,
    pub half: Vec3,
    pub axes: [Vec3; 3],
}

impl Obb {
    #[must_use]
    pub fn new(center: Vec3, half: Vec3, axes: [Vec3; 3]) -> Self {
        Self { center, half, axes }
    }

    /// The axis-aligned box that contains it.
    #[must_use]
    pub fn to_aabb(&self) -> Aabb {
        let extent = self.axes[0].abs() * self.half.x
            + self.axes[1].abs() * self.half.y
            + self.axes[2].abs() * self.half.z;
        Aabb::new(self.center - extent, self.center + extent)
    }
}

/// A capsule: a segment and a radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    pub a: Vec3,
    pub b: Vec3,
    pub radius: f64,
}

impl Capsule {
    #[must_use]
    pub fn new(a: Vec3, b: Vec3, radius: f64) -> Self {
        Self {
            a,
            b,
            radius: radius.abs(),
        }
    }

    #[must_use]
    pub fn length(&self) -> f64 {
        self.a.distance(self.b)
    }
}

/// A rectangle by its lower corner and size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    #[must_use]
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    #[must_use]
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }

    #[must_use]
    pub fn center(&self) -> Vec2 {
        Vec2::new(self.x + self.width * 0.5, self.y + self.height * 0.5)
    }
}

/// A width and a height.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    #[must_use]
    pub fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub fn area(&self) -> f64 {
        self.width * self.height
    }

    #[must_use]
    pub fn aspect(&self) -> f64 {
        if self.height == 0.0 {
            0.0
        } else {
            self.width / self.height
        }
    }
}

/// Six planes, tested from the inside.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    #[must_use]
    pub fn contains_point(&self, point: Vec3) -> bool {
        self.planes.iter().all(|plane| plane.distance_to(point) >= -1e-3)
    }
}

/// A rigid transform: scale, then rotation, then translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quaternion,
    pub scale: Vec3,
}

impl Transform {
    #[must_use]
    pub fn identity() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quaternion::identity(),
            scale: Vec3::ONE,
        }
    }

    #[must_use]
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        let scaled = Vec3::new(
            point.x * self.scale.x,
            point.y * self.scale.y,
            point.z * self.scale.z,
        );
        self.rotation.rotate(scaled) + self.position
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::identity()
    }
}

/// A position and a rotation, with no scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub position: Vec3,
    pub rotation: Quaternion,
}

impl Pose {
    #[must_use]
    pub fn new(position: Vec3, rotation: Quaternion) -> Self {
        Self { position, rotation }
    }

    #[must_use]
    pub fn transform_point(&self, point: Vec3) -> Vec3 {
        self.rotation.rotate(point) + self.position
    }
}

/// An `f32` colour with an alpha.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    #[must_use]
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    #[must_use]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    #[must_use]
    pub fn to_rgba8(self) -> Rgba8 {
        Rgba8::new(
            (self.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.b.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.a.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    }
}

/// An `f64` colour with an alpha.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rgba {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

impl Rgba {
    #[must_use]
    pub fn new(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub fn to_rgba8(self) -> Rgba8 {
        Rgba8::new(
            (self.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.b.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.a.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    }
}

/// A colour in eight-bit channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    #[must_use]
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub fn to_rgba(self) -> Rgba {
        Rgba::new(
            f64::from(self.r) / 255.0,
            f64::from(self.g) / 255.0,
            f64::from(self.b) / 255.0,
            f64::from(self.a) / 255.0,
        )
    }

    #[must_use]
    pub fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// What an intersection answered: a point, a normal, how far along and whether the ray
/// started inside.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f64,
    pub inside: bool,
}

/// The value limited to `[min, max]`.
#[must_use]
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    value.clamp(min, max)
}

/// The point `t` of the way from `a` to `b`.
#[must_use]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Where `value` sits between `a` and `b`, as a fraction; zero when the range is empty.
#[must_use]
pub fn inverse_lerp(a: f64, b: f64, value: f64) -> f64 {
    if (b - a).abs() < EPSILON {
        0.0
    } else {
        (value - a) / (b - a)
    }
}

/// `value` re-mapped from one range to another.
#[must_use]
pub fn remap(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    lerp(out_min, out_max, inverse_lerp(in_min, in_max, value))
}

/// The same as [`remap`], under the name the wishlist also uses.
#[must_use]
pub fn map_range(value: f64, in_min: f64, in_max: f64, out_min: f64, out_max: f64) -> f64 {
    remap(value, in_min, in_max, out_min, out_max)
}

/// A smooth `0..1` ramp between the two edges.
#[must_use]
pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `0` below the edge and `1` at or above it.
#[must_use]
pub fn step(edge: f64, x: f64) -> f64 {
    if x < edge {
        0.0
    } else {
        1.0
    }
}

/// An angle in degrees wrapped into `(-180, 180]`.
#[must_use]
pub fn wrap_angle(degrees: f64) -> f64 {
    let mut wrapped = degrees % 360.0;
    if wrapped > 180.0 {
        wrapped -= 360.0;
    }
    if wrapped <= -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// An angle in degrees wrapped into `[0, 360)`.
#[must_use]
pub fn normalize_angle(degrees: f64) -> f64 {
    let wrapped = degrees % 360.0;
    if wrapped < 0.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// The shortest-arc interpolation between two angles in degrees.
#[must_use]
pub fn lerp_angle(from: f64, to: f64, t: f64) -> f64 {
    wrap_angle(from + wrap_angle(to - from) * t)
}

#[must_use]
pub fn degrees_to_radians(degrees: f64) -> f64 {
    degrees.to_radians()
}

#[must_use]
pub fn radians_to_degrees(radians: f64) -> f64 {
    radians.to_degrees()
}

#[must_use]
pub fn distance(a: Vec3, b: Vec3) -> f64 {
    a.distance(b)
}

#[must_use]
pub fn distance_squared(a: Vec3, b: Vec3) -> f64 {
    a.distance_squared(b)
}

/// The angle between two vectors in radians, `0` when either is zero-length.
#[must_use]
pub fn angle_between(a: Vec3, b: Vec3) -> f64 {
    a.angle_between(b)
}

/// A unit vector from one point towards another; zero when the points coincide.
#[must_use]
pub fn direction_to(from: Vec3, to: Vec3) -> Vec3 {
    (to - from).normalize()
}

/// The mirror of an incident vector about a unit normal.
#[must_use]
pub fn reflect(incident: Vec3, normal: Vec3) -> Vec3 {
    incident - normal * (2.0 * incident.dot(normal))
}

/// The refracted direction, or `None` under total internal reflection.
#[must_use]
pub fn refract(incident: Vec3, normal: Vec3, eta: f64) -> Option<Vec3> {
    let i = incident.normalize();
    let n = normal.normalize();
    let cos_i = -i.dot(n);
    let k = 1.0 - eta * eta * (1.0 - cos_i * cos_i);
    if k < 0.0 {
        None
    } else {
        Some(i * eta + n * (eta * cos_i - k.sqrt()))
    }
}

#[must_use]
pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a.dot(b)
}

#[must_use]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    a.cross(b)
}

/// A unit vector, or the zero vector when the input has no length.
#[must_use]
pub fn normalize(v: Vec3) -> Vec3 {
    v.normalize()
}

/// `v` projected onto `onto`; zero when `onto` is zero-length.
#[must_use]
pub fn project(v: Vec3, onto: Vec3) -> Vec3 {
    let denom = onto.length_squared();
    if denom < EPSILON {
        Vec3::ZERO
    } else {
        onto * (v.dot(onto) / denom)
    }
}

/// The point on the segment `a..b` closest to `point`.
#[must_use]
pub fn closest_point(point: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let span = b - a;
    let denom = span.length_squared();
    if denom < EPSILON {
        return a;
    }
    let t = ((point - a).dot(span) / denom).clamp(0.0, 1.0);
    a + span * t
}

/// The first point where a ray meets a sphere, or `None`; a tangent ray answers the touch.
#[must_use]
pub fn intersect_ray(ray: &Ray, sphere: &Sphere) -> Option<RayHit> {
    let oc = ray.origin - sphere.center;
    let b = 2.0 * oc.dot(ray.direction);
    let c = oc.dot(oc) - sphere.radius * sphere.radius;
    let disc = b * b - 4.0 * c;
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    let mut t = (-b - root) / 2.0;
    if t < 0.0 {
        t = (-b + root) / 2.0;
    }
    if t < 0.0 {
        return None;
    }
    let point = ray.at(t);
    let outward = (point - sphere.center).normalize();
    let inside = c < 0.0;
    Some(RayHit {
        point,
        normal: if inside { -outward } else { outward },
        distance: t,
        inside,
    })
}

/// The first point where a ray meets an axis-aligned box, or `None`.
#[must_use]
pub fn intersect_aabb(ray: &Ray, aabb: &Aabb) -> Option<RayHit> {
    let origin = ray.origin.to_array();
    let direction = ray.direction.to_array();
    let low = aabb.min.to_array();
    let high = aabb.max.to_array();
    let mut near = f64::NEG_INFINITY;
    let mut far = f64::INFINITY;
    let mut normal = Vec3::ZERO;
    for axis in 0..3 {
        if direction[axis].abs() < EPSILON {
            if origin[axis] < low[axis] || origin[axis] > high[axis] {
                return None;
            }
            continue;
        }
        let mut t1 = (low[axis] - origin[axis]) / direction[axis];
        let mut t2 = (high[axis] - origin[axis]) / direction[axis];
        let mut sign = -1.0;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
            sign = 1.0;
        }
        if t1 > near {
            near = t1;
            normal = axis_unit(axis, sign);
        }
        if t2 < far {
            far = t2;
        }
        if near > far {
            return None;
        }
    }
    if far < 0.0 {
        return None;
    }
    let inside = near < 0.0;
    let t = if inside { far } else { near };
    Some(RayHit {
        point: ray.at(t),
        normal: if inside { -normal } else { normal },
        distance: t,
        inside,
    })
}

/// Where a ray meets a sphere and a second sphere, or `None`.
#[must_use]
pub fn intersect_sphere(a: &Sphere, b: &Sphere) -> Option<RayHit> {
    let delta = b.center - a.center;
    let span = delta.length();
    if span > a.radius + b.radius || span < EPSILON {
        return None;
    }
    let normal = delta * (1.0 / span);
    Some(RayHit {
        point: a.center + normal * a.radius,
        normal,
        distance: a.radius,
        inside: span + a.radius <= b.radius,
    })
}

/// Where a ray meets a plane, or `None` when it is parallel or behind.
#[must_use]
pub fn intersect_plane(ray: &Ray, plane: &Plane) -> Option<RayHit> {
    let denom = plane.normal.dot(ray.direction);
    if denom.abs() < EPSILON {
        return None;
    }
    let t = -(plane.normal.dot(ray.origin) + plane.distance) / denom;
    if t < 0.0 {
        return None;
    }
    Some(RayHit {
        point: ray.at(t),
        normal: plane.normal,
        distance: t,
        inside: false,
    })
}

/// Where a ray meets a triangle by the Möller–Trumbore test, or `None`.
#[must_use]
pub fn intersect_triangle(ray: &Ray, triangle: &Triangle) -> Option<RayHit> {
    let edge1 = triangle.b - triangle.a;
    let edge2 = triangle.c - triangle.a;
    let pvec = ray.direction.cross(edge2);
    let det = edge1.dot(pvec);
    if det.abs() < EPSILON {
        return None;
    }
    let inv = 1.0 / det;
    let svec = ray.origin - triangle.a;
    let u = svec.dot(pvec) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qvec = svec.cross(edge1);
    let v = ray.direction.dot(qvec) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = edge2.dot(qvec) * inv;
    if t < 0.0 {
        return None;
    }
    Some(RayHit {
        point: ray.at(t),
        normal: edge1.cross(edge2).normalize(),
        distance: t,
        inside: false,
    })
}

/// Where two 2D segments cross, or `None` when they are parallel or do not overlap.
#[must_use]
pub fn intersect_segments_2d(a1: Vec2, a2: Vec2, b1: Vec2, b2: Vec2) -> Option<Vec2> {
    let r = a2 - a1;
    let s = b2 - b1;
    let denom = r.cross(s);
    if denom.abs() < EPSILON {
        return None;
    }
    let diff = b1 - a1;
    let t = diff.cross(s) / denom;
    let u = diff.cross(r) / denom;
    if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
        Some(a1 + r * t)
    } else {
        None
    }
}

fn axis_unit(axis: usize, sign: f64) -> Vec3 {
    match axis {
        0 => Vec3::new(sign, 0.0, 0.0),
        1 => Vec3::new(0.0, sign, 0.0),
        _ => Vec3::new(0.0, 0.0, sign),
    }
}

/// A named easing curve for [`ease`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Easing {
    Linear,
    QuadIn,
    QuadOut,
    QuadInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    SineIn,
    SineOut,
    SineInOut,
    BackIn,
    BackOut,
    BounceOut,
    ElasticOut,
}

impl Easing {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::QuadIn => "quad_in",
            Self::QuadOut => "quad_out",
            Self::QuadInOut => "quad_in_out",
            Self::CubicIn => "cubic_in",
            Self::CubicOut => "cubic_out",
            Self::CubicInOut => "cubic_in_out",
            Self::SineIn => "sine_in",
            Self::SineOut => "sine_out",
            Self::SineInOut => "sine_in_out",
            Self::BackIn => "back_in",
            Self::BackOut => "back_out",
            Self::BounceOut => "bounce_out",
            Self::ElasticOut => "elastic_out",
        }
    }
}

/// Every easing curve, in the order the wishlist's list reads.
pub const EASINGS: [Easing; 14] = [
    Easing::Linear,
    Easing::QuadIn,
    Easing::QuadOut,
    Easing::QuadInOut,
    Easing::CubicIn,
    Easing::CubicOut,
    Easing::CubicInOut,
    Easing::SineIn,
    Easing::SineOut,
    Easing::SineInOut,
    Easing::BackIn,
    Easing::BackOut,
    Easing::BounceOut,
    Easing::ElasticOut,
];

/// The names of the easing curves, for a caller that wants to list them.
#[must_use]
pub fn easing_list() -> Vec<&'static str> {
    EASINGS.iter().map(|kind| kind.as_str()).collect()
}

/// The curve applied to a normalised `t`; every one answers `0` at `0` and `1` at `1`.
#[must_use]
pub fn ease(kind: Easing, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match kind {
        Easing::Linear => t,
        Easing::QuadIn => t * t,
        Easing::QuadOut => 1.0 - (1.0 - t) * (1.0 - t),
        Easing::QuadInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - 2.0 * (1.0 - t) * (1.0 - t)
            }
        }
        Easing::CubicIn => t * t * t,
        Easing::CubicOut => 1.0 - (1.0 - t).powi(3),
        Easing::CubicInOut => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
            }
        }
        Easing::SineIn => 1.0 - (t * std::f64::consts::FRAC_PI_2).cos(),
        Easing::SineOut => (t * std::f64::consts::FRAC_PI_2).sin(),
        Easing::SineInOut => -(std::f64::consts::PI * t).cos().sub(1.0) / 2.0,
        Easing::BackIn => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            c3 * t * t * t - c1 * t * t
        }
        Easing::BackOut => {
            let c1 = 1.70158;
            let c3 = c1 + 1.0;
            1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0) * (t - 1.0)
        }
        Easing::BounceOut => bounce_out(t),
        Easing::ElasticOut => {
            if t <= 0.0 {
                0.0
            } else if t >= 1.0 {
                1.0
            } else {
                let c4 = std::f64::consts::TAU / 3.0;
                2.0f64.powf(-10.0 * t) * ((t * 10.0 - 0.75) * c4).sin() + 1.0
            }
        }
    }
}

fn bounce_out(t: f64) -> f64 {
    let n1 = 7.5625;
    let d1 = 2.75;
    if t < 1.0 / d1 {
        n1 * t * t
    } else if t < 2.0 / d1 {
        let t = t - 1.5 / d1;
        n1 * t * t + 0.75
    } else if t < 2.5 / d1 {
        let t = t - 2.25 / d1;
        n1 * t * t + 0.9375
    } else {
        let t = t - 2.625 / d1;
        n1 * t * t + 0.984375
    }
}

fn hash2(x: i32, y: i32, seed: u64) -> u64 {
    let h = (x as i64 as u64)
        .wrapping_mul(0x9E3779B97F4A7C15)
        ^ (y as i64 as u64).wrapping_mul(0xC2B2AE3D27D4EB4F)
        ^ seed;
    let mut z = h;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

fn unit(hash: u64) -> f64 {
    (hash >> 11) as f64 / (1u64 << 53) as f64
}

/// Value noise on a unit integer lattice: each lattice point hashes to `[-1, 1]` and the four
/// around a sample are blended by smoothstep. Deterministic for a seed, in `[-1, 1]`,
/// continuous, and it answers exactly the lattice value at a lattice point.
#[must_use]
pub fn value_noise_2d(x: f64, y: f64, seed: u64) -> f64 {
    let x0 = x.floor();
    let y0 = y.floor();
    let (ix, iy) = (x0 as i32, y0 as i32);
    let fx = x - x0;
    let fy = y - y0;
    let u = fx * fx * (3.0 - 2.0 * fx);
    let v = fy * fy * (3.0 - 2.0 * fy);
    let corner = |dx, dy| unit(hash2(ix + dx, iy + dy, seed)) * 2.0 - 1.0;
    let top = lerp(corner(0, 0), corner(1, 0), u);
    let bottom = lerp(corner(0, 1), corner(1, 1), u);
    lerp(top, bottom, v)
}

fn gradient(hash: u64, x: f64, y: f64) -> f64 {
    match hash & 7 {
        0 => x + y,
        1 => x - y,
        2 => -x + y,
        3 => -x - y,
        4 => x,
        5 => -x,
        6 => y,
        _ => -y,
    }
}

/// Gradient noise on the same unit lattice, in about `[-1, 1]`; deterministic and continuous.
#[must_use]
pub fn gradient_noise_2d(x: f64, y: f64, seed: u64) -> f64 {
    let x0 = x.floor();
    let y0 = y.floor();
    let (ix, iy) = (x0 as i32, y0 as i32);
    let fx = x - x0;
    let fy = y - y0;
    let u = fx * fx * (3.0 - 2.0 * fx);
    let v = fy * fy * (3.0 - 2.0 * fy);
    let dot00 = gradient(hash2(ix, iy, seed), fx, fy);
    let dot10 = gradient(hash2(ix + 1, iy, seed), fx - 1.0, fy);
    let dot01 = gradient(hash2(ix, iy + 1, seed), fx, fy - 1.0);
    let dot11 = gradient(hash2(ix + 1, iy + 1, seed), fx - 1.0, fy - 1.0);
    lerp(lerp(dot00, dot10, u), lerp(dot01, dot11, u), v)
}

/// Fractal noise: `octaves` layers of [`value_noise_2d`] at doubling frequency and halving
/// amplitude, in `[-1, 1]`.
#[must_use]
pub fn fbm_2d(x: f64, y: f64, octaves: u32, seed: u64) -> f64 {
    let mut sum = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut total = 0.0;
    for octave in 0..octaves.max(1) {
        sum += value_noise_2d(x * frequency, y * frequency, seed.wrapping_add(u64::from(octave))) * amplitude;
        total += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }
    if total == 0.0 {
        0.0
    } else {
        sum / total
    }
}

/// A splitmix64 generator: fast, deterministic and reproducible from a seed.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[must_use]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    #[must_use]
    pub fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }

    /// A value in `[0, 1)`.
    #[must_use]
    pub fn next_f64(&mut self) -> f64 {
        unit(self.next_u64())
    }

    /// A value in `[min, max)`, or `min` when the range is empty.
    #[must_use]
    pub fn range(&mut self, min: f64, max: f64) -> f64 {
        if max <= min {
            return min;
        }
        min + (max - min) * self.next_f64()
    }

    /// An integer in `[min, max]` inclusive, swapping the bounds if they are reversed.
    #[must_use]
    pub fn integer(&mut self, min: i64, max: i64) -> i64 {
        let (low, high) = if min <= max { (min, max) } else { (max, min) };
        let span = (high - low) as u64 + 1;
        low + (self.next_u64() % span) as i64
    }

    /// True with the given probability, `p` clamped to `[0, 1]`.
    #[must_use]
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p.clamp(0.0, 1.0)
    }
}

fn global_rng() -> &'static Mutex<Rng> {
    static GLOBAL: OnceLock<Mutex<Rng>> = OnceLock::new();
    GLOBAL.get_or_init(|| Mutex::new(Rng::new(seed_from_time())))
}

fn seed_from_time() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64);
    nanos ^ (u64::from(std::process::id())).wrapping_mul(0x9E3779B97F4A7C15)
}

fn with_rng<T>(body: impl FnOnce(&mut Rng) -> T) -> T {
    let mut guard = global_rng().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// A value in `[0, 1)`.
#[must_use]
pub fn random() -> f64 {
    with_rng(Rng::next_f64)
}

/// A value in `[min, max)`.
#[must_use]
pub fn random_range(min: f64, max: f64) -> f64 {
    with_rng(|rng| rng.range(min, max))
}

/// The same as [`random_range`], for a caller that thinks in floats.
#[must_use]
pub fn random_float(min: f64, max: f64) -> f64 {
    random_range(min, max)
}

/// An integer in `[min, max]` inclusive.
#[must_use]
pub fn random_integer(min: i64, max: i64) -> i64 {
    with_rng(|rng| rng.integer(min, max))
}

/// True with probability `chance`.
#[must_use]
pub fn random_bool(chance: f64) -> bool {
    with_rng(|rng| rng.chance(chance))
}

/// A generator seeded a caller's way, reproducible exactly.
#[must_use]
pub fn seeded_random(seed: u64) -> Rng {
    Rng::new(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_are_exact_and_the_easing_list_is_complete() {
        assert_eq!(clamp(7.0, 0.0, 3.0), 3.0);
        assert_eq!(clamp(-1.0, 0.0, 3.0), 0.0);
        assert_eq!(clamp(1.5, 0.0, 3.0), 1.5);
        assert_eq!(lerp(0.0, 10.0, 0.25), 2.5);
        assert_eq!(inverse_lerp(0.0, 10.0, 2.5), 0.25);
        assert_eq!(remap(5.0, 0.0, 10.0, 100.0, 200.0), 150.0);
        assert_eq!(map_range(5.0, 0.0, 10.0, 100.0, 200.0), 150.0);
        assert_eq!(smoothstep(0.0, 10.0, 5.0), 0.5);
        assert_eq!(smoothstep(0.0, 10.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 10.0, 11.0), 1.0);
        assert_eq!(step(5.0, 4.0), 0.0);
        assert_eq!(step(5.0, 5.0), 1.0);
        assert_eq!(ease(Easing::Linear, 0.3), 0.3);
        assert_eq!(ease(Easing::QuadIn, 0.5), 0.25);
        assert_eq!(ease(Easing::QuadOut, 0.5), 0.75);
        assert!((ease(Easing::SineIn, 1.0) - 1.0).abs() < 1e-12);
        for kind in EASINGS {
            assert!(ease(kind, 0.0).abs() < 1e-12, "{} starts at zero", kind.as_str());
            assert!((ease(kind, 1.0) - 1.0).abs() < 1e-12, "{} ends at one", kind.as_str());
        }
        assert_eq!(easing_list().len(), 14);
    }

    #[test]
    fn angles_wrap_and_take_the_short_way_round() {
        assert_eq!(wrap_angle(190.0), -170.0);
        assert_eq!(wrap_angle(-190.0), 170.0);
        assert_eq!(wrap_angle(180.0), 180.0);
        assert_eq!(normalize_angle(-90.0), 270.0);
        assert_eq!(normalize_angle(450.0), 90.0);
        assert!((degrees_to_radians(180.0) - std::f64::consts::PI).abs() < 1e-12);
        assert!((radians_to_degrees(std::f64::consts::PI) - 180.0).abs() < 1e-12);
        assert_eq!(lerp_angle(350.0, 10.0, 0.5), 0.0);
        assert_eq!(lerp_angle(10.0, 350.0, 0.5), 0.0);
        assert_eq!(lerp_angle(0.0, 90.0, 0.5), 45.0);
    }

    #[test]
    fn vectors_add_scale_measure_and_normalise_without_dividing_by_zero() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, 5.0, 6.0);
        assert_eq!(a + b, Vec3::new(5.0, 7.0, 9.0));
        assert_eq!(b - a, Vec3::new(3.0, 3.0, 3.0));
        assert_eq!(a * 2.0, Vec3::new(2.0, 4.0, 6.0));
        assert_eq!(dot(a, b), 32.0);
        assert_eq!(cross(Vec3::X, Vec3::Y), Vec3::Z);
        assert_eq!(Vec3::new(3.0, 4.0, 0.0).length(), 5.0);
        assert_eq!(Vec3::new(3.0, 4.0, 0.0).length_squared(), 25.0);
        assert_eq!(normalize(Vec3::ZERO), Vec3::ZERO);
        assert!(normalize(Vec3::new(3.0, 4.0, 0.0)).approx_eq(Vec3::new(0.6, 0.8, 0.0), 1e-12));
        assert_eq!(distance(Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0)), 5.0);
        assert_eq!(distance_squared(Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0)), 25.0);
        assert_eq!(
            Vec3::ZERO.lerp(Vec3::new(10.0, 20.0, 30.0), 0.5),
            Vec3::new(5.0, 10.0, 15.0)
        );
        assert!((angle_between(Vec3::X, Vec3::Y) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(direction_to(Vec3::ZERO, Vec3::new(0.0, 0.0, 5.0)), Vec3::Z);
        assert_eq!(direction_to(Vec3::ZERO, Vec3::ZERO), Vec3::ZERO);
        assert_eq!(Vec2::new(1.0, 2.0).dot(Vec2::new(3.0, 4.0)), 11.0);
        assert_eq!(Vec2::new(1.0, 0.0).perp(), Vec2::new(0.0, 1.0));
        assert_eq!(Vec4::new(1.0, 2.0, 2.0, 4.0).length(), 5.0);
        assert_eq!(IVec3::new(1, 2, 3) + IVec3::new(1, 1, 1), IVec3::new(2, 3, 4));
        assert_eq!(IVec2::new(5, 2) - IVec2::new(1, 2), IVec2::new(4, 0));
    }

    #[test]
    fn a_matrix_inverts_to_the_identity_and_refuses_a_singular_one() {
        let m = Matrix4::translation(1.0, 2.0, 3.0)
            * Matrix4::rotation_z(std::f64::consts::FRAC_PI_2)
            * Matrix4::scale(2.0, 2.0, 2.0);
        let inverse = m.inverse().expect("non-singular");
        assert!(m.multiply(&inverse).approx_equal(&Matrix4::identity(), 1e-9));
        assert!((m.determinant() - 8.0).abs() < 1e-9, "{}", m.determinant());
        assert!(Matrix4::scale(0.0, 1.0, 1.0).inverse().is_none(), "singular");
        assert_eq!(m.transpose().transpose(), m);

        let move_by = Matrix4::translation(10.0, 0.0, 0.0);
        assert!(move_by
            .transform_point(Vec3::X)
            .approx_eq(Vec3::new(11.0, 0.0, 0.0), 1e-12));
        assert_eq!(move_by.transform_direction(Vec3::X), Vec3::X, "translation ignored");
        assert_eq!(move_by.transform_vector(Vec3::X), Vec3::X);
        assert!(Matrix4::identity().transform_point(Vec3::new(1.0, 2.0, 3.0)).approx_eq(Vec3::new(1.0, 2.0, 3.0), 1e-12));

        let three = Matrix3::from_rows([2.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 8.0]);
        assert_eq!(three.determinant(), 64.0);
        assert!(three
            .multiply(&three.inverse().expect("non-singular"))
            .transform(Vec3::ONE)
            .approx_eq(Vec3::ONE, 1e-12));
        assert_eq!(three.transform(Vec3::ONE), Vec3::new(2.0, 4.0, 8.0));
    }

    #[test]
    fn euler_and_quaternion_rotations_land_where_the_arithmetic_says() {
        assert!(EulerAngles::new(90.0, 0.0, 0.0)
            .to_direction()
            .approx_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-12));
        assert!(EulerAngles::from_direction(Vec3::new(-1.0, 0.0, 0.0))
            .to_direction()
            .approx_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-9));
        let euler = EulerAngles::new(35.0, -20.0, 0.0);
        let q = Quaternion::from_euler(euler);
        assert!(q.rotate(Vec3::Z).approx_eq(euler.to_direction(), 1e-9));

        assert!(Quaternion::identity().rotate(Vec3::new(1.0, 2.0, 3.0)).approx_eq(Vec3::new(1.0, 2.0, 3.0), 1e-12));
        assert!(q.multiply(q.conjugate()).approx_eq(Quaternion::identity(), 1e-9));
        let quarter = Quaternion::from_axis_angle(Vec3::Y, std::f64::consts::FRAC_PI_2);
        assert!(quarter.rotate(Vec3::Z).approx_eq(Vec3::X, 1e-9));
        let half = Quaternion::identity().slerp(quarter, 0.5);
        assert!(half
            .rotate(Vec3::Z)
            .approx_eq(Vec3::new(2.0f64.sqrt() / 2.0, 0.0, 2.0f64.sqrt() / 2.0), 1e-9));
        assert!((half.length() - 1.0).abs() < 1e-9);
        let round_trip = Quaternion::from_axis_angle(Vec3::Y, std::f64::consts::PI).to_euler();
        assert!(round_trip.yaw.abs() > 170.0, "{round_trip:?}");
    }

    #[test]
    fn reflection_refraction_projection_and_the_closest_point() {
        assert!(reflect(Vec3::new(1.0, -1.0, 0.0), Vec3::Y).approx_eq(Vec3::new(1.0, 1.0, 0.0), 1e-12));
        assert!(refract(Vec3::new(0.0, -1.0, 0.0), Vec3::Y, 0.5)
            .expect("it passes")
            .approx_eq(Vec3::new(0.0, -1.0, 0.0), 1e-12));
        assert!(refract(Vec3::new(1.0, -0.5, 0.0), Vec3::Y, 1.5).is_none(), "total internal reflection");
        assert!(project(Vec3::new(2.0, 3.0, 0.0), Vec3::X).approx_eq(Vec3::new(2.0, 0.0, 0.0), 1e-12));
        assert_eq!(project(Vec3::new(1.0, 1.0, 1.0), Vec3::ZERO), Vec3::ZERO);
        assert!(closest_point(Vec3::new(5.0, 5.0, 0.0), Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0))
            .approx_eq(Vec3::new(5.0, 0.0, 0.0), 1e-12));
        assert_eq!(
            closest_point(Vec3::new(-5.0, 1.0, 0.0), Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)),
            Vec3::ZERO
        );
        assert_eq!(closest_point(Vec3::new(5.0, 5.0, 0.0), Vec3::ONE, Vec3::ONE), Vec3::ONE);
    }

    #[test]
    fn a_ray_meets_its_shapes_or_misses_them() {
        let sphere = Sphere::new(Vec3::ZERO, 1.0);
        let hit = intersect_ray(&Ray::new(Vec3::new(0.0, 0.0, -5.0), Vec3::Z), &sphere).expect("a hit");
        assert!(hit.point.approx_eq(Vec3::new(0.0, 0.0, -1.0), 1e-9));
        assert!(hit.normal.approx_eq(Vec3::new(0.0, 0.0, -1.0), 1e-9));
        assert!((hit.distance - 4.0).abs() < 1e-9);
        assert!(intersect_ray(&Ray::new(Vec3::new(5.0, 0.0, -5.0), Vec3::Z), &sphere).is_none(), "a miss");
        let tangent = intersect_ray(&Ray::new(Vec3::new(1.0, 0.0, -5.0), Vec3::Z), &sphere).expect("a tangent");
        assert!(tangent.point.approx_eq(Vec3::new(1.0, 0.0, 0.0), 1e-9));
        assert!((tangent.distance - 5.0).abs() < 1e-9);

        let box3 = Aabb::new(Vec3::ZERO, Vec3::splat(2.0));
        let boxed = intersect_aabb(&Ray::new(Vec3::new(-1.0, 1.0, 1.0), Vec3::X), &box3).expect("a hit");
        assert!(boxed.point.approx_eq(Vec3::new(0.0, 1.0, 1.0), 1e-9));
        assert!(boxed.normal.approx_eq(Vec3::new(-1.0, 0.0, 0.0), 1e-9));
        assert!(intersect_aabb(&Ray::new(Vec3::new(-1.0, 5.0, 1.0), Vec3::X), &box3).is_none(), "a miss");

        let plane = Plane::from_point_normal(Vec3::ZERO, Vec3::Y);
        let flat = intersect_plane(&Ray::new(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y), &plane).expect("a hit");
        assert!((flat.distance - 5.0).abs() < 1e-9);
        assert!(intersect_plane(&Ray::new(Vec3::new(0.0, 5.0, 0.0), Vec3::X), &plane).is_none(), "parallel");

        let triangle = Triangle::new(Vec3::ZERO, Vec3::X, Vec3::Y);
        let tri = intersect_triangle(&Ray::new(Vec3::new(0.25, 0.25, -1.0), Vec3::Z), &triangle).expect("a hit");
        assert!(tri.point.approx_eq(Vec3::new(0.25, 0.25, 0.0), 1e-9));
        assert!(tri.normal.approx_eq(Vec3::Z, 1e-9));
        assert!(intersect_triangle(&Ray::new(Vec3::new(2.0, 2.0, -1.0), Vec3::Z), &triangle).is_none(), "a miss");

        let cross2 = intersect_segments_2d(Vec2::new(0.0, 0.0), Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0), Vec2::new(2.0, 0.0));
        assert!(cross2.expect("a crossing").approx_eq(Vec2::new(1.0, 1.0), 1e-12));
        assert!(intersect_segments_2d(Vec2::ZERO, Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0), Vec2::new(1.0, 2.0)).is_none(), "parallel");

        let a = Sphere::new(Vec3::ZERO, 1.0);
        let b = Sphere::new(Vec3::new(1.5, 0.0, 0.0), 1.0);
        assert!(intersect_sphere(&a, &b).expect("they touch").point.approx_eq(Vec3::X, 1e-12));
        let tangent2 = Sphere::new(Vec3::new(2.0, 0.0, 0.0), 1.0);
        assert!(intersect_sphere(&a, &tangent2).is_some(), "tangent spheres touch");
        let apart = Sphere::new(Vec3::new(5.0, 0.0, 0.0), 1.0);
        assert!(intersect_sphere(&a, &apart).is_none(), "a miss");
    }

    #[test]
    fn boxes_union_intersect_and_measure() {
        let a = Aabb::new(Vec3::ZERO, Vec3::splat(2.0));
        let b = Aabb::new(Vec3::ONE, Vec3::splat(3.0));
        assert_eq!(a.union(&b), Aabb::new(Vec3::ZERO, Vec3::splat(3.0)));
        assert_eq!(a.intersection(&b), Some(Aabb::new(Vec3::ONE, Vec3::splat(2.0))));
        assert!(a.contains(Vec3::ONE));
        assert!(a.contains(Vec3::new(0.0, 0.0, 2.0)));
        assert!(!a.contains(Vec3::new(0.0, 0.0, 2.5)));
        assert_eq!(a.expand(1.0), Aabb::new(Vec3::splat(-1.0), Vec3::splat(3.0)));
        assert_eq!(a.center(), Vec3::ONE);
        assert_eq!(a.size(), Vec3::splat(2.0));
        assert_eq!(a.volume(), 8.0);
        assert_eq!(a.surface_area(), 24.0);
        let far = Aabb::new(Vec3::splat(10.0), Vec3::splat(11.0));
        assert!(a.intersection(&far).is_none());
        assert_eq!(Aabb::from_center_size(Vec3::ONE, Vec3::splat(2.0)), a);
        assert!(Sphere::new(Vec3::ZERO, 1.0).contains(Vec3::new(0.5, 0.5, 0.5)));
        assert!(Sphere::new(Vec3::ZERO, 1.0).intersects(&Sphere::new(Vec3::X, 0.5)));
        assert!((Sphere::new(Vec3::ZERO, 1.0).distance_to(Vec3::new(3.0, 0.0, 0.0)) - 2.0).abs() < 1e-12);
        assert_eq!(Plane::from_point_normal(Vec3::new(0.0, 2.0, 0.0), Vec3::Y).project(Vec3::new(1.0, 5.0, 1.0)), Vec3::new(1.0, 2.0, 1.0));
    }

    #[test]
    fn the_noise_is_seeded_and_continuous() {
        let value = value_noise_2d(0.3, 0.7, 42);
        assert_eq!(value, value_noise_2d(0.3, 0.7, 42), "deterministic for a seed");
        assert!((-1.0..=1.0).contains(&value), "{value}");
        let near = value_noise_2d(0.3 + 1e-4, 0.7, 42);
        assert!((value - near).abs() < 1e-3, "continuous: {value} vs {near}");
        let hashed = value_noise_2d(2.0, 2.0, 42);
        assert_eq!(hashed, value_noise_2d(2.0, 2.0, 42));
        assert!((-1.0..=1.0).contains(&hashed));

        let gradient = gradient_noise_2d(0.3, 0.7, 1);
        assert_eq!(gradient, gradient_noise_2d(0.3, 0.7, 1));
        assert!((-1.0..=1.0).contains(&gradient), "{gradient}");
        assert!((gradient - gradient_noise_2d(0.3 + 1e-4, 0.7, 1)).abs() < 1e-3);

        let differs = (0..16).any(|i| {
            value_noise_2d(i as f64 * 0.37, 0.5, 7) != value_noise_2d(i as f64 * 0.37, 0.5, 8)
        });
        assert!(differs, "a different seed gives different noise");
        assert!((-1.0..=1.0).contains(&fbm_2d(0.3, 0.7, 4, 42)));
    }

    #[test]
    fn the_rng_reproduces_a_sequence_exactly() {
        let mut rng = seeded_random(42);
        assert_eq!(rng.next_u64(), 13679457532755275413);
        assert_eq!(rng.next_u64(), 2949826092126892291);
        assert_eq!(rng.next_u64(), 5139283748462763858);

        let mut again = seeded_random(42);
        assert_eq!(again.next_u32(), 803958421);
        assert_eq!(again.next_u64(), 2949826092126892291, "the u32 consumed one step");
        assert_eq!(Rng::new(7).next_f64(), Rng::new(7).next_f64());

        let mut fresh = seeded_random(42);
        assert!((fresh.next_f64() - 0.7415648787718233).abs() < 1e-15);

        let mut ranged = seeded_random(1);
        for _ in 0..100 {
            let value = ranged.range(10.0, 20.0);
            assert!((10.0..20.0).contains(&value), "{value}");
            let integer = ranged.integer(1, 6);
            assert!((1..=6).contains(&integer), "{integer}");
        }
        assert_eq!(seeded_random(1).range(5.0, 5.0), 5.0);
        assert!(seeded_random(1).chance(1.0));
        assert!(!seeded_random(1).chance(0.0));

        assert!((0.0..1.0).contains(&random()));
        assert_eq!(random_integer(1, 1), 1);
        assert!(random_bool(1.0));
        assert!(!random_bool(0.0));
        assert!((0.0..1.0).contains(&random_range(0.0, 1.0)));
        assert!((0.0..1.0).contains(&random_float(0.0, 1.0)));
    }

    #[test]
    fn the_remaining_types_carry_real_values() {
        assert_eq!(Size::new(16.0, 9.0).area(), 144.0);
        assert!((Size::new(16.0, 9.0).aspect() - 16.0 / 9.0).abs() < 1e-12);
        assert_eq!(Rect::new(0.0, 0.0, 10.0, 10.0).center(), Vec2::new(5.0, 5.0));
        assert!(Rect::new(0.0, 0.0, 10.0, 10.0).contains(5.0, 5.0));
        assert!(Quaternion::from_axis_angle(Vec3::Z, 0.0).approx_eq(Quaternion::identity(), 1e-12));
        assert!(Transform::identity().transform_point(Vec3::ONE).approx_eq(Vec3::ONE, 1e-12));
        assert_eq!(Pose::new(Vec3::X, Quaternion::identity()).transform_point(Vec3::ONE), Vec3::new(2.0, 1.0, 1.0));
        assert_eq!(Triangle::new(Vec3::ZERO, Vec3::X, Vec3::Y).area(), 0.5);
        assert_eq!(Capsule::new(Vec3::ZERO, Vec3::new(0.0, 3.0, 0.0), 0.5).length(), 3.0);
        assert_eq!(
            Obb::new(Vec3::ONE, Vec3::splat(1.0), [Vec3::X, Vec3::Y, Vec3::Z]).to_aabb(),
            Aabb::new(Vec3::ZERO, Vec3::splat(2.0))
        );
        assert!(Frustum { planes: [Plane::from_point_normal(Vec3::ZERO, Vec3::Y); 6] }.contains_point(Vec3::Y));
        assert_eq!(Color::rgb(1.0, 0.0, 0.0).to_rgba8(), Rgba8::new(255, 0, 0, 255));
        assert_eq!(Rgba::new(0.0, 1.0, 0.0, 1.0).to_rgba8(), Rgba8::new(0, 255, 0, 255));
        assert_eq!(Rgba8::new(255, 128, 0, 255).to_array(), [255, 128, 0, 255]);
        assert!(Rgba8::new(255, 255, 255, 255).to_rgba().r == 1.0);
        assert!(-Vec3::X == Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(2.0 * Vec3::X, Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(Matrix4::identity() * Matrix4::identity(), Matrix4::identity());
    }
}
