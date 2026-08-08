use crate::module_artifact::{MAX_MODULE_BYTES, ensure_regular_module};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_XCFRAMEWORK_FILES: usize = 64;
const MAX_TOOL_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const IOS_PROJECT_MODULE_FILE: &str = "libgodot_rs_project_module.dylib";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct XcframeworkIdentity {
    pub sha256: String,
    pub byte_len: u64,
    pub file_count: usize,
}

pub(crate) fn create_ios_xcframework(
    device_arm64: &Path,
    simulator_arm64: &Path,
    simulator_x86_64: &Path,
    destination: &Path,
) -> Result<XcframeworkIdentity, String> {
    for artifact in [device_arm64, simulator_arm64, simulator_x86_64] {
        ensure_regular_module(artifact)?;
    }
    if destination.extension().and_then(OsStr::to_str) != Some("xcframework") {
        return Err(format!(
            "iOS project module destination must end in `.xcframework`: {}",
            destination.display()
        ));
    }
    match std::fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(format!(
                "refusing to replace an existing iOS XCFramework: {}",
                destination.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "could not inspect iOS XCFramework destination `{}`: {error}",
                destination.display()
            ));
        }
    }
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "iOS XCFramework destination has no parent: {}",
            destination.display()
        )
    })?;
    let source_module_file = device_arm64
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| is_safe_component(name))
        .ok_or_else(|| {
            format!(
                "iOS project module has an unsafe file name: {}",
                device_arm64.display()
            )
        })?;
    for artifact in [simulator_arm64, simulator_x86_64] {
        if artifact.file_name() != Some(OsStr::new(source_module_file)) {
            return Err(
                "iOS device and simulator Cargo artifacts must have the same file name".to_owned(),
            );
        }
    }
    let staging_directory = parent.join(".godot-rust-xcframework-input");
    std::fs::create_dir(&staging_directory).map_err(|error| {
        format!(
            "could not create iOS XCFramework staging directory `{}`: {error}",
            staging_directory.display()
        )
    })?;
    let result = (|| {
        let device_directory = staging_directory.join("device");
        let simulator_directory = staging_directory.join("simulator");
        std::fs::create_dir(&device_directory).map_err(|error| {
            format!(
                "could not create iOS device staging directory `{}`: {error}",
                device_directory.display()
            )
        })?;
        std::fs::create_dir(&simulator_directory).map_err(|error| {
            format!(
                "could not create iOS Simulator staging directory `{}`: {error}",
                simulator_directory.display()
            )
        })?;
        let staged_device = device_directory.join(IOS_PROJECT_MODULE_FILE);
        std::fs::copy(device_arm64, &staged_device).map_err(|error| {
            format!(
                "could not stage iOS device project module `{}`: {error}",
                device_arm64.display()
            )
        })?;
        ensure_regular_module(&staged_device)?;
        let simulator_universal = simulator_directory.join(IOS_PROJECT_MODULE_FILE);
        run_bounded_command(
            Command::new("lipo")
                .args(["-create", "-output"])
                .arg(&simulator_universal)
                .arg(simulator_arm64)
                .arg(simulator_x86_64),
            "`lipo` for the iOS Simulator Universal module",
        )?;
        ensure_regular_module(&simulator_universal)?;
        run_bounded_command(
            Command::new("lipo").arg(&simulator_universal).args([
                "-verify_arch",
                "arm64",
                "x86_64",
            ]),
            "`lipo` iOS Simulator architecture validation",
        )?;
        run_bounded_command(
            Command::new("xcodebuild")
                .arg("-create-xcframework")
                .arg("-library")
                .arg(&staged_device)
                .arg("-library")
                .arg(&simulator_universal)
                .arg("-output")
                .arg(destination),
            "`xcodebuild -create-xcframework` for the iOS project module",
        )?;
        inspect_ios_xcframework(destination)
    })();
    let cleanup = std::fs::remove_dir_all(&staging_directory).map_err(|error| {
        format!(
            "could not remove iOS XCFramework staging directory `{}`: {error}",
            staging_directory.display()
        )
    });
    match (result, cleanup) {
        (Ok(identity), Ok(())) => Ok(identity),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

pub(crate) fn inspect_ios_xcframework(path: &Path) -> Result<XcframeworkIdentity, String> {
    let root = canonical_xcframework(path)?;
    let files = collect_files(&root)?;
    let info_plist = root.join("Info.plist");
    if !files.iter().any(|path| path == &info_plist) {
        return Err(format!(
            "iOS XCFramework is missing Info.plist: {}",
            root.display()
        ));
    }
    let plist = std::fs::read(&info_plist).map_err(|error| {
        format!(
            "could not read iOS XCFramework Info.plist `{}`: {error}",
            info_plist.display()
        )
    })?;
    let plist_text = std::str::from_utf8(&plist)
        .map_err(|_| "iOS XCFramework Info.plist is not UTF-8 XML".to_owned())?;
    for required in [
        "AvailableLibraries",
        "SupportedArchitectures",
        "SupportedPlatform",
        "SupportedPlatformVariant",
        "simulator",
        "arm64",
        "x86_64",
        "ios",
    ] {
        if !plist_text.contains(required) {
            return Err(format!(
                "iOS XCFramework Info.plist does not declare `{required}`"
            ));
        }
    }
    let dylibs = files
        .iter()
        .filter(|path| path.extension().and_then(OsStr::to_str) == Some("dylib"))
        .collect::<Vec<_>>();
    if dylibs.len() != 2 {
        return Err(format!(
            "iOS XCFramework must contain exactly two dynamic libraries, found {}",
            dylibs.len()
        ));
    }
    hash_xcframework_files(&root, &files)
}

pub(crate) fn copy_ios_xcframework(
    source: &Path,
    destination: &Path,
) -> Result<XcframeworkIdentity, String> {
    let source = canonical_xcframework(source)?;
    let expected = inspect_ios_xcframework(&source)?;
    match std::fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(format!(
                "refusing to replace an existing iOS XCFramework: {}",
                destination.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "could not inspect iOS XCFramework destination `{}`: {error}",
                destination.display()
            ));
        }
    }
    std::fs::create_dir(destination).map_err(|error| {
        format!(
            "could not create staged iOS XCFramework `{}`: {error}",
            destination.display()
        )
    })?;
    copy_directory_contents(&source, destination)?;
    let actual = inspect_ios_xcframework(destination)?;
    if actual != expected {
        return Err("copied iOS XCFramework does not match its source".to_owned());
    }
    Ok(actual)
}

