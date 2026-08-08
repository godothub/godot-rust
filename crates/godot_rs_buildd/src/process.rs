use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

pub(crate) const CANCEL_FILE_ENV: &str = "GODOT_RS_BUILD_CANCEL_FILE";
const OUTPUT_LIMIT: usize = 64 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(20);
#[cfg(unix)]
const TERMINATION_GRACE: Duration = Duration::from_secs(2);

pub(crate) fn run_command(command: &mut Command, description: &str) -> Result<Output, String> {
    let cancel_file = std::env::var_os(CANCEL_FILE_ENV).map(std::path::PathBuf::from);
    run_command_with_cancel_file(command, description, cancel_file.as_deref())
}

fn run_command_with_cancel_file(
    command: &mut Command,
    description: &str,
    cancel_file: Option<&std::path::Path>,
) -> Result<Output, String> {
    if cancellation_requested(cancel_file) {
        return Err(format!("{description} was cancelled before it started"));
    }
    configure_process_group(command);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start {description}: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{description} has no stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{description} has no stderr pipe"))?;
    let stdout_reader = std::thread::spawn(move || read_bounded(stdout));
    let stderr_reader = std::thread::spawn(move || read_bounded(stderr));

    let status = loop {
        if cancellation_requested(cancel_file) {
            terminate_process_tree(&mut child);
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(format!("{description} was cancelled"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(error) => {
                terminate_process_tree(&mut child);
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("could not wait for {description}: {error}"));
            }
        }
    };
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| format!("{description} stdout reader panicked"))?
        .map_err(|error| format!("could not read {description} stdout: {error}"))?;
    let (stderr, stderr_truncated) = stderr_reader
        .join()
        .map_err(|_| format!("{description} stderr reader panicked"))?
        .map_err(|error| format!("could not read {description} stderr: {error}"))?;
    if stdout_truncated || stderr_truncated {
        return Err(format!(
            "{description} output exceeded the {} byte safety limit",
            OUTPUT_LIMIT
        ));
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded(mut pipe: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut truncated = false;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = pipe.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        if output.len() < OUTPUT_LIMIT {
            let keep = read.min(OUTPUT_LIMIT - output.len());
            output.extend_from_slice(&buffer[..keep]);
            truncated |= keep != read;
        } else {
            truncated = true;
        }
    }
    Ok((output, truncated))
}

fn cancellation_requested(cancel_file: Option<&std::path::Path>) -> bool {
    cancel_file.is_some_and(std::path::Path::exists)
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_tree(child: &mut std::process::Child) {
    unsafe extern "C" {
        fn kill(process_id: i32, signal: i32) -> i32;
    }
    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;
    let Ok(process_id) = i32::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };
    // SAFETY: A negative PID targets only the process group created for this
    // exact child. Signal constants are stable POSIX values on supported
    // Unix editor hosts.
    let _ = unsafe { kill(-process_id, SIGTERM) };
    let deadline = Instant::now() + TERMINATION_GRACE;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(POLL_INTERVAL),
            Err(_) => break,
        }
    }
    // SAFETY: The same private child process group is force-stopped only
    // after its graceful termination window expires.
    let _ = unsafe { kill(-process_id, SIGKILL) };
}

#[cfg(windows)]
fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn ordinary_commands_preserve_stdout_stderr_and_status() {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "echo output"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "printf output"]);
            command
        };
        let output = run_command(&mut command, "test command").expect("command output");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "output");
    }

    #[test]
    fn cancellation_stops_the_child_process_tree() {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let marker = std::env::temp_dir().join(format!(
            "godot-rust-cancel-test-{}-{id}",
            std::process::id()
        ));
        let marker_writer = marker.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(marker_writer, b"cancel\n").expect("cancellation marker");
        });
        let mut command = if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", "ping -n 20 127.0.0.1 >NUL"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 10"]);
            command
        };
        let started = std::time::Instant::now();
        let error = run_command_with_cancel_file(&mut command, "slow command", Some(&marker))
            .expect_err("cancelled command");
        writer.join().expect("marker writer");
        let _ = std::fs::remove_file(&marker);
        assert!(error.contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
