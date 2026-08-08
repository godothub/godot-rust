//! Godot-style copyable math values used by scripts and reflected calls.

use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// A two-dimensional vector using Godot's standard `real_t` precision.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vector2 {
    pub x: f32,
    pub y: f32,
}

impl Vector2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0);
    pub const LEFT: Self = Self::new(-1.0, 0.0);
    pub const RIGHT: Self = Self::new(1.0, 0.0);
    pub const UP: Self = Self::new(0.0, -1.0);
    pub const DOWN: Self = Self::new(0.0, 1.0);

    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn splat(value: f32) -> Self {
        Self::new(value, value)
    }

    #[must_use]
    pub const fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
    }

    #[must_use]
    pub const fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    #[must_use]
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 0.0 && length.is_finite() {
            self / length
        } else {
            Self::ZERO
        }
    }

    #[must_use]
    pub fn lerp(self, to: Self, weight: f32) -> Self {
        self + (to - self) * weight
    }

    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// A three-dimensional vector using Godot's standard `real_t` precision.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vector3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A two-dimensional vector of signed 32-bit integer coordinates.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct Vector2i {
    pub x: i32,
    pub y: i32,
}

impl Vector2i {
    pub const ZERO: Self = Self::new(0, 0);
    pub const ONE: Self = Self::new(1, 1);
    pub const LEFT: Self = Self::new(-1, 0);
    pub const RIGHT: Self = Self::new(1, 0);
    pub const UP: Self = Self::new(0, -1);
    pub const DOWN: Self = Self::new(0, 1);

    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    #[must_use]
    pub const fn splat(value: i32) -> Self {
        Self::new(value, value)
    }

    #[must_use]
    pub const fn dot(self, other: Self) -> i64 {
        self.x as i64 * other.x as i64 + self.y as i64 * other.y as i64
    }
}

/// A three-dimensional vector of signed 32-bit integer coordinates.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct Vector3i {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Vector3i {
    pub const ZERO: Self = Self::new(0, 0, 0);
    pub const ONE: Self = Self::new(1, 1, 1);
    pub const LEFT: Self = Self::new(-1, 0, 0);
    pub const RIGHT: Self = Self::new(1, 0, 0);
    pub const UP: Self = Self::new(0, 1, 0);
    pub const DOWN: Self = Self::new(0, -1, 0);
    pub const FORWARD: Self = Self::new(0, 0, -1);
    pub const BACK: Self = Self::new(0, 0, 1);

    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn splat(value: i32) -> Self {
        Self::new(value, value, value)
    }

    #[must_use]
    pub const fn dot(self, other: Self) -> i64 {
        self.x as i64 * other.x as i64
            + self.y as i64 * other.y as i64
            + self.z as i64 * other.z as i64
    }
}

/// A four-dimensional vector using Godot's standard `real_t` precision.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Vector4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vector4 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0, 1.0, 1.0);

    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    #[must_use]
    pub const fn splat(value: f32) -> Self {
        Self::new(value, value, value, value)
    }

    #[must_use]
    pub const fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    #[must_use]
    pub const fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    #[must_use]
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 0.0 && length.is_finite() {
            self / length
        } else {
            Self::ZERO
        }
    }
}

/// A four-dimensional vector of signed 32-bit integer coordinates.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct Vector4i {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub w: i32,
}

impl Vector4i {
    pub const ZERO: Self = Self::new(0, 0, 0, 0);
    pub const ONE: Self = Self::new(1, 1, 1, 1);

    #[must_use]
    pub const fn new(x: i32, y: i32, z: i32, w: i32) -> Self {
        Self { x, y, z, w }
    }

    #[must_use]
    pub const fn splat(value: i32) -> Self {
        Self::new(value, value, value, value)
    }

    #[must_use]
    pub const fn dot(self, other: Self) -> i64 {
        self.x as i64 * other.x as i64
            + self.y as i64 * other.y as i64
            + self.z as i64 * other.z as i64
            + self.w as i64 * other.w as i64
    }
}

