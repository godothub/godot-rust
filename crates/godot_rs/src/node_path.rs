use core::fmt;

/// Owned UTF-8 spelling of a Godot `NodePath`.
///
/// The Script project stores the portable textual form. The Host constructs
/// and destroys Godot's native `NodePath` only around the synchronous engine
/// call, so project modules never depend on the engine's C++ object layout.
#[derive(Clone, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodePath(String);

impl NodePath {
    /// Creates a path from its Godot text spelling.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrows the Godot text spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the path is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consumes the value and returns its UTF-8 spelling.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for NodePath {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for NodePath {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<NodePath> for String {
    fn from(value: NodePath) -> Self {
        value.0
    }
}

impl AsRef<str> for NodePath {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for NodePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("NodePath").field(&self.0).finish()
    }
}

impl fmt::Display for NodePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_paths_are_owned_utf8_spellings() {
        let path = NodePath::from("../玩家/%武器:position");
        assert_eq!(path.as_str(), "../玩家/%武器:position");
        assert!(!path.is_empty());
        assert_eq!(path.clone().into_string(), "../玩家/%武器:position");
        assert_eq!(path.to_string(), "../玩家/%武器:position");
    }
}