fn canonical_xcframework(path: &Path) -> Result<PathBuf, String> {
    if path.extension().and_then(OsStr::to_str) != Some("xcframework") {
        return Err(format!(
            "iOS artifact is not an XCFramework: {}",
            path.display()
        ));
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        format!(
            "could not inspect iOS XCFramework `{}`: {error}",
            path.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "iOS XCFramework must be a non-symlink directory: {}",
            path.display()
        ));
    }
    path.canonicalize().map_err(|error| {
        format!(
            "could not resolve iOS XCFramework `{}`: {error}",
            path.display()
        )
    })
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let mut entries = std::fs::read_dir(&directory)
            .map_err(|error| {
                format!(
                    "could not read iOS XCFramework directory `{}`: {error}",
                    directory.display()
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("could not inspect iOS XCFramework entry: {error}"))?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries.into_iter().rev() {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "iOS XCFramework contains a non-UTF-8 path".to_owned())?;
            if !is_safe_component(&name) {
                return Err(format!(
                    "iOS XCFramework contains an unsafe path component: `{name}`"
                ));
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                format!(
                    "could not inspect iOS XCFramework entry `{}`: {error}",
                    path.display()
                )
            })?;
            if metadata.file_type().is_symlink() {
                return Err(format!(
                    "iOS XCFramework must not contain symbolic links: {}",
                    path.display()
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file() {
                files.push(path);
                if files.len() > MAX_XCFRAMEWORK_FILES {
                    return Err(format!(
                        "iOS XCFramework contains more than {MAX_XCFRAMEWORK_FILES} files"
                    ));
                }
            } else {
                return Err(format!(
                    "iOS XCFramework contains an unsupported filesystem entry: {}",
                    path.display()
                ));
            }
        }
    }
    files.sort_by(|left, right| {
        left.strip_prefix(root)
            .expect("collected file remains inside root")
            .cmp(
                right
                    .strip_prefix(root)
                    .expect("collected file remains inside root"),
            )
    });
    Ok(files)
}

fn hash_xcframework_files(root: &Path, files: &[PathBuf]) -> Result<XcframeworkIdentity, String> {
    let mut hasher = Sha256::new();
    hasher.update(b"godot-rust-ios-xcframework-v1\0");
    let mut byte_len = 0_u64;
    for path in files {
        let relative = path
            .strip_prefix(root)
            .expect("collected file remains inside root")
            .to_str()
            .ok_or_else(|| "iOS XCFramework contains a non-UTF-8 path".to_owned())?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let relative_len = u64::try_from(relative.len())
            .map_err(|_| "iOS XCFramework path length overflowed u64".to_owned())?;
        hasher.update(relative_len.to_le_bytes());
        hasher.update(relative.as_bytes());
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            format!(
                "could not inspect iOS XCFramework file `{}`: {error}",
                path.display()
            )
        })?;
        let file_len = metadata.len();
        byte_len = byte_len
            .checked_add(file_len)
            .ok_or_else(|| "iOS XCFramework size overflowed u64".to_owned())?;
        if byte_len > MAX_MODULE_BYTES {
            return Err(format!(
                "iOS XCFramework exceeds the {MAX_MODULE_BYTES} byte safety limit"
            ));
        }
        hasher.update(file_len.to_le_bytes());
        let mut reader = BufReader::new(File::open(path).map_err(|error| {
            format!(
                "could not open iOS XCFramework file `{}`: {error}",
                path.display()
            )
        })?);
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer).map_err(|error| {
                format!(
                    "could not hash iOS XCFramework file `{}`: {error}",
                    path.display()
                )
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    Ok(XcframeworkIdentity {
        sha256: format!("{:x}", hasher.finalize()),
        byte_len,
        file_count: files.len(),
    })
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<(), String> {
    let mut entries = std::fs::read_dir(source)
        .map_err(|error| {
            format!(
                "could not read source XCFramework directory `{}`: {error}",
                source.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("could not inspect source XCFramework entry: {error}"))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "iOS XCFramework contains a non-UTF-8 path".to_owned())?;
        if !is_safe_component(&name) {
            return Err(format!(
                "iOS XCFramework contains an unsafe path component: `{name}`"
            ));
        }
        let source_path = entry.path();
        let destination_path = destination.join(&name);
        let metadata = std::fs::symlink_metadata(&source_path).map_err(|error| {
            format!(
                "could not inspect source XCFramework entry `{}`: {error}",
                source_path.display()
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "iOS XCFramework must not contain symbolic links: {}",
                source_path.display()
            ));
        }
        if metadata.is_dir() {
            std::fs::create_dir(&destination_path).map_err(|error| {
                format!(
                    "could not create staged XCFramework directory `{}`: {error}",
                    destination_path.display()
                )
            })?;
            copy_directory_contents(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            copy_file(&source_path, &destination_path)?;
        } else {
            return Err(format!(
                "iOS XCFramework contains an unsupported filesystem entry: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), String> {
    let mut source = File::open(source).map_err(|error| {
        format!(
            "could not open source XCFramework file `{}`: {error}",
            source.display()
        )
    })?;
    let mut destination_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| {
            format!(
                "could not create staged XCFramework file `{}`: {error}",
                destination.display()
            )
        })?;
    io::copy(&mut source, &mut destination_file)
        .and_then(|_| destination_file.flush())
        .and_then(|()| destination_file.sync_all())
        .map_err(|error| {
            format!(
                "could not copy staged XCFramework file `{}`: {error}",
                destination.display()
            )
        })
}

fn is_safe_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.bytes().all(|byte| {
            byte == b'_' || byte == b'-' || byte == b'.' || byte.is_ascii_alphanumeric()
        })
}

fn run_bounded_command(command: &mut Command, purpose: &str) -> Result<(), String> {
    let output = crate::process::run_command(command, purpose)?;
    if output.stdout.len() > MAX_TOOL_OUTPUT_BYTES || output.stderr.len() > MAX_TOOL_OUTPUT_BYTES {
        return Err(format!(
            "{purpose} output exceeded the {MAX_TOOL_OUTPUT_BYTES} byte safety limit"
        ));
    }
    if !output.status.success() {
        return Err(format!(
            "{purpose} failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
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
            let path = std::env::temp_dir().join(format!(
                "godot-rust-xcframework-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir(&path).expect("temporary directory");
            Self(path)
        }

        fn xcframework(&self, name: &str) -> PathBuf {
            let root = self.0.join(name);
            let device = root.join("ios-arm64");
            let simulator = root.join("ios-arm64_x86_64-simulator");
            std::fs::create_dir_all(&device).expect("device directory");
            std::fs::create_dir_all(&simulator).expect("simulator directory");
            std::fs::write(
                root.join("Info.plist"),
                br#"<?xml version="1.0"?>
<plist><dict><key>AvailableLibraries</key><array>
<dict><key>SupportedArchitectures</key><array><string>arm64</string></array>
<key>SupportedPlatform</key><string>ios</string></dict>
<dict><key>SupportedArchitectures</key><array><string>arm64</string><string>x86_64</string></array>
<key>SupportedPlatform</key><string>ios</string>
<key>SupportedPlatformVariant</key><string>simulator</string></dict>
</array></dict></plist>
"#,
            )
            .expect("Info.plist");
            std::fs::write(device.join("libproject.dylib"), b"device").expect("device library");
            std::fs::write(simulator.join("libproject.dylib"), b"simulator")
                .expect("simulator library");
            root
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn copying_an_xcframework_preserves_its_content_identity() {
        let temporary = TempDirectory::new();
        let source = temporary.xcframework("source.xcframework");
        let destination = temporary.0.join("destination.xcframework");
        let expected = inspect_ios_xcframework(&source).expect("source identity");
        assert_eq!(
            copy_ios_xcframework(&source, &destination).expect("copy identity"),
            expected
        );
        assert_eq!(
            inspect_ios_xcframework(&destination).expect("destination identity"),
            expected
        );
    }

    #[cfg(unix)]
    #[test]
    fn xcframework_validation_rejects_symbolic_links() {
        use std::os::unix::fs::symlink;

        let temporary = TempDirectory::new();
        let source = temporary.xcframework("source.xcframework");
        symlink(
            source.join("Info.plist"),
            source.join("ios-arm64/linked.plist"),
        )
        .expect("symbolic link");
        assert!(
            inspect_ios_xcframework(&source)
                .expect_err("symbolic link must fail")
                .contains("symbolic links")
        );
    }
}