impl Vector3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0, 1.0);
    pub const LEFT: Self = Self::new(-1.0, 0.0, 0.0);
    pub const RIGHT: Self = Self::new(1.0, 0.0, 0.0);
    pub const UP: Self = Self::new(0.0, 1.0, 0.0);
    pub const DOWN: Self = Self::new(0.0, -1.0, 0.0);
    pub const FORWARD: Self = Self::new(0.0, 0.0, -1.0);
    pub const BACK: Self = Self::new(0.0, 0.0, 1.0);

    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn splat(value: f32) -> Self {
        Self::new(value, value, value)
    }

    #[must_use]
    pub const fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    #[must_use]
    pub const fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    #[must_use]
    pub const fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[must_use]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    #[must_use]
    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 0.0 && length.is_finite() {
            self / length
        } else {
            Self::ZERO
        }
    }

    #[must_use]
    pub fn lerp(self, to: Self, weight: f32) -> Self {
        self + (to - self) * weight
    }

    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// A linear RGBA color compatible with Godot's `Color` value.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);
    pub const BLACK: Self = Self::rgba(0.0, 0.0, 0.0, 1.0);
    pub const WHITE: Self = Self::rgba(1.0, 1.0, 1.0, 1.0);
    pub const RED: Self = Self::rgba(1.0, 0.0, 0.0, 1.0);
    pub const GREEN: Self = Self::rgba(0.0, 1.0, 0.0, 1.0);
    pub const BLUE: Self = Self::rgba(0.0, 0.0, 1.0, 1.0);

    #[must_use]
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::rgba(r, g, b, 1.0)
    }

    #[must_use]
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    #[must_use]
    pub const fn with_alpha(self, alpha: f32) -> Self {
        Self::rgba(self.r, self.g, self.b, alpha)
    }

    #[must_use]
    pub fn lerp(self, to: Self, weight: f32) -> Self {
        self + (to - self) * weight
    }

    #[must_use]
    pub const fn is_finite(self) -> bool {
        self.r.is_finite() && self.g.is_finite() && self.b.is_finite() && self.a.is_finite()
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::BLACK
    }
}

/// A Godot floating-point rectangle represented by its position and size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Rect2 {
    pub position: Vector2,
    pub size: Vector2,
}

impl Rect2 {
    #[must_use]
    pub const fn new(position: Vector2, size: Vector2) -> Self {
        Self { position, size }
    }

    #[must_use]
    pub const fn from_components(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self::new(Vector2::new(x, y), Vector2::new(width, height))
    }

    #[must_use]
    pub fn end(self) -> Vector2 {
        self.position + self.size
    }

    #[must_use]
    pub fn has_point(self, point: Vector2) -> bool {
        let end = self.end();
        point.x >= self.position.x
            && point.y >= self.position.y
            && point.x < end.x
            && point.y < end.y
    }
}

/// A Godot integer rectangle represented by its position and size.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct Rect2i {
    pub position: Vector2i,
    pub size: Vector2i,
}

impl Rect2i {
    #[must_use]
    pub const fn new(position: Vector2i, size: Vector2i) -> Self {
        Self { position, size }
    }

    #[must_use]
    pub const fn from_components(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self::new(Vector2i::new(x, y), Vector2i::new(width, height))
    }

    #[must_use]
    pub fn end(self) -> Vector2i {
        self.position + self.size
    }

    #[must_use]
    pub fn has_point(self, point: Vector2i) -> bool {
        let end = self.end();
        point.x >= self.position.x
            && point.y >= self.position.y
            && point.x < end.x
            && point.y < end.y
    }
}

/// A Godot quaternion stored as `(x, y, z, w)`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Quaternion {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quaternion {
    pub const IDENTITY: Self = Self::new(0.0, 0.0, 0.0, 1.0);

    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    #[must_use]
    pub const fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    #[must_use]
    pub const fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[must_use]
    pub fn normalized(self) -> Self {
        let length = self.length_squared().sqrt();
        if length > 0.0 && length.is_finite() {
            self / length
        } else {
            Self::IDENTITY
        }
    }
}

impl Default for Quaternion {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A Godot plane represented by a normal and distance from the origin.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Plane {
    pub normal: Vector3,
    pub d: f32,
}

impl Plane {
    #[must_use]
    pub const fn new(normal: Vector3, d: f32) -> Self {
        Self { normal, d }
    }

