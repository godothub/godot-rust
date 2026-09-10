#[cfg(unix)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

pub(crate) fn has_managed_header(contents: &[u8], header_line: &[u8]) -> bool {
    contents
        .strip_prefix(header_line)
        .is_some_and(|remainder| remainder.starts_with(b"\n") || remainder.starts_with(b"\r\n"))
}

pub(crate) fn ensure_directory(path: &Path, purpose: &str) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "{purpose} must not be a symbolic link: {}",
            path.display()
        )),
        Ok(metadata) if !metadata.is_dir() => {
            Err(format!("{purpose} is not a directory: {}", path.display()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => std::fs::create_dir(path)
            .map_err(|error| format!("could not create {purpose} `{}`: {error}", path.display())),
        Err(error) => Err(format!(
            "could not inspect {purpose} `{}`: {error}",
            path.display()
        )),
    }
}

pub(crate) fn create_temporary_directory(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    for _ in 0..128 {
        let id = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("{prefix}-{}-{id}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "could not create temporary directory `{}`: {error}",
                    path.display()
                ));
            }
        }
    }
    Err(format!(
        "could not allocate a unique temporary directory in `{}`",
        parent.display()
    ))
}

pub(crate) fn atomic_write(path: &Path, contents: &[u8], purpose: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{purpose} has no parent: {}", path.display()))?;
    let permissions = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "refusing to replace {purpose} that is not a regular file: {}",
                path.display()
            ));
        }
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(format!(
                "could not inspect {purpose} `{}`: {error}",
                path.display()
            ));
        }
    };
    let id = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(".godothub-write-{}-{id}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| {
            format!(
                "could not create temporary {purpose} `{}`: {error}",
                temporary.display()
            )
        })?;
    let written = (|| {
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(contents)?;
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "could not flush temporary {purpose} `{}`: {error}",
            temporary.display()
        ));
    }
    if let Err(error) = replace_file(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!(
            "could not atomically replace {purpose} `{}`: {error}",
            path.display()
        ));
    }
    sync_directory(parent)
}

#[cfg(not(windows))]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        // SAFETY: Both paths are terminated UTF-16 buffers retained for the call.
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        // Readers and antivirus scanners can briefly deny deletion or sharing.
        if !matches!(error.raw_os_error(), Some(5 | 32 | 33))
            || std::time::Instant::now() >= deadline
        {
            return Err(error);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "could not flush managed directory `{}`: {error}",
                path.display()
            )
        })
}

#[cfg(not(unix))]
pub(crate) fn sync_directory(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::has_managed_header;

    #[test]
    fn managed_headers_accept_platform_line_endings_and_reject_prefixes() {
        let header = b"// @generated";
        assert!(has_managed_header(b"// @generated\nbody\n", header));
        assert!(has_managed_header(b"// @generated\r\nbody\r\n", header));
        assert!(!has_managed_header(b"// @generated", header));
        assert!(!has_managed_header(
            b"// @generated by someone else\n",
            header
        ));
        assert!(!has_managed_header(b" // @generated\n", header));
    }

    #[cfg(windows)]
    #[test]
    fn atomic_replacement_waits_for_readers_and_preserves_locked_files() {
        use std::os::windows::fs::OpenOptionsExt;
        use std::time::{Duration, Instant};

        let directory =
            super::create_temporary_directory(&std::env::temp_dir(), "godot-rust-managed-sharing")
                .expect("temporary directory");
        let path = directory.join("Cargo.toml");
        std::fs::write(&path, b"original").expect("original manifest");
        let reader = super::OpenOptions::new()
            .read(true)
            .share_mode(3) // Allow reads and writes, but keep the file name locked.
            .open(&path)
            .expect("reader");
        let started = Instant::now();
        let blocked = super::atomic_write(&path, b"rejected", "manifest");
        assert!(blocked.is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);

        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(reader);
        });
        let saved = super::atomic_write(&path, b"updated", "manifest");
        release.join().expect("reader closed");
        saved.expect("save after reader closes");
        assert_eq!(std::fs::read(&path).unwrap(), b"updated");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_replacement_preserves_existing_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = super::create_temporary_directory(
            &std::env::temp_dir(),
            "godot-rust-managed-permissions",
        )
        .expect("temporary directory");
        let path = directory.join("private.rs");
        std::fs::write(&path, b"original").expect("private file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("private permissions");
        super::atomic_write(&path, b"updated", "private Rust source").expect("atomic replacement");
        let contents = std::fs::read(&path).expect("updated contents");
        let mode = std::fs::metadata(&path)
            .expect("updated metadata")
            .permissions()
            .mode();
        let _ = std::fs::remove_dir_all(directory);
        assert_eq!(contents, b"updated");
        assert_eq!(mode & 0o777, 0o600);
    }
}
