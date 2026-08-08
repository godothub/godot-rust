use std::path::{Path, PathBuf};

const MAX_DEP_INFO_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DEPENDENCY_COUNT: usize = 100_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtifactDepInfo {
    pub path: PathBuf,
    pub dependencies: Vec<PathBuf>,
}

pub(crate) fn read_artifact_dep_info(
    artifact: &Path,
    cargo_root: &Path,
) -> Result<ArtifactDepInfo, String> {
    let mut path = artifact.to_owned();
    path.set_extension("d");
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        format!(
            "could not inspect Cargo dependency information `{}`: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "Cargo dependency information is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_DEP_INFO_BYTES {
        return Err(format!(
            "Cargo dependency information exceeds the {MAX_DEP_INFO_BYTES} byte safety limit"
        ));
    }
    let source = std::fs::read(&path).map_err(|error| {
        format!(
            "could not read Cargo dependency information `{}`: {error}",
            path.display()
        )
    })?;
    let dependencies = parse_dep_info(&source)?
        .into_iter()
        .map(|dependency| {
            if dependency.is_absolute() {
                dependency
            } else {
                cargo_root.join(dependency)
            }
        })
        .map(|dependency| {
            let metadata = std::fs::symlink_metadata(&dependency).map_err(|error| {
                format!(
                    "could not inspect Cargo source input `{}`: {error}",
                    dependency.display()
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "Cargo source input is not a regular file: {}",
                    dependency.display()
                ));
            }
            dependency.canonicalize().map_err(|error| {
                format!(
                    "could not resolve Cargo source input `{}`: {error}",
                    dependency.display()
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let path = path.canonicalize().map_err(|error| {
        format!(
            "could not resolve Cargo dependency information `{}`: {error}",
            path.display()
        )
    })?;
    Ok(ArtifactDepInfo { path, dependencies })
}

fn parse_dep_info(source: &[u8]) -> Result<Vec<PathBuf>, String> {
    if source.contains(&0) {
        return Err("Cargo dependency information contains a NUL byte".to_owned());
    }
    let delimiter = rule_delimiter(source)
        .ok_or_else(|| "Cargo dependency information has no Makefile rule".to_owned())?;
    let rule = first_logical_rule(&source[delimiter + 1..]);
    let fields = parse_make_fields(rule)?;
    if fields.is_empty() {
        return Err("Cargo dependency information contains no source inputs".to_owned());
    }
    if fields.len() > MAX_DEPENDENCY_COUNT {
        return Err(format!(
            "Cargo dependency information contains more than {MAX_DEPENDENCY_COUNT} inputs"
        ));
    }
    fields
        .into_iter()
        .map(bytes_to_path)
        .collect::<Result<Vec<_>, _>>()
}

fn rule_delimiter(source: &[u8]) -> Option<usize> {
    let mut escaped = false;
    for (index, byte) in source.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if byte == b':' && source.get(index + 1).is_none_or(u8::is_ascii_whitespace) {
            return Some(index);
        }
        if matches!(byte, b'\r' | b'\n') {
            return None;
        }
    }
    None
}

fn first_logical_rule(source: &[u8]) -> &[u8] {
    let mut index = 0;
    while index < source.len() {
        match source[index] {
            b'\\' if source.get(index + 1) == Some(&b'\n') => index += 2,
            b'\\'
                if source.get(index + 1) == Some(&b'\r')
                    && source.get(index + 2) == Some(&b'\n') =>
            {
                index += 3;
            }
            b'\r' | b'\n' => return &source[..index],
            _ => index += 1,
        }
    }
    source
}

fn parse_make_fields(source: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut fields = Vec::new();
    let mut field = Vec::new();
    let mut index = 0;
    while index < source.len() {
        let byte = source[index];
        if byte == b'\\' {
            let Some(next) = source.get(index + 1).copied() else {
                return Err("Cargo dependency information ends with an escape".to_owned());
            };
            match next {
                b'\n' => index += 2,
                b'\r' if source.get(index + 2) == Some(&b'\n') => index += 3,
                b' ' | b'#' | b'\\' => {
                    field.push(next);
                    index += 2;
                }
                _ => {
                    field.push(b'\\');
                    index += 1;
                }
            }
        } else if byte.is_ascii_whitespace() {
            if !field.is_empty() {
                fields.push(std::mem::take(&mut field));
            }
            index += 1;
        } else {
            field.push(byte);
            index += 1;
        }
    }
    if !field.is_empty() {
        fields.push(field);
    }
    Ok(fields)
}

#[cfg(unix)]
fn bytes_to_path(bytes: Vec<u8>) -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: Vec<u8>) -> Result<PathBuf, String> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| "Cargo dependency information contains a non-UTF-8 path".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_absolute_inputs_and_make_escapes() {
        let source =
            b"/tmp/target/libgame.rlib: /tmp/src/lib.rs /tmp/has\\ space.rs /tmp/hash\\#name.rs\n";
        assert_eq!(
            parse_dep_info(source).expect("dependency information"),
            [
                PathBuf::from("/tmp/src/lib.rs"),
                PathBuf::from("/tmp/has space.rs"),
                PathBuf::from("/tmp/hash#name.rs"),
            ]
        );
    }

    #[test]
    fn parses_continuations_and_windows_drive_colons() {
        let source =
            b"C:\\\\target\\\\game.rlib: C:\\\\src\\\\lib.rs \\\r\n C:\\\\src\\\\other.rs\r\n";
        assert_eq!(
            parse_dep_info(source).expect("dependency information"),
            [
                PathBuf::from(r"C:\src\lib.rs"),
                PathBuf::from(r"C:\src\other.rs"),
            ]
        );
    }

    #[test]
    fn preserves_unescaped_windows_path_separators() {
        let source = b"C:\\target\\game.rlib: C:\\src\\lib.rs C:\\src\\other.rs\r\n";
        assert_eq!(
            parse_dep_info(source).expect("Windows dependency information"),
            [
                PathBuf::from(r"C:\src\lib.rs"),
                PathBuf::from(r"C:\src\other.rs"),
            ]
        );
    }

    #[test]
    fn rejects_incomplete_or_empty_rules() {
        assert!(parse_dep_info(b"not a rule\n").is_err());
        assert!(parse_dep_info(b"target:\n").is_err());
        assert!(parse_dep_info(b"target: source\\").is_err());
        assert!(parse_dep_info(b"target: source\0hidden\n").is_err());
    }
}
