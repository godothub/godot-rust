use godot_rs_codegen::{
    ExpectedApiVersion, GodotApiVersion, LoadedApi, analyze_engine_api, diff_api,
    generate_api_snapshot, generate_binding_bundle, generate_engine_api, generate_raw_ffi,
    load_api, load_official_api_source, load_target_catalog, validate_api, verify_binding_bundle,
    verify_engine_api_coverage, verify_official_input,
};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

const USAGE: &str = "\
usage:
  godot_rs_codegen verify-targets <godot-api.toml>
  godot_rs_codegen verify-bundles <godot-api.toml> <bundle-root>
  godot_rs_codegen validate --godot <4.4..4.7> --source <godot-api.toml> <extension_api.json>
  godot_rs_codegen report-engine-api --godot <4.4..4.7> --source <godot-api.toml> <extension_api.json> [report.json]
  godot_rs_codegen <generate-snapshot|check-snapshot> --godot <4.4..4.7> --source <godot-api.toml> <extension_api.json> <output.rs>
  godot_rs_codegen <generate-engine-api|check-engine-api> --godot <4.4..4.7> --source <godot-api.toml> <extension_api.json> <output.rs>
  godot_rs_codegen <generate-sys|check-sys> --godot <4.4..4.7> --source <godot-api.toml> <gdextension_interface.h> <output.rs>
  godot_rs_codegen <generate-bundle|check-bundle> --godot <4.4..4.7> --source <godot-api.toml> [--gdextension-interface-json <gdextension_interface.json>] <gdextension_interface.h> <extension_api.json> <godot-4.x-output-directory>
  godot_rs_codegen diff <from-extension_api.json> <to-extension_api.json> [report.md]";

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("godot_rs_codegen failed: {error}");
        std::process::exit(1);
    }
}

fn run(arguments: Vec<String>) -> Result<(), Box<dyn Error>> {
    let (command, rest) = arguments.split_first().ok_or(USAGE)?;
    match command.as_str() {
        "verify-targets" => verify_targets(rest),
        "verify-bundles" => verify_bundles(rest),
        "diff" => run_diff(rest),
        "generate-bundle" | "check-bundle" => run_bundle_command(command, rest),
        "validate"
        | "report-engine-api"
        | "generate-snapshot"
        | "check-snapshot"
        | "generate-engine-api"
        | "check-engine-api"
        | "generate-sys"
        | "check-sys" => run_generation_command(command, rest),
        _ => Err(format!("unknown command `{command}`\n{USAGE}").into()),
    }
}

fn verify_bundles(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let [catalog_path, bundle_root] = arguments else {
        return Err(
            format!("verify-bundles expects the API manifest and bundle root\n{USAGE}").into(),
        );
    };
    let catalog = load_target_catalog(Path::new(catalog_path))?;
    for target in catalog.native_targets() {
        let source = load_official_api_source(Path::new(catalog_path), *target)?;
        let bundle_path = Path::new(bundle_root).join(format!("godot-{target}"));
        let manifest = verify_binding_bundle(&source, &bundle_path)?;
        println!(
            "Godot {} binding bundle verified: {} classes, {}",
            manifest.godot,
            manifest.api_inventory.engine_class_count,
            bundle_path.display()
        );
    }
    Ok(())
}

fn run_bundle_command(command: &str, arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let invocation = BundleInvocation::parse(arguments)?;
    let source = load_official_api_source(&invocation.source_manifest, invocation.target)?;
    let bundle = generate_binding_bundle(
        &source,
        &invocation.gdextension_interface,
        invocation.gdextension_interface_json.as_deref(),
        &invocation.extension_api,
    )?;
    if command == "generate-bundle" {
        bundle.write_to(&invocation.output_directory)?;
        println!("{}", invocation.output_directory.display());
    } else {
        bundle.check(&invocation.output_directory)?;
        println!(
            "binding bundle is current: {}",
            invocation.output_directory.display()
        );
    }
    Ok(())
}

