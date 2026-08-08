use crate::{ExportArchitecture, ExportOperatingSystem};
use object::{Architecture, BinaryFormat, Object, ObjectSymbol};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read};
use std::path::Path;
use std::process::Command;

pub(crate) const MAX_MODULE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_VALIDATOR_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn ensure_regular_module(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect `{}`: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "project module artifact is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_MODULE_BYTES {
        return Err(format!(
            "project module artifact exceeds the {} byte safety limit",
            MAX_MODULE_BYTES
        ));
    }
    Ok(())
}

pub(crate) fn copy_module(source: &Path, destination: &Path) -> Result<(), String> {
    ensure_regular_module(source)?;
    let mut source =
        File::open(source).map_err(|error| format!("could not open module artifact: {error}"))?;
    let mut destination = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|error| format!("could not create staged module artifact: {error}"))?;
    let byte_len = std::io::copy(&mut source, &mut destination)
        .map_err(|error| format!("could not copy module artifact: {error}"))?;
    if byte_len > MAX_MODULE_BYTES {
        return Err(format!(
            "project module artifact exceeds the {} byte safety limit",
            MAX_MODULE_BYTES
        ));
    }
    destination
        .sync_all()
        .map_err(|error| format!("could not flush staged module artifact: {error}"))
}

pub(crate) fn hash_module(path: &Path) -> Result<(String, u64), String> {
    ensure_regular_module(path)?;
    let file = File::open(path)
        .map_err(|error| format!("could not open staged module for hashing: {error}"))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut byte_len = 0_u64;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("could not hash staged module: {error}"))?;
        if read == 0 {
            break;
        }
        byte_len = byte_len
            .checked_add(read as u64)
            .ok_or_else(|| "project module artifact size overflowed u64".to_owned())?;
        if byte_len > MAX_MODULE_BYTES {
            return Err(format!(
                "project module artifact exceeds the {} byte safety limit",
                MAX_MODULE_BYTES
            ));
        }
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), byte_len))
}

