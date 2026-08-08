use core::fmt;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

const MAX_UID_FILE_BYTES: usize = 64;
pub(crate) const INVALID_RESOURCE_UID: i64 = -1;

#[derive(Debug)]
pub(crate) enum ResourceUidError {
    Io(io::Error),
    TooLarge(usize),
    InvalidUtf8,
    InvalidText,
}

impl fmt::Display for ResourceUidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "could not read Resource UID file: {error}"),
            Self::TooLarge(size) => {
                write!(formatter, "Resource UID file is too large: {size} bytes")
            }
            Self::InvalidUtf8 => formatter.write_str("Resource UID file is not UTF-8"),
            Self::InvalidText => {
                formatter.write_str("Resource UID file has invalid canonical text")
            }
        }
    }
}

pub(crate) fn uid_file_path(resource_path: &Path) -> PathBuf {
    let mut path = OsString::from(resource_path.as_os_str());
    path.push(".uid");
    PathBuf::from(path)
}

pub(crate) fn read_uid_file(resource_path: &Path) -> Result<Option<i64>, ResourceUidError> {
    let path = uid_file_path(resource_path);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ResourceUidError::Io(error)),
    };
    parse_uid_file_bytes(&bytes).map(Some)
}

pub(crate) fn to_text(mut uid: i64) -> Option<String> {
    const ALPHABET: &[u8; 34] = b"abcdefghijklmnopqrstuvwxy012345678";
    if uid < 0 {
        return None;
    }
    let mut reversed = [0_u8; 13];
    let mut length = 0;
    loop {
        reversed[length] = ALPHABET[(uid % 34) as usize];
        length += 1;
        uid /= 34;
        if uid == 0 {
            break;
        }
    }
    let mut result = String::with_capacity(6 + length);
    result.push_str("uid://");
    result.extend(
        reversed[..length]
            .iter()
            .rev()
            .map(|byte| char::from(*byte)),
    );
    Some(result)
}

fn parse_uid_file_bytes(bytes: &[u8]) -> Result<i64, ResourceUidError> {
    if bytes.len() > MAX_UID_FILE_BYTES {
        return Err(ResourceUidError::TooLarge(bytes.len()));
    }
    let text = core::str::from_utf8(bytes).map_err(|_| ResourceUidError::InvalidUtf8)?;
    let line = text.lines().next().unwrap_or_default();
    parse_text(line).ok_or(ResourceUidError::InvalidText)
}

pub(crate) fn parse_text(text: &str) -> Option<i64> {
    let digits = text.as_bytes().strip_prefix(b"uid://")?;
    if digits.is_empty() || (digits.len() > 1 && digits[0] == b'a') {
        return None;
    }
    let mut uid = 0_u64;
    for digit in digits {
        let value = match *digit {
            b'a'..=b'y' => u64::from(*digit - b'a'),
            b'0'..=b'8' => u64::from(*digit - b'0') + 25,
            _ => return None,
        };
        uid = uid.checked_mul(34)?.checked_add(value)?;
    }
    (uid <= i64::MAX as u64).then_some(uid as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_file_parser_matches_godot_text_files() {
        let expected = parse_text("uid://cttch2f1b2wi6").expect("test UID");
        assert_eq!(
            parse_uid_file_bytes(b"uid://cttch2f1b2wi6\n").expect("valid UID file"),
            expected
        );
        assert!(matches!(
            parse_uid_file_bytes(b"uid://<invalid>\n"),
            Err(ResourceUidError::InvalidText)
        ));
        assert!(matches!(
            parse_uid_file_bytes(&[b'a'; MAX_UID_FILE_BYTES + 1]),
            Err(ResourceUidError::TooLarge(_))
        ));
        assert_eq!(
            uid_file_path(Path::new("/tmp/player.rs")),
            PathBuf::from("/tmp/player.rs.uid")
        );
        assert_eq!(to_text(0).as_deref(), Some("uid://a"));
        let uid = parse_text("uid://cttch2f1b2wi6").expect("test UID");
        assert_eq!(to_text(uid).as_deref(), Some("uid://cttch2f1b2wi6"));
        assert_eq!(to_text(INVALID_RESOURCE_UID), None);
        assert!(parse_text("uid://aa").is_none());
        assert!(parse_text("uid://z").is_none());
    }
}
