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
    let fields = parse_cargo_fields(rule)?;
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

fn parse_cargo_fields(source: &[u8]) -> Result<Vec<Vec<u8>>, String> {
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
                // Cargo only escapes spaces in its artifact .d files. In particular,
                // backslashes in Windows prefixes and before '#' are literal.
                b' ' => {
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("godot-rust-dep-info-{}-{id}", std::process::id()));
            std::fs::create_dir(&path).expect("temporary directory");
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_absolute_inputs_and_cargo_space_escapes() {
        let source =
            b"/tmp/target/libgame.rlib: /tmp/src/lib.rs /tmp/has\\ space.rs /tmp/hash#name.rs /tmp/$value.rs\n\n/tmp/src/lib.rs:\n\n# env-dep:VALUE=value\n";
        assert_eq!(
            parse_dep_info(source).expect("dependency information"),
            [
                PathBuf::from("/tmp/src/lib.rs"),
                PathBuf::from("/tmp/has space.rs"),
                PathBuf::from("/tmp/hash#name.rs"),
                PathBuf::from("/tmp/$value.rs"),
            ]
        );
    }

    #[test]
    fn parses_continuations_and_windows_drive_colons() {
        for newline in ["\n", "\r\n"] {
            let source = format!(
                "{}{}{}{}{}",
                r"C:\target\game.rlib: C:\src\lib.rs ", '\\', newline, r" C:\src\other.rs", newline,
            );
            assert_eq!(
                parse_dep_info(source.as_bytes()).expect("dependency information"),
                [
                    PathBuf::from(r"C:\src\lib.rs"),
                    PathBuf::from(r"C:\src\other.rs"),
                ]
            );
        }
    }

    #[test]
    fn preserves_windows_prefixes_and_literal_path_characters() {
        for (encoded, expected) in [
            (
                r"H:\Work\godot_rust_test\src\lib.rs",
                r"H:\Work\godot_rust_test\src\lib.rs",
            ),
            (
                r"\\?\H:\Work\godot_rust_test\src\lib.rs",
                r"\\?\H:\Work\godot_rust_test\src\lib.rs",
            ),
            (r"\\server\share\src\lib.rs", r"\\server\share\src\lib.rs"),
            (
                r"\\?\UNC\server\share\src\lib.rs",
                r"\\?\UNC\server\share\src\lib.rs",
            ),
            (r"C:\#project\$src\lib.rs", r"C:\#project\$src\lib.rs"),
            (
                r"\\?\C:\开发\has\ space\#scripts\$value.rs",
                r"\\?\C:\开发\has space\#scripts\$value.rs",
            ),
            (
                r"\\server\share\has\ space\#scripts\lib.rs",
                r"\\server\share\has space\#scripts\lib.rs",
            ),
            (
                r"\\?\UNC\server\share\开发\has\ space\lib.rs",
                r"\\?\UNC\server\share\开发\has space\lib.rs",
            ),
        ] {
            let source = format!(r"C:\target\game.rlib: {encoded}");
            let dependencies = parse_dep_info(source.as_bytes()).expect("Windows dependency");
            assert_eq!(dependencies.len(), 1, "{encoded}");
            // Compare the original spelling as well as Path semantics: Windows Path
            // comparisons can hide changes to separators.
            assert_eq!(dependencies[0].as_os_str(), expected, "{encoded}");
            #[cfg(windows)]
            assert!(dependencies[0].is_absolute(), "{encoded}");
        }
    }

    #[test]
    fn preserves_literal_backslashes_in_unix_path_spellings() {
        for (encoded, expected) in [
            (r"/tmp/double\\folder/lib.rs", r"/tmp/double\\folder/lib.rs"),
            (r"/tmp/slash\#folder/lib.rs", r"/tmp/slash\#folder/lib.rs"),
            (r"/tmp/slash\\ folder/lib.rs", r"/tmp/slash\ folder/lib.rs"),
            (r"/tmp/$$value/lib.rs", r"/tmp/$$value/lib.rs"),
        ] {
            let source = format!("/tmp/target/libgame.rlib: {encoded}\n");
            let dependencies = parse_dep_info(source.as_bytes()).expect("literal path characters");
            assert_eq!(dependencies.len(), 1, "{encoded}");
            assert_eq!(dependencies[0].as_os_str(), expected, "{encoded}");
        }
    }

    #[test]
    fn reads_canonical_absolute_and_relative_source_files() {
        let temporary = TempDirectory::new();
        let root = temporary.0.join("#project $ 中文 with space");
        std::fs::create_dir_all(root.join("src")).expect("source directory");
        let root = root.canonicalize().expect("canonical project");
        let absolute = root.join("src/lib.rs");
        let relative = root.join("src/other.rs");
        std::fs::write(&absolute, b"pub fn example() {}\n").expect("absolute source");
        std::fs::write(&relative, b"pub fn other() {}\n").expect("relative source");
        let absolute = absolute.canonicalize().expect("canonical source");
        #[cfg(windows)]
        assert!(matches!(
            absolute.components().next(),
            Some(std::path::Component::Prefix(prefix)) if prefix.kind().is_verbatim()
        ));
        let artifact = root.join("game.rlib");
        let dep_info_path = artifact.with_extension("d");
        let source = format!(
            "{}: {} src/other.rs\n",
            artifact
                .to_str()
                .expect("artifact path")
                .replace(' ', "\\ "),
            absolute.to_str().expect("source path").replace(' ', "\\ "),
        );
        std::fs::write(&dep_info_path, source).expect("dependency information");

        let dep_info = read_artifact_dep_info(&artifact, &root).expect("source inputs");
        assert_eq!(
            dep_info.dependencies,
            [absolute, relative.canonicalize().expect("canonical source")]
        );
        assert_eq!(
            dep_info.path,
            dep_info_path.canonicalize().expect("canonical dep-info")
        );
    }

    #[test]
    fn reads_real_cargo_dep_info_with_special_path_characters() {
        let temporary = TempDirectory::new();
        let root = temporary.0.join("#project $ 中文 with space");
        #[cfg(unix)]
        let root = root.join(r"literal\\ slashes\#and\ space");
        std::fs::create_dir_all(root.join("src")).expect("source directory");
        let root = root.canonicalize().expect("canonical project");
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"dep_info_fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n",
        )
        .expect("Cargo manifest");
        let source = root.join("src/lib.rs");
        std::fs::write(&source, b"pub fn example() {}\n").expect("Rust source");
        let target = root.join("target");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = std::process::Command::new(cargo)
            .args(["build", "--offline", "--lib", "--message-format=json"])
            // This fixture only generates dep-info. It does not rebuild std with
            // the sanitizer settings used by the outer test process.
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTDOCFLAGS")
            .env_remove("CARGO_ENCODED_RUSTDOCFLAGS")
            .arg("--target-dir")
            .arg(&target)
            .current_dir(&root)
            .output()
            .expect("run Cargo for a dependency-free library");
        assert!(
            output.status.success(),
            "Cargo failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let artifact = String::from_utf8(output.stdout)
            .expect("Cargo JSON")
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|message| message["reason"] == "compiler-artifact")
            .flat_map(|message| message["filenames"].as_array().cloned().unwrap_or_default())
            .filter_map(|value| value.as_str().map(PathBuf::from))
            .find(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "rlib")
            })
            .expect("Cargo library artifact");
        let dep_info = read_artifact_dep_info(&artifact, &root).expect("real Cargo source inputs");
        assert_eq!(
            dep_info.dependencies,
            [source.canonicalize().expect("canonical source")]
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