pub(crate) fn validate_with_program(
    module: &Path,
    validator: &OsStr,
) -> Result<(String, String), String> {
    ensure_regular_module(module)?;
    let output = crate::process::run_command(
        Command::new(validator).arg(module),
        &format!("module validator `{}`", validator.to_string_lossy()),
    )?;
    if output.stdout.len() > MAX_VALIDATOR_OUTPUT_BYTES
        || output.stderr.len() > MAX_VALIDATOR_OUTPUT_BYTES
    {
        return Err(format!(
            "module validator output exceeded the {} byte safety limit",
            MAX_VALIDATOR_OUTPUT_BYTES
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(format!(
            "module validator rejected candidate with status {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok((stdout, stderr))
}

pub(crate) fn validate_cross_target_module(
    module: &Path,
    operating_system: ExportOperatingSystem,
    architecture: ExportArchitecture,
) -> Result<(String, String), String> {
    validate_cross_target_module_entry(
        module,
        operating_system,
        architecture,
        "godot_rs_module_entry",
    )
}

pub(crate) fn validate_cross_target_module_entry(
    module: &Path,
    operating_system: ExportOperatingSystem,
    architecture: ExportArchitecture,
    expected_entry: &str,
) -> Result<(String, String), String> {
    ensure_regular_module(module)?;
    if expected_entry.is_empty()
        || !expected_entry
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        return Err("required module entry is not a C identifier".to_owned());
    }
    if architecture == ExportArchitecture::Universal {
        return validate_macos_universal_entry(module, operating_system, expected_entry);
    }
    let bytes = std::fs::read(module)
        .map_err(|error| format!("could not read cross-target module: {error}"))?;
    let file = object::File::parse(bytes.as_slice())
        .map_err(|error| format!("cross-target module is not a valid object file: {error}"))?;
    let expected_format = expected_binary_format(operating_system);
    if file.format() != expected_format {
        return Err(format!(
            "cross-target module format mismatch: expected {expected_format:?}, found {:?}",
            file.format()
        ));
    }
    let expected_architecture = match architecture {
        ExportArchitecture::Arm32 => Architecture::Arm,
        ExportArchitecture::Arm64 => Architecture::Aarch64,
        ExportArchitecture::Wasm32 => Architecture::Wasm32,
        ExportArchitecture::X86_32 => Architecture::I386,
        ExportArchitecture::X86_64 => Architecture::X86_64,
        ExportArchitecture::Universal => unreachable!("handled before object parsing"),
    };
    if file.architecture() != expected_architecture {
        return Err(format!(
            "cross-target module architecture mismatch: expected {expected_architecture:?}, \
             found {:?}",
            file.architecture()
        ));
    }
    let has_entry = if file.format() == BinaryFormat::Pe {
        file.exports()
            .map_err(|error| {
                format!("could not inspect cross-target PE export directory: {error}")
            })?
            .iter()
            .any(|export| exported_name_matches(export.name(), expected_entry))
    } else {
        file.dynamic_symbols()
            .chain(file.symbols())
            .filter_map(|symbol| symbol.name().ok())
            .any(|name| name.trim_start_matches('_') == expected_entry)
    };
    if !has_entry {
        return Err(format!(
            "cross-target module does not export the required `{expected_entry}` symbol"
        ));
    }
    if operating_system == ExportOperatingSystem::Web && file.section_by_name("dylink.0").is_none()
    {
        return Err(
            "Web module is not an Emscripten side module: the `dylink.0` section is missing"
                .to_owned(),
        );
    }

    Ok((
        format!(
            "validated {:?} {:?} dynamic library and required entry symbol\n",
            file.format(),
            file.architecture()
        ),
        String::new(),
    ))
}

fn exported_name_matches(name: &[u8], expected_entry: &str) -> bool {
    name == expected_entry.as_bytes() || name.strip_prefix(b"_") == Some(expected_entry.as_bytes())
}

const fn expected_binary_format(operating_system: ExportOperatingSystem) -> BinaryFormat {
    match operating_system {
        ExportOperatingSystem::Android | ExportOperatingSystem::Linux => BinaryFormat::Elf,
        ExportOperatingSystem::Ios | ExportOperatingSystem::Macos => BinaryFormat::MachO,
        ExportOperatingSystem::Web => BinaryFormat::Wasm,
        // A linked Windows DLL uses the PE image format. COFF is the format
        // of an unlinked object file and must not be accepted as an exported
        // project module.
        ExportOperatingSystem::Windows => BinaryFormat::Pe,
    }
}

fn validate_macos_universal_entry(
    module: &Path,
    operating_system: ExportOperatingSystem,
    expected_entry: &str,
) -> Result<(String, String), String> {
    if operating_system != ExportOperatingSystem::Macos {
        return Err("Universal modules are supported only for macOS export".to_owned());
    }
    let lipo = crate::process::run_command(
        Command::new("lipo")
            .arg(module)
            .args(["-verify_arch", "x86_64", "arm64"]),
        "`lipo` Universal architecture validation",
    )?;
    if lipo.stdout.len() > MAX_VALIDATOR_OUTPUT_BYTES
        || lipo.stderr.len() > MAX_VALIDATOR_OUTPUT_BYTES
    {
        return Err(format!(
            "`lipo` output exceeded the {} byte safety limit",
            MAX_VALIDATOR_OUTPUT_BYTES
        ));
    }
    if !lipo.status.success() {
        return Err(format!(
            "macOS Universal module does not contain x86_64 and arm64 slices: {}",
            String::from_utf8_lossy(&lipo.stderr).trim()
        ));
    }
    let nm = crate::process::run_command(
        Command::new("nm").args(["-gU"]).arg(module),
        "`nm` Universal entry validation",
    )?;
    if nm.stdout.len() > MAX_VALIDATOR_OUTPUT_BYTES || nm.stderr.len() > MAX_VALIDATOR_OUTPUT_BYTES
    {
        return Err(format!(
            "`nm` output exceeded the {} byte safety limit",
            MAX_VALIDATOR_OUTPUT_BYTES
        ));
    }
    if !nm.status.success() {
        return Err(format!(
            "`nm` could not inspect the macOS Universal module: {}",
            String::from_utf8_lossy(&nm.stderr).trim()
        ));
    }
    let symbol = format!("_{expected_entry}");
    let output = String::from_utf8_lossy(&nm.stdout);
    if !output
        .lines()
        .any(|line| line.split_whitespace().last() == Some(symbol.as_str()))
    {
        return Err(format!(
            "macOS Universal module does not export the required `{expected_entry}` symbol"
        ));
    }
    Ok((
        format!(
            "validated macOS Universal x86_64/arm64 module and `{expected_entry}` entry symbol\n"
        ),
        String::from_utf8_lossy(&nm.stderr).into_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temporary_directory() -> std::path::PathBuf {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "godot-rust-module-artifact-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temporary directory");
        directory
    }

    #[test]
    fn copied_modules_retain_content_identity() {
        let directory = temporary_directory();
        let source = directory.join("source.so");
        let destination = directory.join("destination.so");
        std::fs::write(&source, b"validated project module").expect("source module");
        copy_module(&source, &destination).expect("copy module");
        assert_eq!(
            hash_module(&source).expect("source hash"),
            hash_module(&destination).expect("destination hash")
        );
        std::fs::remove_dir_all(directory).expect("remove temporary directory");
    }

    #[test]
    fn cross_target_validation_rejects_non_object_data() {
        let directory = temporary_directory();
        let module = directory.join("invalid.wasm");
        std::fs::write(&module, b"not a WebAssembly module").expect("invalid module");
        assert!(
            validate_cross_target_module(
                &module,
                ExportOperatingSystem::Web,
                ExportArchitecture::Wasm32,
            )
            .expect_err("invalid object")
            .contains("not a valid object file")
        );
        std::fs::remove_dir_all(directory).expect("remove temporary directory");
    }

    #[test]
    fn final_dynamic_library_formats_match_each_export_platform() {
        for operating_system in [ExportOperatingSystem::Android, ExportOperatingSystem::Linux] {
            assert_eq!(expected_binary_format(operating_system), BinaryFormat::Elf);
        }
        for operating_system in [ExportOperatingSystem::Ios, ExportOperatingSystem::Macos] {
            assert_eq!(
                expected_binary_format(operating_system),
                BinaryFormat::MachO
            );
        }
        assert_eq!(
            expected_binary_format(ExportOperatingSystem::Web),
            BinaryFormat::Wasm
        );
        assert_eq!(
            expected_binary_format(ExportOperatingSystem::Windows),
            BinaryFormat::Pe
        );
        assert_ne!(
            expected_binary_format(ExportOperatingSystem::Windows),
            BinaryFormat::Coff
        );
    }

    #[test]
    fn windows_export_names_accept_only_exact_c_entry_spelling() {
        assert!(exported_name_matches(
            b"godot_rs_native_init",
            "godot_rs_native_init"
        ));
        assert!(exported_name_matches(
            b"_godot_rs_native_init",
            "godot_rs_native_init"
        ));
        assert!(!exported_name_matches(
            b"godot_rs_native_init_internal",
            "godot_rs_native_init"
        ));
        assert!(!exported_name_matches(
            b"prefix_godot_rs_native_init",
            "godot_rs_native_init"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_modules_are_rejected() {
        use std::os::unix::fs::symlink;

        let directory = temporary_directory();
        let source = directory.join("source.so");
        let link = directory.join("link.so");
        std::fs::write(&source, b"project module").expect("source module");
        symlink(&source, &link).expect("module symbolic link");
        assert!(
            ensure_regular_module(&link)
                .expect_err("symbolic link")
                .contains("not a regular file")
        );
        std::fs::remove_dir_all(directory).expect("remove temporary directory");
    }
}