    #[must_use]
    pub const fn from_components(x: f32, y: f32, z: f32, d: f32) -> Self {
        Self::new(Vector3::new(x, y, z), d)
    }

    #[must_use]
    pub fn normalized(self) -> Self {
        let length = self.normal.length();
        if length > 0.0 && length.is_finite() {
            Self::new(self.normal / length, self.d / length)
        } else {
            Self::default()
        }
    }

    #[must_use]
    pub const fn distance_to(self, point: Vector3) -> f32 {
        self.normal.dot(point) - self.d
    }
}

/// A Godot 2D affine transform stored as two basis columns and an origin.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Transform2D {
    pub x: Vector2,
    pub y: Vector2,
    pub origin: Vector2,
}

impl Transform2D {
    pub const IDENTITY: Self = Self::new(Vector2::RIGHT, Vector2::DOWN, Vector2::ZERO);

    #[must_use]
    pub const fn new(x: Vector2, y: Vector2, origin: Vector2) -> Self {
        Self { x, y, origin }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __components(&self) -> &[f32] {
        // SAFETY: `repr(C)` and the three `Vector2` fields make this exactly
        // six contiguous f32 components.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast::<f32>(), 6) }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn __from_components(value: [f32; 6]) -> Self {
        Self::new(
            Vector2::new(value[0], value[1]),
            Vector2::new(value[2], value[3]),
            Vector2::new(value[4], value[5]),
        )
    }
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A Godot axis-aligned 3D bounding box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Aabb {
    pub position: Vector3,
    pub size: Vector3,
}

impl Aabb {
    #[must_use]
    pub const fn new(position: Vector3, size: Vector3) -> Self {
        Self { position, size }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __components(&self) -> &[f32] {
        // SAFETY: `repr(C)` and the two `Vector3` fields make this exactly six
        // contiguous f32 components.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast::<f32>(), 6) }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn __from_components(value: [f32; 6]) -> Self {
        Self::new(
            Vector3::new(value[0], value[1], value[2]),
            Vector3::new(value[3], value[4], value[5]),
        )
    }

    #[must_use]
    pub fn end(self) -> Vector3 {
        self.position + self.size
    }
}

/// A Godot 3×3 basis matrix stored in native component order.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Basis {
    pub x: Vector3,
    pub y: Vector3,
    pub z: Vector3,
}

impl Basis {
    pub const IDENTITY: Self = Self::new(Vector3::RIGHT, Vector3::UP, Vector3::BACK);

    #[must_use]
    pub const fn new(x: Vector3, y: Vector3, z: Vector3) -> Self {
        Self { x, y, z }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __components(&self) -> &[f32] {
        // SAFETY: `repr(C)` and the three `Vector3` fields make this exactly
        // nine contiguous f32 components.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast::<f32>(), 9) }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn __from_components(value: [f32; 9]) -> Self {
        Self::new(
            Vector3::new(value[0], value[1], value[2]),
            Vector3::new(value[3], value[4], value[5]),
            Vector3::new(value[6], value[7], value[8]),
        )
    }
}

impl Default for Basis {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A Godot 3D affine transform.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Transform3D {
    pub basis: Basis,
    pub origin: Vector3,
}

impl Transform3D {
    pub const IDENTITY: Self = Self::new(Basis::IDENTITY, Vector3::ZERO);

    #[must_use]
    pub const fn new(basis: Basis, origin: Vector3) -> Self {
        Self { basis, origin }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __components(&self) -> &[f32] {
        // SAFETY: `repr(C)`, `Basis`, and `Vector3` make this exactly twelve
        // contiguous f32 components.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast::<f32>(), 12) }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn __from_components(value: [f32; 12]) -> Self {
        Self::new(
            Basis::__from_components([
                value[0], value[1], value[2], value[3], value[4], value[5], value[6], value[7],
                value[8],
            ]),
            Vector3::new(value[9], value[10], value[11]),
        )
    }
}

impl Default for Transform3D {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A Godot 4×4 projection matrix stored as four native Vector4 columns.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct Projection {
    pub x: Vector4,
    pub y: Vector4,
    pub z: Vector4,
    pub w: Vector4,
}

impl Projection {
    pub const IDENTITY: Self = Self::new(
        Vector4::new(1.0, 0.0, 0.0, 0.0),
        Vector4::new(0.0, 1.0, 0.0, 0.0),
        Vector4::new(0.0, 0.0, 1.0, 0.0),
        Vector4::new(0.0, 0.0, 0.0, 1.0),
    );

    #[must_use]
    pub const fn new(x: Vector4, y: Vector4, z: Vector4, w: Vector4) -> Self {
        Self { x, y, z, w }
    }

    #[doc(hidden)]
    #[must_use]
    pub fn __components(&self) -> &[f32] {
        // SAFETY: `repr(C)` and the four `Vector4` fields make this exactly
        // sixteen contiguous f32 components.
        unsafe { core::slice::from_raw_parts(core::ptr::from_ref(self).cast::<f32>(), 16) }
    }

    #[doc(hidden)]
    #[must_use]
    pub const fn __from_components(value: [f32; 16]) -> Self {
        Self::new(
            Vector4::new(value[0], value[1], value[2], value[3]),
            Vector4::new(value[4], value[5], value[6], value[7]),
            Vector4::new(value[8], value[9], value[10], value[11]),
            Vector4::new(value[12], value[13], value[14], value[15]),
        )
    }
}

impl Default for Projection {
    fn default() -> Self {
        Self::IDENTITY
    }
}

macro_rules! vector_operators {
    ($type:ty { $($field:ident),+ $(,)? }) => {
        impl Add for $type {
            type Output = Self;

            fn add(self, other: Self) -> Self {
                Self { $($field: self.$field + other.$field),+ }
            }
        }

        impl AddAssign for $type {
            fn add_assign(&mut self, other: Self) {
                $(self.$field += other.$field;)+
            }
        }

        impl Sub for $type {
            type Output = Self;

            fn sub(self, other: Self) -> Self {
                Self { $($field: self.$field - other.$field),+ }
            }
        }

        impl SubAssign for $type {
            fn sub_assign(&mut self, other: Self) {
                $(self.$field -= other.$field;)+
            }
        }

        impl Mul<f32> for $type {
            type Output = Self;

            fn mul(self, scalar: f32) -> Self {
                Self { $($field: self.$field * scalar),+ }
            }
        }

        impl MulAssign<f32> for $type {
            fn mul_assign(&mut self, scalar: f32) {
                $(self.$field *= scalar;)+
            }
        }

        impl Div<f32> for $type {
            type Output = Self;

            fn div(self, scalar: f32) -> Self {
                Self { $($field: self.$field / scalar),+ }
            }
        }

        impl DivAssign<f32> for $type {
            fn div_assign(&mut self, scalar: f32) {
                $(self.$field /= scalar;)+
            }
        }

        impl Neg for $type {
            type Output = Self;

            fn neg(self) -> Self {
                Self { $($field: -self.$field),+ }
            }
        }
    };
}

vector_operators!(Vector2 { x, y });
vector_operators!(Vector3 { x, y, z });
vector_operators!(Vector4 { x, y, z, w });
vector_operators!(Quaternion { x, y, z, w });

macro_rules! integer_vector_operators {
    ($type:ty { $($field:ident),+ $(,)? }) => {
        impl Add for $type {
            type Output = Self;

            fn add(self, other: Self) -> Self {
                Self { $($field: self.$field + other.$field),+ }
            }
        }

        impl AddAssign for $type {
            fn add_assign(&mut self, other: Self) {
                $(self.$field += other.$field;)+
            }
        }

        impl Sub for $type {
            type Output = Self;

            fn sub(self, other: Self) -> Self {
                Self { $($field: self.$field - other.$field),+ }
            }
        }

        impl SubAssign for $type {
            fn sub_assign(&mut self, other: Self) {
                $(self.$field -= other.$field;)+
            }
        }

        impl Mul<i32> for $type {
            type Output = Self;

            fn mul(self, scalar: i32) -> Self {
                Self { $($field: self.$field * scalar),+ }
            }
        }

        impl MulAssign<i32> for $type {
            fn mul_assign(&mut self, scalar: i32) {
                $(self.$field *= scalar;)+
            }
        }

        impl Div<i32> for $type {
            type Output = Self;

            fn div(self, scalar: i32) -> Self {
                Self { $($field: self.$field / scalar),+ }
            }
        }

        impl DivAssign<i32> for $type {
            fn div_assign(&mut self, scalar: i32) {
                $(self.$field /= scalar;)+
            }
        }

        impl Neg for $type {
            type Output = Self;

            fn neg(self) -> Self {
                Self { $($field: -self.$field),+ }
            }
        }
    };
}

integer_vector_operators!(Vector2i { x, y });
integer_vector_operators!(Vector3i { x, y, z });
integer_vector_operators!(Vector4i { x, y, z, w });
vector_operators!(Color { r, g, b, a });

impl Mul for Color {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        Self::rgba(
            self.r * other.r,
            self.g * other.g,
            self.b * other.b,
            self.a * other.a,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(left: f32, right: f32) -> bool {
        (left - right).abs() <= 0.000_01
    }

    #[test]
    fn vector2_uses_godot_screen_directions_and_normalization() {
        assert_eq!(Vector2::UP, Vector2::new(0.0, -1.0));
        let normalized = Vector2::new(3.0, 4.0).normalized();
        assert!(near(normalized.x, 0.6));
        assert!(near(normalized.y, 0.8));
        assert_eq!(Vector2::ZERO.normalized(), Vector2::ZERO);
        assert_eq!(
            Vector2::ZERO.lerp(Vector2::splat(10.0), 0.25),
            Vector2::splat(2.5)
        );
    }

    #[test]
    fn vector3_cross_product_matches_godot_axes() {
        assert_eq!(
            Vector3::RIGHT.cross(Vector3::UP),
            Vector3::new(0.0, 0.0, 1.0)
        );
        assert!(near(Vector3::new(2.0, 3.0, 6.0).length(), 7.0));
    }

    #[test]
    fn integer_vectors_preserve_godot_axes_and_exact_arithmetic() {
        assert_eq!(Vector2i::UP, Vector2i::new(0, -1));
        assert_eq!(
            Vector2i::new(3, -4) + Vector2i::new(5, 6),
            Vector2i::new(8, 2)
        );
        assert_eq!(Vector3i::RIGHT.dot(Vector3i::LEFT), -1);
        assert_eq!(Vector3i::new(1, 2, 3) * 4, Vector3i::new(4, 8, 12));
    }

    #[test]
    fn colors_default_to_opaque_black_and_compose_components() {
        assert_eq!(Color::default(), Color::BLACK);
        assert_eq!(Color::RED.with_alpha(0.5), Color::rgba(1.0, 0.0, 0.0, 0.5));
        assert_eq!(
            Color::rgba(0.5, 0.5, 1.0, 0.5) * Color::rgba(0.5, 1.0, 0.25, 0.5),
            Color::rgba(0.25, 0.5, 0.25, 0.25)
        );
    }

    #[test]
    fn rectangles_quaternions_and_planes_follow_godot_component_order() {
        let rectangle = Rect2::from_components(10.0, 20.0, 30.0, 40.0);
        assert_eq!(rectangle.end(), Vector2::new(40.0, 60.0));
        assert!(rectangle.has_point(Vector2::new(10.0, 20.0)));
        assert!(!rectangle.has_point(Vector2::new(40.0, 60.0)));

        let integer_rectangle = Rect2i::from_components(-2, 3, 4, 5);
        assert_eq!(integer_rectangle.end(), Vector2i::new(2, 8));

        assert_eq!(Quaternion::default(), Quaternion::IDENTITY);
        assert_eq!(
            Quaternion::new(0.0, 0.0, 0.0, 2.0).normalized(),
            Quaternion::IDENTITY
        );

        let plane = Plane::new(Vector3::UP, 2.0);
        assert_eq!(plane.distance_to(Vector3::new(0.0, 5.0, 0.0)), 3.0);
    }

    #[test]
    fn four_dimensional_vectors_preserve_exact_arithmetic() {
        assert_eq!(
            Vector4::new(1.0, 2.0, 3.0, 4.0) * 2.0,
            Vector4::new(2.0, 4.0, 6.0, 8.0)
        );
        assert_eq!(Vector4i::new(1, -2, 3, -4).dot(Vector4i::ONE), -2);
    }
}
