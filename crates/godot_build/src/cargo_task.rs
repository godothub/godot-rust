use crate::diagnostic::{CargoDiagnostic, parse_compiler_message};
use crate::metadata::{CargoPackageModel, GodotRustMode};
use crate::project_setup::sync_cargo_api_config;
use crate::toolchain::WEB_RUST_TOOLCHAIN;
use godot_api::{GODOT_API_ENV, GodotApiVersion};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
const WEB_RUSTFLAGS: &[&str] = &[
    "-C link-arg=--no-entry",
    "-C link-arg=-sSIDE_MODULE=2",
    "-C link-arg=-sWASM_BIGINT",
    "-C link-arg=-sERROR_ON_UNDEFINED_SYMBOLS=0",
    "-C link-arg=-sSUPPORT_LONGJMP=0",
    "-C link-arg=-sDISABLE_EXCEPTION_CATCHING=1",
    // Rust already optimizes and applies LTO before the final link. Keeping
    // Emscripten's link mode at O0 prevents its version-pinned Binaryen from
    // re-optimizing newer Rust WebAssembly features that it cannot parse.
    "-C link-arg=-O0",
    // Emscripten 4 validates every side-module export as a JavaScript
    // identifier. Rust's v0 mangling is reversible and uses only
    // identifier-safe ASCII characters.
    "-C symbol-mangling-version=v0",
    "-C panic=abort",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CargoTaskKind {
    Check,
    Build,
    Test,
    Clippy,
    Format,
}

#[derive(Clone, Debug, Serialize)]
pub struct CargoTaskReport {
    pub kind: CargoTaskKind,
    pub root: PathBuf,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub command: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub diagnostics: Vec<CargoDiagnostic>,
    pub artifacts: Vec<PathBuf>,
    pub messages: Vec<String>,
    pub stderr: String,
}

pub fn run_cargo_task(
    root: impl AsRef<Path>,
    kind: CargoTaskKind,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<CargoTaskReport, String> {
    run_cargo_task_inner(root.as_ref(), kind, cargo, None, &BTreeMap::new(), None)
}

pub fn run_package_cargo_task(
    root: impl AsRef<Path>,
    kind: CargoTaskKind,
    package: &CargoPackageModel,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<CargoTaskReport, String> {
    let environment = script_api_environment(package)?;
    let godot_api = package
        .godot_api
        .unwrap_or_else(|| GodotApiVersion::new(4, 4));
    sync_cargo_api_config(root.as_ref(), godot_api)?;
    run_filtered_cargo_task(root.as_ref(), kind, package, cargo, &environment, None)
}

pub fn run_package_cargo_build(
    root: impl AsRef<Path>,
    package: &CargoPackageModel,
    target: &str,
    release: bool,
    runtime_godot: GodotApiVersion,
    android_sdk: Option<&Path>,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<CargoTaskReport, String> {
    if target.is_empty()
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("Cargo export target is not a canonical Rust target triple".to_owned());
    }
    let environment = script_api_environment(package)?;
    let godot_api = package
        .godot_api
        .unwrap_or_else(|| GodotApiVersion::new(4, 4));
    ensure_export_rust_target(root.as_ref(), target)?;
    let environment = export_build_environment(target, runtime_godot, android_sdk, environment)?;
    sync_cargo_api_config(root.as_ref(), godot_api)?;
    run_filtered_cargo_task(
        root.as_ref(),
        CargoTaskKind::Build,
        package,
        cargo,
        &environment,
        Some(BuildOptions { target, release }),
    )
}

pub fn run_native_package_cargo_build(
    root: impl AsRef<Path>,
    package: &CargoPackageModel,
    target: &str,
    release: bool,
    runtime_godot: GodotApiVersion,
    android_sdk: Option<&Path>,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<CargoTaskReport, String> {
    if package.godot_rust_mode != Some(GodotRustMode::Extension) {
        return Err(format!(
            "Cargo package `{}` is not configured for Extension Mode",
            package.name
        ));
    }
    if target.is_empty()
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("Cargo export target is not a canonical Rust target triple".to_owned());
    }
    let godot_api = package.godot_api.ok_or_else(|| {
        format!(
            "Native package `{}` has no validated Godot API target",
            package.name
        )
    })?;
    ensure_export_rust_target(root.as_ref(), target)?;
    let environment = export_build_environment(
        target,
        runtime_godot,
        android_sdk,
        BTreeMap::from([(GODOT_API_ENV.to_owned(), godot_api.to_string())]),
    )?;
    sync_cargo_api_config(root.as_ref(), godot_api)?;
    run_filtered_cargo_task(
        root.as_ref(),
        CargoTaskKind::Build,
        package,
        cargo,
        &environment,
        Some(BuildOptions { target, release }),
    )
}

pub fn run_native_package_cargo_task(
    root: impl AsRef<Path>,
    kind: CargoTaskKind,
    package: &CargoPackageModel,
    godot_api: GodotApiVersion,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<CargoTaskReport, String> {
    let environment = BTreeMap::from([(GODOT_API_ENV.to_owned(), godot_api.to_string())]);
    sync_cargo_api_config(root.as_ref(), godot_api)?;
    run_filtered_cargo_task(root.as_ref(), kind, package, cargo, &environment, None)
}

fn script_api_environment(package: &CargoPackageModel) -> Result<BTreeMap<String, String>, String> {
    if package.godot_rust_mode == Some(GodotRustMode::Extension) {
        return Err(format!(
            "Cargo package `{}` is configured for Extension Mode",
            package.name
        ));
    }
    let godot_api = package
        .godot_api
        .unwrap_or_else(|| GodotApiVersion::new(4, 4));
    Ok(BTreeMap::from([(
        GODOT_API_ENV.to_owned(),
        godot_api.to_string(),
    )]))
}

fn export_build_environment(
    target: &str,
    godot_api: GodotApiVersion,
    android_sdk: Option<&Path>,
    mut environment: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, String> {
    if target.ends_with("-linux-android") || target.ends_with("-linux-androideabi") {
        let linker = android_linker(target, android_sdk)?;
        environment.insert(cargo_target_variable(target, "LINKER"), linker);
    }
    if let Some(linker) = linux_cross_linker(target)? {
        environment.insert(cargo_target_variable(target, "LINKER"), linker);
    }
    if target == "aarch64-apple-ios" {
        ensure_xcode_ios_sdk("iphoneos", "device")?;
    } else if matches!(target, "aarch64-apple-ios-sim" | "x86_64-apple-ios") {
        ensure_xcode_ios_sdk("iphonesimulator", "Simulator")?;
    }
    if target == "wasm32-unknown-emscripten" {
        let emcc = emscripten_linker()?;
        validate_emscripten_version(&emcc, godot_api)?;
        environment.insert(cargo_target_variable(target, "LINKER"), emcc);
        environment.insert(
            cargo_target_variable(target, "RUSTFLAGS"),
            WEB_RUSTFLAGS.join(" "),
        );
        environment.insert("EMCC_SKIP_SANITY_CHECK".to_owned(), "1".to_owned());
        environment.insert("RUSTUP_TOOLCHAIN".to_owned(), WEB_RUST_TOOLCHAIN.to_owned());
    }
    Ok(environment)
}

fn cargo_target_variable(target: &str, suffix: &str) -> String {
    format!(
        "CARGO_TARGET_{}_{}",
        target.replace('-', "_").to_ascii_uppercase(),
        suffix
    )
}

fn ensure_export_rust_target(root: &Path, target: &str) -> Result<(), String> {
    if target == "wasm32-unknown-emscripten" {
        return ensure_web_rust_toolchain(root);
    }
    ensure_rust_target_installed(root, target)
}

fn ensure_web_rust_toolchain(root: &Path) -> Result<(), String> {
    let output = crate::process::run_command(
        Command::new("rustup")
            .args([
                "component",
                "list",
                "--toolchain",
                WEB_RUST_TOOLCHAIN,
                "--installed",
            ])
            .current_dir(root),
        &format!("Rust Web toolchain inspection for `{WEB_RUST_TOOLCHAIN}`"),
    )
    .map_err(|error| {
        format!("{error}; use Rust toolchain repair to install the pinned Web toolchain")
    })?;
    let installed = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() || !installed.lines().any(|line| line.starts_with("rust-src")) {
        return Err(format!(
            "Godot Web export requires `{WEB_RUST_TOOLCHAIN}` with `rust-src`; \
             use Rust toolchain repair to install it"
        ));
    }
    Ok(())
}

fn ensure_rust_target_installed(root: &Path, target: &str) -> Result<(), String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let output = crate::process::run_command(
        Command::new(&rustc)
            .args(["--print", "target-libdir", "--target", target])
            .current_dir(root),
        &format!("Rust target inspection for `{target}`"),
    )
    .map_err(|error| {
        if error.contains("cancelled") {
            error
        } else {
            missing_rust_target(target, &error)
        }
    })?;
    if !output.status.success() {
        return Err(missing_rust_target(
            target,
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    let target_libdir = String::from_utf8(output.stdout)
        .map_err(|_| missing_rust_target(target, "rustc returned a non-UTF-8 target directory"))?;
    let target_libdir = PathBuf::from(target_libdir.trim());
    let has_core = std::fs::read_dir(&target_libdir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("libcore-") && name.ends_with(".rlib"))
        });
    if !has_core {
        return Err(missing_rust_target(
            target,
            &format!(
                "Rust target library directory is incomplete: {}",
                target_libdir.display()
            ),
        ));
    }
    Ok(())
}

fn missing_rust_target(target: &str, detail: &str) -> String {
    format!(
        "Rust target `{target}` is not installed or is incomplete: {detail}. Run `rustup target \
         add {target}` for the active project toolchain, then export again"
    )
}

fn android_linker(target: &str, android_sdk: Option<&Path>) -> Result<String, String> {
    let variable = cargo_target_variable(target, "LINKER");
    if let Some(linker) = std::env::var_os(&variable) {
        let linker = PathBuf::from(linker);
        if linker.is_file() {
            return Ok(linker.to_string_lossy().into_owned());
        }
        return Err(format!(
            "`{variable}` does not point to an Android NDK linker: {}",
            linker.display()
        ));
    }
    let ndk_root = discover_android_ndk(android_sdk).ok_or_else(|| {
        "Android export requires the Android NDK. Install it from Android Studio or Godot's \
         Android export setup, then set `ANDROID_NDK_ROOT` (or `ANDROID_HOME`/`ANDROID_SDK_ROOT`) \
         and export again"
            .to_owned()
    })?;
    let prebuilt_root = ndk_root.join("toolchains/llvm/prebuilt");
    let mut prebuilts = std::fs::read_dir(&prebuilt_root)
        .map_err(|error| {
            format!(
                "could not inspect Android NDK toolchains `{}`: {error}",
                prebuilt_root.display()
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    prebuilts.sort();
    let prebuilt = match prebuilts.as_slice() {
        [prebuilt] => prebuilt,
        [] => {
            return Err(format!(
                "Android NDK has no LLVM prebuilt toolchain: {}",
                prebuilt_root.display()
            ));
        }
        _ => {
            return Err(format!(
                "Android NDK contains multiple LLVM prebuilt toolchains; set `{variable}` \
                 explicitly"
            ));
        }
    };
    let clang_target = match target {
        "armv7-linux-androideabi" => "armv7a-linux-androideabi",
        "aarch64-linux-android" => "aarch64-linux-android",
        "i686-linux-android" => "i686-linux-android",
        "x86_64-linux-android" => "x86_64-linux-android",
        _ => return Err(format!("unsupported Android Rust target `{target}`")),
    };
    let executable_suffix = if cfg!(windows) { ".cmd" } else { "" };
    let linker = prebuilt
        .join("bin")
        .join(format!("{clang_target}21-clang{executable_suffix}"));
    if !linker.is_file() {
        return Err(format!(
            "Android NDK linker is missing for `{target}`: {}",
            linker.display()
        ));
    }
    Ok(linker.to_string_lossy().into_owned())
}

fn discover_android_ndk(configured_sdk: Option<&Path>) -> Option<PathBuf> {
    for variable in ["ANDROID_NDK_ROOT", "ANDROID_NDK_HOME"] {
        if let Some(path) = std::env::var_os(variable).map(PathBuf::from) {
            if path.is_dir() {
                return Some(path);
            }
        }
    }
    if let Some(ndk) = configured_sdk.and_then(ndk_from_sdk) {
        return Some(ndk);
    }
    for variable in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        let Some(sdk) = std::env::var_os(variable).map(PathBuf::from) else {
            continue;
        };
        if let Some(ndk) = ndk_from_sdk(&sdk) {
            return Some(ndk);
        }
    }
    None
}

fn ndk_from_sdk(sdk: &Path) -> Option<PathBuf> {
    let bundled = sdk.join("ndk-bundle");
    if bundled.is_dir() {
        return Some(bundled);
    }
    let mut versions = std::fs::read_dir(sdk.join("ndk"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    versions.sort_by_key(|path| {
        path.file_name()
            .and_then(OsStr::to_str)
            .map(ndk_version_key)
            .unwrap_or_default()
    });
    versions.pop()
}

fn ndk_version_key(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|component| component.parse().unwrap_or(0))
        .collect()
}

fn linux_cross_linker(target: &str) -> Result<Option<String>, String> {
    let Some(program) = (match target {
        "aarch64-unknown-linux-gnu" => Some("aarch64-linux-gnu-gcc"),
        "armv7-unknown-linux-gnueabihf" => Some("arm-linux-gnueabihf-gcc"),
        _ => None,
    }) else {
        return Ok(None);
    };
    let variable = cargo_target_variable(target, "LINKER");
    if let Some(linker) = std::env::var_os(&variable) {
        return Ok(Some(linker.to_string_lossy().into_owned()));
    }
    if executable_on_path(program) {
        return Ok(Some(program.to_owned()));
    }
    Err(format!(
        "Linux target `{target}` requires the `{program}` cross linker. Install a cross C \
         toolchain that provides it, or set `{variable}` to its absolute path"
    ))
}

fn executable_on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| {
            let candidate = directory.join(program);
            candidate.is_file()
                || (cfg!(windows) && directory.join(format!("{program}.exe")).is_file())
        })
    })
}

fn ensure_xcode_ios_sdk(sdk: &str, display_name: &str) -> Result<(), String> {
    let output = crate::process::run_command(
        Command::new("xcrun").args(["--sdk", sdk, "--show-sdk-path"]),
        &format!("Xcode iOS {display_name} SDK inspection"),
    )
    .map_err(|error| {
        format!(
            "{error}; install Xcode with iOS platform support and select its Command Line Tools"
        )
    })?;
    if output.status.success() && !output.stdout.is_empty() {
        return Ok(());
    }
    Err(format!(
        "Xcode could not locate the iOS {display_name} SDK: {}. Install iOS platform support in \
         Xcode and select its Command Line Tools",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn emscripten_linker() -> Result<String, String> {
    let variable = cargo_target_variable("wasm32-unknown-emscripten", "LINKER");
    if let Some(linker) = std::env::var_os(&variable) {
        return Ok(linker.to_string_lossy().into_owned());
    }
    if let Some(emsdk) = std::env::var_os("EMSDK") {
        let emcc = PathBuf::from(emsdk)
            .join("upstream/emscripten")
            .join(if cfg!(windows) { "emcc.bat" } else { "emcc" });
        if emcc.is_file() {
            return Ok(emcc.to_string_lossy().into_owned());
        }
    }
    Ok(if cfg!(windows) {
        "emcc.bat".to_owned()
    } else {
        "emcc".to_owned()
    })
}

fn validate_emscripten_version(emcc: &str, godot_api: GodotApiVersion) -> Result<(), String> {
    let expected = emscripten_version_for_godot(godot_api)?;
    let output = crate::process::run_command(
        Command::new(emcc).arg("--version"),
        &format!("Emscripten linker `{emcc}`"),
    )
    .map_err(|error| format!("{error}; activate emsdk {expected}"))?;
    if !output.status.success() {
        return Err(format!(
            "Emscripten linker `{emcc}` failed while checking its version"
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout
        .lines()
        .next()
        .is_some_and(|line| line.contains(expected))
    {
        return Err(format!(
            "Godot {godot_api} Web templates require Emscripten {expected}, but `{emcc} \
             --version` reported `{}`",
            stdout.lines().next().unwrap_or("<empty output>").trim()
        ));
    }
    Ok(())
}

fn emscripten_version_for_godot(godot_api: GodotApiVersion) -> Result<&'static str, String> {
    Ok(match (godot_api.major(), godot_api.minor()) {
        (4, 4) => "3.1.64",
        (4, 5) => "4.0.10",
        (4, 6 | 7) => "4.0.20",
        _ => {
            return Err(format!(
                "Web export has no authenticated Emscripten toolchain mapping for Godot {godot_api}"
            ));
        }
    })
}

fn run_filtered_cargo_task(
    root: &Path,
    kind: CargoTaskKind,
    package: &CargoPackageModel,
    cargo: Option<impl AsRef<OsStr>>,
    environment: &BTreeMap<String, String>,
    build_options: Option<BuildOptions<'_>>,
) -> Result<CargoTaskReport, String> {
    let [target] = package.cdylib_targets.as_slice() else {
        return Err(format!(
            "Cargo package `{}` must have exactly one cdylib target",
            package.name
        ));
    };
    run_cargo_task_inner(
        root,
        kind,
        cargo,
        Some(PackageFilter {
            id: &package.id,
            name: &package.name,
            target_name: &target.name,
        }),
        environment,
        build_options,
    )
}

#[derive(Clone, Copy)]
struct PackageFilter<'a> {
    id: &'a str,
    name: &'a str,
    target_name: &'a str,
}

#[derive(Clone, Copy)]
struct BuildOptions<'a> {
    target: &'a str,
    release: bool,
}

fn run_cargo_task_inner(
    root: &Path,
    kind: CargoTaskKind,
    cargo: Option<impl AsRef<OsStr>>,
    package: Option<PackageFilter<'_>>,
    environment: &BTreeMap<String, String>,
    build_options: Option<BuildOptions<'_>>,
) -> Result<CargoTaskReport, String> {
    let root = root.canonicalize().map_err(|error| {
        format!(
            "could not resolve Cargo project root `{}`: {error}",
            root.display()
        )
    })?;
    if !root.join("Cargo.toml").is_file() {
        return Err(format!(
            "Cargo project has no Cargo.toml: {}",
            root.display()
        ));
    }
    let cargo = cargo
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| OsString::from("cargo"));
    let arguments = task_arguments(kind, package.map(|package| package.name), build_options);
    let output = crate::process::run_command(
        Command::new(&cargo)
            .args(&arguments)
            .envs(environment)
            .current_dir(&root),
        &format!("`{}`", cargo.to_string_lossy()),
    )?;
    if output.stdout.len() > MAX_CAPTURE_BYTES || output.stderr.len() > MAX_CAPTURE_BYTES {
        return Err(format!(
            "Cargo output exceeded the {} byte safety limit",
            MAX_CAPTURE_BYTES
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "Cargo returned non-UTF-8 stdout".to_owned())?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let mut diagnostics = Vec::new();
    let mut artifacts = Vec::new();
    let mut messages = Vec::new();
    for line in stdout.lines().filter(|line| !line.is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            messages.push(line.to_owned());
            continue;
        };
        if let Some(diagnostic) = parse_compiler_message(&value, &root) {
            diagnostics.push(diagnostic);
            continue;
        }
        if value.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
            && artifact_is_cdylib(&value)
            && package.is_none_or(|package| artifact_matches_package(&value, package))
        {
            artifacts.extend(
                value
                    .get("filenames")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(PathBuf::from)
                    .filter(|path| is_dynamic_library(path)),
            );
        }
    }

    Ok(CargoTaskReport {
        kind,
        root,
        success: output.status.success(),
        exit_code: output.status.code(),
        command: std::iter::once(cargo.to_string_lossy().into_owned())
            .chain(
                arguments
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned()),
            )
            .collect(),
        environment: environment.clone(),
        diagnostics,
        artifacts,
        messages,
        stderr,
    })
}

fn task_arguments(
    kind: CargoTaskKind,
    package: Option<&str>,
    build_options: Option<BuildOptions<'_>>,
) -> Vec<OsString> {
    let mut arguments = match kind {
        CargoTaskKind::Check => vec![OsString::from("check")],
        CargoTaskKind::Build => vec![OsString::from("build")],
        CargoTaskKind::Test => vec![OsString::from("test"), OsString::from("--no-run")],
        CargoTaskKind::Clippy => vec![OsString::from("clippy")],
        CargoTaskKind::Format => {
            let mut arguments = vec![OsString::from("fmt")];
            if let Some(package) = package {
                arguments.extend([OsString::from("--package"), OsString::from(package)]);
            }
            arguments.extend([OsString::from("--"), OsString::from("--check")]);
            return arguments;
        }
    };
    if let Some(package) = package {
        arguments.extend([OsString::from("--package"), OsString::from(package)]);
    }
    if let Some(options) = build_options {
        debug_assert_eq!(kind, CargoTaskKind::Build);
        arguments.extend([OsString::from("--target"), OsString::from(options.target)]);
        if options.target == "wasm32-unknown-emscripten" {
            arguments.extend([
                OsString::from("-Z"),
                OsString::from("build-std=std,panic_abort"),
            ]);
        }
        if options.release {
            arguments.push(OsString::from("--release"));
        }
    }
    arguments.push(OsString::from(
        "--message-format=json-diagnostic-rendered-ansi",
    ));
    arguments
}

fn artifact_matches_package(value: &serde_json::Value, package: PackageFilter<'_>) -> bool {
    value.get("package_id").and_then(serde_json::Value::as_str) == Some(package.id)
        && value
            .pointer("/target/name")
            .and_then(serde_json::Value::as_str)
            == Some(package.target_name)
}

fn artifact_is_cdylib(value: &serde_json::Value) -> bool {
    value
        .pointer("/target/crate_types")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|types| types.iter().any(|type_| type_.as_str() == Some("cdylib")))
}

fn is_dynamic_library(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| matches!(extension, "dll" | "dylib" | "so" | "wasm"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_tasks_have_editor_safe_argument_shapes() {
        assert_eq!(
            task_arguments(CargoTaskKind::Check, None, None),
            ["check", "--message-format=json-diagnostic-rendered-ansi"].map(OsString::from)
        );
        assert_eq!(
            task_arguments(CargoTaskKind::Build, None, None),
            ["build", "--message-format=json-diagnostic-rendered-ansi"].map(OsString::from)
        );
        assert_eq!(
            task_arguments(CargoTaskKind::Format, None, None),
            ["fmt", "--", "--check"].map(OsString::from)
        );
        assert_eq!(
            task_arguments(CargoTaskKind::Build, Some("godothub_project"), None),
            [
                "build",
                "--package",
                "godothub_project",
                "--message-format=json-diagnostic-rendered-ansi",
            ]
            .map(OsString::from)
        );
        assert_eq!(
            task_arguments(
                CargoTaskKind::Build,
                Some("godothub_project"),
                Some(BuildOptions {
                    target: "aarch64-apple-darwin",
                    release: true,
                }),
            ),
            [
                "build",
                "--package",
                "godothub_project",
                "--target",
                "aarch64-apple-darwin",
                "--release",
                "--message-format=json-diagnostic-rendered-ansi",
            ]
            .map(OsString::from)
        );
        assert_eq!(
            task_arguments(
                CargoTaskKind::Build,
                Some("godothub_project"),
                Some(BuildOptions {
                    target: "wasm32-unknown-emscripten",
                    release: true,
                }),
            ),
            [
                "build",
                "--package",
                "godothub_project",
                "--target",
                "wasm32-unknown-emscripten",
                "-Z",
                "build-std=std,panic_abort",
                "--release",
                "--message-format=json-diagnostic-rendered-ansi",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn export_target_triples_reject_shell_and_argument_injection() {
        let package = CargoPackageModel {
            id: "game 0.1.0".to_owned(),
            name: "game".to_owned(),
            manifest_path: PathBuf::from("/game/Cargo.toml"),
            workspace_default: true,
            godot_rs_dependency: true,
            godot_rust_enabled: true,
            godot_rust_mode: None,
            godot_api: None,
            scripts_path: None,
            editor: crate::EditorWorkflowConfig::default(),
            script_mode_configured: true,
            configuration_issues: Vec::new(),
            cdylib_targets: vec![crate::CargoTargetModel {
                name: "game".to_owned(),
                src_path: PathBuf::from("/game/src/lib.rs"),
            }],
        };
        assert!(
            run_package_cargo_build(
                "/missing",
                &package,
                "--target malicious",
                false,
                GodotApiVersion::new(4, 4),
                None,
                None::<&OsStr>,
            )
            .expect_err("invalid target")
            .contains("canonical Rust target triple")
        );
    }

    #[test]
    fn web_toolchain_tracks_the_exporting_godot_runtime() {
        assert!(WEB_RUSTFLAGS.contains(&"-C symbol-mangling-version=v0"));
        assert_eq!(
            emscripten_version_for_godot(GodotApiVersion::new(4, 4)).expect("Godot 4.4"),
            "3.1.64"
        );
        assert_eq!(
            emscripten_version_for_godot(GodotApiVersion::new(4, 7)).expect("Godot 4.7"),
            "4.0.20"
        );
    }

    #[test]
    fn android_ndk_versions_sort_numerically() {
        assert!(ndk_version_key("27.2.12479018") > ndk_version_key("9.0.0"));
        assert!(ndk_version_key("26.3.11579264") > ndk_version_key("26.1.10909125"));
    }

    #[test]
    fn only_cdylib_artifacts_are_candidates_for_module_publication() {
        assert!(artifact_is_cdylib(&serde_json::json!({
            "target": { "crate_types": ["cdylib", "rlib"] }
        })));
        assert!(!artifact_is_cdylib(&serde_json::json!({
            "target": { "crate_types": ["bin"] }
        })));
    }

    #[test]
    fn native_tasks_use_the_internal_api_selection_environment() {
        let environment = BTreeMap::from([(
            GODOT_API_ENV.to_owned(),
            GodotApiVersion::new(4, 6).to_string(),
        )]);
        assert_eq!(
            environment.get(GODOT_API_ENV).map(String::as_str),
            Some("4.6")
        );
    }

    #[test]
    fn script_tasks_select_the_configured_api_target() {
        let package = package(
            Some(GodotRustMode::Script),
            Some(GodotApiVersion::new(4, 7)),
        );
        let environment = script_api_environment(&package).expect("Script API environment");
        assert_eq!(
            environment.get(GODOT_API_ENV).map(String::as_str),
            Some("4.7")
        );
    }

    #[test]
    fn legacy_script_tasks_default_to_the_4_4_api_target() {
        let package = package(None, None);
        let environment = script_api_environment(&package).expect("legacy Script API environment");
        assert_eq!(
            environment.get(GODOT_API_ENV).map(String::as_str),
            Some("4.4")
        );
    }

    #[test]
    fn native_packages_cannot_enter_the_script_task_path() {
        let package = package(
            Some(GodotRustMode::Extension),
            Some(GodotApiVersion::new(4, 6)),
        );
        assert!(
            script_api_environment(&package)
                .expect_err("Native package must use Native task path")
                .contains("Extension Mode")
        );
    }

    #[test]
    fn package_artifacts_require_exact_id_target_and_library_extension() {
        let value = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "game 0.1.0",
            "target": {
                "name": "game",
                "crate_types": ["cdylib", "rlib"]
            },
            "filenames": [
                "/game/target/debug/libgame.rlib",
                "/game/target/debug/libgame.so"
            ]
        });
        let package = PackageFilter {
            id: "game 0.1.0",
            name: "game",
            target_name: "game",
        };
        assert!(artifact_matches_package(&value, package));
        assert!(is_dynamic_library(Path::new(
            "/game/target/debug/libgame.so"
        )));
        assert!(!is_dynamic_library(Path::new(
            "/game/target/debug/libgame.rlib"
        )));

        let dependency = PackageFilter {
            id: "other 0.1.0",
            ..package
        };
        assert!(!artifact_matches_package(&value, dependency));
    }

    fn package(
        mode: Option<GodotRustMode>,
        godot_api: Option<GodotApiVersion>,
    ) -> CargoPackageModel {
        CargoPackageModel {
            id: "game 0.1.0".to_owned(),
            name: "game".to_owned(),
            manifest_path: PathBuf::from("/game/Cargo.toml"),
            workspace_default: true,
            godot_rs_dependency: true,
            godot_rust_enabled: true,
            godot_rust_mode: mode,
            godot_api,
            scripts_path: None,
            editor: crate::EditorWorkflowConfig::default(),
            script_mode_configured: mode == Some(GodotRustMode::Script),
            configuration_issues: Vec::new(),
            cdylib_targets: vec![crate::CargoTargetModel {
                name: "game".to_owned(),
                src_path: PathBuf::from("/game/src/lib.rs"),
            }],
        }
    }
}
