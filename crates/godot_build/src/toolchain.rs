use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolchainReport {
    pub project_root: PathBuf,
    pub cargo_command: String,
    pub cargo_version_verbose: String,
    pub rustc_version_verbose: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RustTargetInstallReport {
    pub target: String,
    pub stdout: String,
    pub stderr: String,
}

const INSTALLABLE_RUST_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "aarch64-apple-ios",
    "aarch64-linux-android",
    "aarch64-pc-windows-msvc",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-ios-sim",
    "armv7-linux-androideabi",
    "armv7-unknown-linux-gnueabihf",
    "i686-linux-android",
    "i686-pc-windows-msvc",
    "i686-unknown-linux-gnu",
    "wasm32-unknown-emscripten",
    "x86_64-apple-darwin",
    "x86_64-apple-ios",
    "x86_64-linux-android",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

pub(crate) const WEB_RUST_TOOLCHAIN: &str = "nightly-2025-02-20";

pub fn probe_toolchain(
    project_root: impl AsRef<Path>,
    cargo: Option<impl AsRef<OsStr>>,
) -> Result<ToolchainReport, String> {
    let project_root = project_root.as_ref().canonicalize().map_err(|error| {
        format!(
            "could not resolve project root `{}`: {error}",
            project_root.as_ref().display()
        )
    })?;
    let cargo = cargo
        .map(|value| value.as_ref().to_owned())
        .unwrap_or_else(|| OsString::from("cargo"));
    let cargo_version_verbose =
        run_version_command(&project_root, &cargo, ["--version", "--verbose"])?;
    let rustc_version_verbose = run_version_command(&project_root, OsStr::new("rustc"), ["-Vv"])?;
    Ok(ToolchainReport {
        project_root,
        cargo_command: cargo.to_string_lossy().into_owned(),
        cargo_version_verbose,
        rustc_version_verbose,
    })
}

pub fn install_rust_target(
    project_root: impl AsRef<Path>,
    target: &str,
) -> Result<RustTargetInstallReport, String> {
    if !INSTALLABLE_RUST_TARGETS.contains(&target) {
        return Err(format!(
            "Rust target `{target}` is not part of the supported godot-rust export matrix"
        ));
    }
    let project_root = project_root.as_ref().canonicalize().map_err(|error| {
        format!(
            "could not resolve project root `{}`: {error}",
            project_root.as_ref().display()
        )
    })?;
    let (arguments, description) = if target == "wasm32-unknown-emscripten" {
        (
            vec![
                "toolchain",
                "install",
                WEB_RUST_TOOLCHAIN,
                "--profile",
                "minimal",
                "--component",
                "rust-src",
            ],
            format!(
                "`rustup toolchain install {WEB_RUST_TOOLCHAIN} --profile minimal \
                 --component rust-src`"
            ),
        )
    } else {
        (
            vec!["target", "add", target],
            format!("`rustup target add {target}`"),
        )
    };
    let output = crate::process::run_command(
        Command::new("rustup")
            .args(&arguments)
            .current_dir(&project_root),
        &description,
    )?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "rustup returned non-UTF-8 stdout".to_owned())?;
    let stderr = String::from_utf8(output.stderr)
        .map_err(|_| "rustup returned non-UTF-8 stderr".to_owned())?;
    if !output.status.success() {
        let details = stderr.trim();
        return Err(format!(
            "{description} exited with {}{}{}",
            output.status,
            if details.is_empty() { "" } else { ": " },
            details,
        ));
    }
    Ok(RustTargetInstallReport {
        target: target.to_owned(),
        stdout: stdout.trim().to_owned(),
        stderr: stderr.trim().to_owned(),
    })
}

fn run_version_command<const N: usize>(
    root: &Path,
    program: &OsStr,
    arguments: [&str; N],
) -> Result<String, String> {
    let output = crate::process::run_command(
        Command::new(program).args(arguments).current_dir(root),
        &format!("`{}` in `{}`", program.to_string_lossy(), root.display()),
    )?;
    if !output.status.success() {
        return Err(format!(
            "`{}` exited with {}: {}",
            program.to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_owned())
        .map_err(|_| {
            format!(
                "`{}` returned non-UTF-8 version output",
                program.to_string_lossy()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_workspace_toolchain_is_reported_without_writes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let report = probe_toolchain(root, None::<&str>).expect("toolchain");
        assert!(report.cargo_version_verbose.starts_with("cargo "));
        assert!(report.rustc_version_verbose.starts_with("rustc "));
    }

    #[test]
    fn target_installation_is_limited_to_the_export_matrix() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root");
        let error = install_rust_target(root, "nightly; delete-everything")
            .expect_err("unsupported target");
        assert!(error.contains("not part of the supported"));
    }
}
