use std::path::PathBuf;

fn main() {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let Some(path) = arguments.next().map(PathBuf::from) else {
        eprintln!("usage: godot_rs_module_check <project-module> [--exercise <res-path>]");
        std::process::exit(2);
    };
    let exercise_path = match (arguments.next(), arguments.next(), arguments.next()) {
        (None, None, None) => None,
        (Some(flag), Some(source_path), None) if flag == "--exercise" => Some(source_path),
        _ => {
            eprintln!("usage: godot_rs_module_check <project-module> [--exercise <res-path>]");
            std::process::exit(2);
        }
    };

    match godot_rs_host::validate_project_module(&path) {
        Ok(script_count) => {
            println!("validated project module with {script_count} script(s)");
        }
        Err(error) => {
            eprintln!("project module validation failed: {error}");
            std::process::exit(1);
        }
    }
    if let Some(source_path) = exercise_path {
        let source_path = source_path.to_string_lossy();
        if let Err(error) = godot_rs_host::exercise_project_module_callbacks(&path, &source_path) {
            eprintln!("project module callback validation failed: {error}");
            std::process::exit(1);
        }
    }
}