fn verify_targets(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let [catalog_path] = arguments else {
        return Err(format!("verify-targets expects the API manifest\n{USAGE}").into());
    };
    let catalog = load_target_catalog(Path::new(catalog_path))?;
    for target in catalog.native_targets() {
        let source = load_official_api_source(Path::new(catalog_path), *target)?;
        println!(
            "Godot {} source entry validated: {}",
            source.target(),
            catalog_path
        );
    }
    println!(
        "API target matrix validated: Host {}, Native {}",
        catalog.host_baseline(),
        catalog
            .native_targets()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

fn run_diff(arguments: &[String]) -> Result<(), Box<dyn Error>> {
    if !(2..=3).contains(&arguments.len()) {
        return Err(format!(
            "diff expects two API JSON paths and an optional report path\n{USAGE}"
        )
        .into());
    }
    let from_path = &arguments[0];
    let to_path = &arguments[1];
    let from = load_api(Path::new(from_path))?;
    let to = load_api(Path::new(to_path))?;
    let report = diff_api(&from.api, &to.api).to_markdown();
    if let Some(report_path) = arguments.get(2) {
        fs::write(report_path, report)?;
        println!("{report_path}");
    } else {
        print!("{report}");
    }
    Ok(())
}

fn run_generation_command(command: &str, arguments: &[String]) -> Result<(), Box<dyn Error>> {
    let invocation = GenerationInvocation::parse(arguments)?;
    let source = load_official_api_source(&invocation.source_manifest, invocation.target)?;

    match command {
        "generate-sys" | "check-sys" => {
            let output_path = invocation
                .output_path
                .as_ref()
                .ok_or("Raw FFI generation requires an output Rust path")?;
            verify_official_input(&invocation.input_path, source.gdextension_interface())?;
            let generated = generate_raw_ffi(&invocation.input_path)?;
            if command == "generate-sys" {
                fs::write(output_path, generated)?;
                println!("{}", output_path.display());
            } else {
                check_generated_file(output_path, &generated, "Raw FFI")?;
            }
        }
        "validate"
        | "report-engine-api"
        | "generate-snapshot"
        | "check-snapshot"
        | "generate-engine-api"
        | "check-engine-api" => {
            verify_official_input(&invocation.input_path, source.extension_api())?;
            let loaded = load_validated_api(&invocation.input_path, invocation.target)?;
            run_api_command(command, &invocation, &loaded)?;
        }
        _ => unreachable!("commands are checked by run"),
    }
    Ok(())
}

fn run_api_command(
    command: &str,
    invocation: &GenerationInvocation,
    loaded: &LoadedApi,
) -> Result<(), Box<dyn Error>> {
    match command {
        "validate" => {
            if invocation.output_path.is_some() {
                return Err("validate does not accept an output path".into());
            }
            println!(
                "Godot {}.{}.{} API validated for target {}: {} classes, SHA-256 {}",
                loaded.api.header.version_major,
                loaded.api.header.version_minor,
                loaded.api.header.version_patch,
                invocation.target,
                loaded.api.classes.len(),
                loaded.sha256
            );
        }
        "report-engine-api" => {
            let report = analyze_engine_api(&loaded.api)?;
            let json = format!("{}\n", serde_json::to_string_pretty(&report)?);
            if let Some(output_path) = &invocation.output_path {
                fs::write(output_path, json)?;
                println!("{}", output_path.display());
            } else {
                print!("{json}");
            }
        }
        "generate-snapshot" => {
            let output_path = invocation
                .output_path
                .as_ref()
                .ok_or("snapshot generation requires an output Rust path")?;
            let generated = generate_api_snapshot(&loaded.api, &loaded.sha256);
            fs::write(output_path, generated)?;
            println!("{}", output_path.display());
        }
        "check-snapshot" => {
            let output_path = invocation
                .output_path
                .as_ref()
                .ok_or("snapshot check requires a generated Rust path")?;
            let expected = generate_api_snapshot(&loaded.api, &loaded.sha256);
            check_generated_file(output_path, &expected, "snapshot")?;
        }
        "generate-engine-api" => {
            let output_path = invocation
                .output_path
                .as_ref()
                .ok_or("engine API generation requires an output Rust path")?;
            let generated = generate_engine_api(&loaded.api, &loaded.sha256)?;
            fs::write(output_path, generated)?;
            println!("{}", output_path.display());
        }
        "check-engine-api" => {
            let output_path = invocation
                .output_path
                .as_ref()
                .ok_or("engine API check requires a generated Rust path")?;
            let report = analyze_engine_api(&loaded.api)?;
            verify_engine_api_coverage(&report)?;
            let expected = generate_engine_api(&loaded.api, &loaded.sha256)?;
            check_generated_file(output_path, &expected, "engine API")?;
            println!(
                "full official API coverage is complete: {} generated entries, {} explicitly classified entries, {} generated class methods, {} generated virtual overrides, {} raw-pointer methods intentionally omitted",
                report.generated_official_entries,
                report.classified_official_entries,
                report.generated_methods,
                report.generated_virtual_methods,
                report.unsafe_pointer_methods + report.unsafe_pointer_virtual_methods
            );
        }
        _ => unreachable!("non-API commands are handled separately"),
    }
    Ok(())
}

fn check_generated_file(path: &Path, expected: &str, label: &str) -> Result<(), Box<dyn Error>> {
    let actual = fs::read_to_string(path)?;
    if actual != expected {
        return Err(format!("generated {label} is stale: {}", path.display()).into());
    }
    println!("{label} is current: {}", path.display());
    Ok(())
}

struct GenerationInvocation {
    target: GodotApiVersion,
    source_manifest: PathBuf,
    input_path: PathBuf,
    output_path: Option<PathBuf>,
}

struct BundleInvocation {
    target: GodotApiVersion,
    source_manifest: PathBuf,
    gdextension_interface: PathBuf,
    gdextension_interface_json: Option<PathBuf>,
    extension_api: PathBuf,
    output_directory: PathBuf,
}

impl BundleInvocation {
    fn parse(arguments: &[String]) -> Result<Self, Box<dyn Error>> {
        let [godot_flag, target, source_flag, source_manifest, rest @ ..] = arguments else {
            return Err(format!("bundle command is missing required arguments\n{USAGE}").into());
        };
        if godot_flag != "--godot" {
            return Err(format!("expected `--godot`, found `{godot_flag}`\n{USAGE}").into());
        }
        if source_flag != "--source" {
            return Err(format!("expected `--source`, found `{source_flag}`\n{USAGE}").into());
        }
        let target = target.parse::<GodotApiVersion>()?;
        let (gdextension_interface_json, rest) = if let [flag, path, remaining @ ..] = rest {
            if flag == "--gdextension-interface-json" {
                (Some(PathBuf::from(path)), remaining)
            } else {
                (None, rest)
            }
        } else {
            (None, rest)
        };
        let [gdextension_interface, extension_api, output_directory] = rest else {
            return Err(format!(
                "bundle command expects Header, API JSON, and output directory\n{USAGE}"
            )
            .into());
        };
        let output_directory = PathBuf::from(output_directory);
        let expected_directory = format!("godot-{target}");
        if output_directory.file_name().and_then(|name| name.to_str())
            != Some(expected_directory.as_str())
        {
            return Err(format!(
                "binding bundle output directory must end with `{expected_directory}`"
            )
            .into());
        }
        Ok(Self {
            target,
            source_manifest: PathBuf::from(source_manifest),
            gdextension_interface: PathBuf::from(gdextension_interface),
            gdextension_interface_json,
            extension_api: PathBuf::from(extension_api),
            output_directory,
        })
    }
}

impl GenerationInvocation {
    fn parse(arguments: &[String]) -> Result<Self, Box<dyn Error>> {
        let [
            godot_flag,
            target,
            source_flag,
            source_manifest,
            input_path,
            rest @ ..,
        ] = arguments
        else {
            return Err(
                format!("generation command is missing required arguments\n{USAGE}").into(),
            );
        };
        if godot_flag != "--godot" {
            return Err(format!("expected `--godot`, found `{godot_flag}`\n{USAGE}").into());
        }
        if source_flag != "--source" {
            return Err(format!("expected `--source`, found `{source_flag}`\n{USAGE}").into());
        }
        if rest.len() > 1 {
            return Err("unexpected extra argument".into());
        }

        Ok(Self {
            target: target.parse()?,
            source_manifest: PathBuf::from(source_manifest),
            input_path: PathBuf::from(input_path),
            output_path: rest.first().map(PathBuf::from),
        })
    }
}

fn load_validated_api(
    api_path: &Path,
    target: GodotApiVersion,
) -> Result<LoadedApi, Box<dyn Error>> {
    let loaded = load_api(api_path)?;
    let issues = validate_api(
        &loaded.api,
        ExpectedApiVersion {
            major: target.major(),
            minor: target.minor(),
        },
    );
    if !issues.is_empty() {
        let details = issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n- ");
        return Err(format!("API validation failed:\n- {details}").into());
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn generation_invocation_requires_explicit_target_and_manifest() {
        let invocation = GenerationInvocation::parse(&args(&[
            "--godot",
            "4.6",
            "--source",
            "godot-api.toml",
            "extension_api.json",
            "generated.rs",
        ]))
        .expect("invocation should parse");
        assert_eq!(invocation.target, GodotApiVersion::new(4, 6));
        assert_eq!(invocation.source_manifest, PathBuf::from("godot-api.toml"));
        assert_eq!(invocation.output_path, Some(PathBuf::from("generated.rs")));
    }

    #[test]
    fn generation_invocation_rejects_patch_targets_and_implicit_defaults() {
        assert!(
            GenerationInvocation::parse(&args(&[
                "--godot",
                "4.6.3",
                "--source",
                "godot-api.toml",
                "extension_api.json",
            ]))
            .is_err()
        );
        assert!(
            GenerationInvocation::parse(&args(&[
                "4.6",
                "--source",
                "godot-api.toml",
                "extension_api.json",
            ]))
            .is_err()
        );
    }

    #[test]
    fn bundle_invocation_enforces_versioned_output_layout() {
        let invocation = BundleInvocation::parse(&args(&[
            "--godot",
            "4.7",
            "--source",
            "godot-api.toml",
            "--gdextension-interface-json",
            "gdextension_interface.json",
            "gdextension_interface.h",
            "extension_api.json",
            "generated/godot-4.7",
        ]))
        .expect("bundle invocation should parse");
        assert_eq!(invocation.target, GodotApiVersion::new(4, 7));
        assert_eq!(
            invocation.gdextension_interface_json,
            Some(PathBuf::from("gdextension_interface.json"))
        );
        assert!(
            BundleInvocation::parse(&args(&[
                "--godot",
                "4.7",
                "--source",
                "godot-api.toml",
                "gdextension_interface.h",
                "extension_api.json",
                "generated/current",
            ]))
            .is_err()
        );
    }

    #[test]
    fn high_level_engine_api_accepts_every_supported_target() {
        for target in ["4.4", "4.5", "4.6", "4.7"] {
            let invocation = GenerationInvocation::parse(&args(&[
                "--godot",
                target,
                "--source",
                "godot-api.toml",
                "extension_api.json",
                &format!("generated/godot-{target}/engine_api.rs"),
            ]))
            .expect("supported engine API target");
            assert_eq!(invocation.target.to_string(), target);
        }
    }
}
