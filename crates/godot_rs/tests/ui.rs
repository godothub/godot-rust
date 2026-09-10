use std::fmt::Write;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn script_examples_accept_valid_code_and_reject_invalid_code() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../target"))
        .join("ui");
    let target = std::path::absolute(target).expect("absolute UI target directory");
    let project = target.join("cases");
    std::fs::create_dir_all(&project).expect("create UI test project");
    std::fs::copy(root.join("../../Cargo.lock"), project.join("Cargo.lock"))
        .expect("reuse workspace dependencies");
    let mut manifest = format!(
        "[package]\nname = \"godot-rust-ui\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
         [workspace]\n[dependencies]\ngodot_rs = {{ path = {} }}\n",
        serde_json::to_string(&root).expect("SDK path")
    );
    let mut cases = Vec::new();
    for category in ["pass", "fail"] {
        let mut files = std::fs::read_dir(root.join("tests/ui").join(category))
            .expect("UI test cases")
            .map(|entry| entry.expect("UI test file").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
            .collect::<Vec<_>>();
        files.sort();
        assert!(!files.is_empty(), "missing {category} examples");
        for file in files {
            let name = format!("{category}_{}", file.file_stem().unwrap().to_str().unwrap());
            writeln!(
                manifest,
                "[[bin]]\nname = {name:?}\npath = {}",
                serde_json::to_string(&file).expect("example path")
            )
            .unwrap();
            cases.push((name, file, category == "pass"));
        }
    }
    std::fs::write(project.join("Cargo.toml"), manifest).expect("UI test manifest");
    for (name, file, should_pass) in cases {
        let output = Command::new(env!("CARGO"))
            .current_dir(&project)
            .args([
                "check",
                "--offline",
                "--message-format=json",
                "--bin",
                &name,
            ])
            .arg("--target-dir")
            .arg(&target)
            .output()
            .expect("compile UI example");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if should_pass {
            assert!(output.status.success(), "{name}\n{stdout}\n{stderr}");
        } else {
            let has_source_error = stdout.lines().any(|line| {
                let Ok(message) = serde_json::from_str::<serde_json::Value>(line) else {
                    return false;
                };
                message["reason"] == "compiler-message"
                    && message["message"]["level"] == "error"
                    && message["message"]["spans"].as_array().is_some_and(|spans| {
                        spans.iter().any(|span| {
                            span["is_primary"] == true
                                && span["file_name"].as_str().is_some_and(|name| {
                                    PathBuf::from(name).file_name() == file.file_name()
                                })
                        })
                    })
            });
            assert!(
                !output.status.success() && has_source_error,
                "{name} must report a compiler error in its source\n{stdout}\n{stderr}"
            );
        }
    }
}
