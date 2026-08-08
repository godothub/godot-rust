/// Runtime Godot version reported through the official GDExtension interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EngineVersion {
    pub(crate) major: u32,
    pub(crate) minor: u32,
    pub(crate) patch: u32,
}

/// Returns whether the 4.4-baseline Host may run in this engine.
pub(crate) const fn is_supported_godot(version: EngineVersion) -> bool {
    version.major == 4 && version.minor >= 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_version_policy_uses_the_4_4_baseline() {
        assert!(!is_supported_godot(EngineVersion {
            major: 4,
            minor: 3,
            patch: 9,
        }));
        assert!(is_supported_godot(EngineVersion {
            major: 4,
            minor: 4,
            patch: 0,
        }));
        assert!(is_supported_godot(EngineVersion {
            major: 4,
            minor: 7,
            patch: 3,
        }));
        assert!(!is_supported_godot(EngineVersion {
            major: 5,
            minor: 0,
            patch: 0,
        }));
    }
}
