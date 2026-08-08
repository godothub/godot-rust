use core::borrow::Borrow;
use core::fmt;
use core::ops::Deref;

/// UTF-8 spelling of an interned Godot `StringName`.
///
/// Project modules keep ordinary owned UTF-8 instead of exposing Godot's
/// process-local intern pointer. The Host converts this value to and from the
/// native `StringName` representation at the engine boundary.
#[derive(Clone, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StringName(String);

impl StringName {
    /// Creates a StringName spelling from owned or borrowed UTF-8.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the UTF-8 spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the owned UTF-8 spelling.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl Deref for StringName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for StringName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for StringName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl From<String> for StringName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for StringName {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<StringName> for String {
    fn from(value: StringName) -> Self {
        value.into_string()
    }
}

impl fmt::Debug for StringName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("StringName").field(&self.0).finish()
    }
}

impl fmt::Display for StringName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_names_are_ergonomic_owned_utf8_spellings() {
        let name = StringName::from("玩家/生命值");
        assert_eq!(name.as_str(), "玩家/生命值");
        assert_eq!(&*name, "玩家/生命值");
        assert_eq!(name.to_string(), "玩家/生命值");
        assert_eq!(String::from(name), "玩家/生命值");
    }
}
