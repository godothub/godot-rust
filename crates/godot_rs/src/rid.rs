/// Opaque handle to a Godot RenderingServer, PhysicsServer, or other server resource.
///
/// A `Rid` is copyable and may be stored in ordinary Rust state. Godot owns
/// the referenced resource; copying this value does not extend its lifetime.
/// The default value is invalid and can be used where Godot accepts an empty RID.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Rid {
    raw: u64,
}

impl Rid {
    /// The invalid RID returned by Godot when no server resource is assigned.
    pub const INVALID: Self = Self { raw: 0 };

    /// Whether this handle contains a non-zero Godot RID.
    ///
    /// This does not prove that the resource has not subsequently been freed.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.raw != 0
    }

    /// Returns Godot's opaque numeric RID identifier.
    #[must_use]
    pub const fn id(self) -> u64 {
        self.raw
    }

    #[must_use]
    pub(crate) const fn from_raw(raw: u64) -> Self {
        Self { raw }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_rid_is_safe_and_non_forgeable_through_public_constructors() {
        assert_eq!(Rid::default(), Rid::INVALID);
        assert!(!Rid::INVALID.is_valid());
        assert_eq!(Rid::INVALID.id(), 0);

        let live = Rid::from_raw(u64::MAX);
        assert!(live.is_valid());
        assert_eq!(live.id(), u64::MAX);
    }
}
