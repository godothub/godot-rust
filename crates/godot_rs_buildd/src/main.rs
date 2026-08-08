use std::io::{self, Write};
use std::path::{Path, PathBuf};

fn main() {
    let mut arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let cancel_file = if arguments.len() >= 2
        && arguments.get(arguments.len() - 2).map(String::as_str) == Some("--cancel-file")
    {
        let path = arguments
            .last()
            .expect("cancel-file flag has one value")
            .clone();
        arguments.truncate(arguments.len() - 2);
        Some(path)
    } else {
        None
    };
    if let Some(cancel_file) = cancel_file {
        // SAFETY: The build daemon is still single-threaded and has not
        // spawned any command-reader threads. Child commands only read this
        // process-local value.
        unsafe {
            std::env::set_var(godot_rs_buildd::process_cancel_file_env(), cancel_file);
        }
    }
    let (request, response_file) = match arguments.as_slice() {
        [] => (None, None),
        [flag, request] if flag == "--request" => (Some(Ok(request.clone())), None),
        [flag, request] if flag == "--request-hex" => {
            (Some(godot_rs_buildd::decode_hex_request(request)), None)
        }
        [flag, request, response_flag, response_file]
            if flag == "--request" && response_flag == "--response-file" =>
        {
            (
                Some(Ok(request.clone())),
                Some(PathBuf::from(response_file)),
            )
        }
        [flag, request, response_flag, response_file]
            if flag == "--request-hex" && response_flag == "--response-file" =>
        {
            (
                Some(godot_rs_buildd::decode_hex_request(request)),
                Some(PathBuf::from(response_file)),
            )
        }
        _ => {
            eprintln!(
                "usage: godot_rs_buildd [--request <json> | --request-hex <hex>] \
                 [--response-file <path>] [--cancel-file <path>]"
            );
            std::process::exit(2);
        }
    };
    if let Some(request) = request {
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                eprintln!("godot-rust build daemon failed: {error}");
                std::process::exit(2);
            }
        };
        let response = godot_rs_buildd::handle_json_request(&request);
        let result = match response_file {
            Some(path) => write_response_file(&path, &response),
            None => {
                let mut output = io::stdout().lock();
                serde_json::to_writer(&mut output, &response)
                    .map_err(|error| error.to_string())
                    .and_then(|()| writeln!(output).map_err(|error| error.to_string()))
            }
        };
        if let Err(error) = result {
            eprintln!("godot-rust build daemon failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = godot_rs_buildd::serve(io::stdin().lock(), io::stdout().lock()) {
        eprintln!("godot-rust build daemon failed: {error}");
        std::process::exit(1);
    }
}

fn write_response_file(
    path: &Path,
    response: &godot_rs_buildd::BuilddResponse,
) -> Result<(), String> {
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            format!(
                "could not create response file `{}`: {error}",
                path.display()
            )
        })?;
    serde_json::to_writer(&mut output, response).map_err(|error| error.to_string())?;
    output
        .write_all(b"\n")
        .and_then(|()| output.sync_all())
        .map_err(|error| {
            format!(
                "could not flush response file `{}`: {error}",
                path.display()
            )
        })
}
