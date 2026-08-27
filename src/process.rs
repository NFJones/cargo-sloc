//! Bounded subprocess execution for Cargo and rustc queries.

use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
#[cfg(not(unix))]
use std::sync::mpsc::TryRecvError;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use thiserror::Error;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_STREAM_LIMIT: usize = 64 * 1024 * 1024;
#[cfg(not(unix))]
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Complete bounded output from a subprocess.
#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// Failure to execute or capture a bounded subprocess.
#[derive(Debug, Error)]
pub(crate) enum ProcessError {
    #[error("failed to start {purpose}: {source}")]
    Spawn {
        purpose: String,
        #[source]
        source: io::Error,
    },
    #[error("failed while waiting for {purpose}: {source}")]
    Wait {
        purpose: String,
        #[source]
        source: io::Error,
    },
    #[error("{purpose} timed out after {timeout:?}")]
    Timeout { purpose: String, timeout: Duration },
    #[error("{purpose} exceeded the {limit}-byte output limit")]
    OutputLimit { purpose: String, limit: usize },
    #[error("failed to capture {purpose} output: {message}")]
    Capture { purpose: String, message: String },
    #[error("{purpose} exited with {status}: {stderr}")]
    NonZero {
        purpose: String,
        status: ExitStatus,
        stderr: String,
    },
    #[error("{purpose} returned non-UTF-8 {stream}: {message}")]
    NonUtf8 {
        purpose: String,
        stream: &'static str,
        message: String,
    },
    #[error("{purpose} returned invalid output: {message}")]
    InvalidOutput { purpose: String, message: String },
}

#[derive(Clone, Copy)]
struct Limits {
    timeout: Duration,
    stream_bytes: usize,
}

impl Limits {
    fn from_environment() -> Self {
        Self {
            timeout: std::env::var("CARGO_SLOC_SUBPROCESS_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_TIMEOUT),
            stream_bytes: std::env::var("CARGO_SLOC_SUBPROCESS_OUTPUT_LIMIT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(DEFAULT_STREAM_LIMIT),
        }
    }
}

/// Executes a command with bounded elapsed time and captured output.
pub(crate) fn run(
    command: &mut Command,
    purpose: impl Into<String>,
) -> Result<BoundedOutput, ProcessError> {
    run_with_limits(command, purpose.into(), Limits::from_environment())
}

/// Executes one bounded `cargo metadata` query and returns its JSON document.
pub(crate) fn cargo_metadata_json(
    command: &cargo_metadata::MetadataCommand,
    purpose: impl Into<String>,
) -> Result<String, ProcessError> {
    crate::metrics::record_query(crate::metrics::Query::CargoMetadata);
    let purpose = purpose.into();
    let output = run(&mut command.cargo_command(), purpose.clone())?;
    if !output.status.success() {
        return Err(ProcessError::NonZero {
            purpose,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|error| ProcessError::NonUtf8 {
        purpose: purpose.clone(),
        stream: "stdout",
        message: error.to_string(),
    })?;
    let json = stdout
        .lines()
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| ProcessError::InvalidOutput {
            purpose: purpose.clone(),
            message: "no JSON document was present on stdout".to_owned(),
        })?;
    Ok(json.to_owned())
}

fn run_with_limits(
    command: &mut Command,
    purpose: String,
    limits: Limits,
) -> Result<BoundedOutput, ProcessError> {
    crate::metrics::record_subprocess();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    let mut child = command.spawn().map_err(|source| ProcessError::Spawn {
        purpose: purpose.clone(),
        source,
    })?;
    let stdout = child.stdout.take().expect("piped stdout is present");
    let stderr = child.stderr.take().expect("piped stderr is present");
    let exceeded = Arc::new(AtomicBool::new(false));
    let (events, receiver) = mpsc::channel();
    spawn_reader(
        stdout,
        limits.stream_bytes,
        Arc::clone(&exceeded),
        events.clone(),
        ProcessStream::Stdout,
    );
    spawn_reader(
        stderr,
        limits.stream_bytes,
        Arc::clone(&exceeded),
        events.clone(),
        ProcessStream::Stderr,
    );

    #[cfg(unix)]
    {
        wait_with_notifications(child, purpose, limits, events, receiver)
    }

    #[cfg(not(unix))]
    {
        drop(events);
        wait_with_polling(child, purpose, limits, exceeded, receiver)
    }
}

#[derive(Clone, Copy)]
enum ProcessStream {
    Stdout,
    Stderr,
}

enum ProcessEvent {
    #[cfg(unix)]
    Exited(io::Result<ExitStatus>),
    Stream {
        stream: ProcessStream,
        output: io::Result<Vec<u8>>,
    },
    OutputLimit,
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn wait_with_notifications(
    mut child: std::process::Child,
    purpose: String,
    limits: Limits,
    events: Sender<ProcessEvent>,
    receiver: Receiver<ProcessEvent>,
) -> Result<BoundedOutput, ProcessError> {
    let process_id = child.id();
    thread::spawn(move || {
        let result = child.wait();
        let _ = events.send(ProcessEvent::Exited(result));
    });
    let started = Instant::now();
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;
    loop {
        if status.is_some() && stdout.is_some() && stderr.is_some() {
            return Ok(BoundedOutput {
                status: status.take().expect("completed process status is present"),
                stdout: stdout.take().expect("completed stdout is present"),
                stderr: stderr.take().expect("completed stderr is present"),
            });
        }
        let remaining = limits.timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            if status.is_none() {
                terminate_process(process_id);
            }
            return Err(ProcessError::Timeout {
                purpose,
                timeout: limits.timeout,
            });
        }
        match receiver.recv_timeout(remaining) {
            Ok(ProcessEvent::Exited(result)) => match result {
                Ok(exit_status) => status = Some(exit_status),
                Err(source) => {
                    if status.is_none() {
                        terminate_process(process_id);
                    }
                    return Err(ProcessError::Wait { purpose, source });
                }
            },
            Ok(ProcessEvent::Stream { stream, output }) => {
                store_stream_output(stream, output, &purpose, &mut stdout, &mut stderr)?;
            }
            Ok(ProcessEvent::OutputLimit) => {
                if status.is_none() {
                    terminate_process(process_id);
                }
                return Err(ProcessError::OutputLimit {
                    purpose,
                    limit: limits.stream_bytes,
                });
            }
            Err(RecvTimeoutError::Timeout) => {
                if status.is_none() {
                    terminate_process(process_id);
                }
                return Err(ProcessError::Timeout {
                    purpose,
                    timeout: limits.timeout,
                });
            }
            Err(RecvTimeoutError::Disconnected) => {
                if status.is_none() {
                    terminate_process(process_id);
                }
                return Err(ProcessError::Capture {
                    purpose,
                    message: "subprocess wait channel disconnected".to_owned(),
                });
            }
        }
    }
}

#[cfg(not(unix))]
fn wait_with_polling(
    mut child: std::process::Child,
    purpose: String,
    limits: Limits,
    exceeded: Arc<AtomicBool>,
    receiver: Receiver<ProcessEvent>,
) -> Result<BoundedOutput, ProcessError> {
    let deadline = Instant::now() + limits.timeout;
    let mut status = None;
    let mut stdout = None;
    let mut stderr = None;

    loop {
        loop {
            match receiver.try_recv() {
                Ok(ProcessEvent::Stream { stream, output }) => {
                    store_stream_output(stream, output, &purpose, &mut stdout, &mut stderr)?;
                }
                Ok(ProcessEvent::OutputLimit) => {
                    terminate(&mut child);
                    return Err(ProcessError::OutputLimit {
                        purpose,
                        limit: limits.stream_bytes,
                    });
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if stdout.is_none() || stderr.is_none() {
                        terminate(&mut child);
                        return Err(ProcessError::Capture {
                            purpose,
                            message: "subprocess output channel disconnected".to_owned(),
                        });
                    }
                    break;
                }
            }
        }
        if exceeded.load(Ordering::Acquire) {
            terminate(&mut child);
            return Err(ProcessError::OutputLimit {
                purpose,
                limit: limits.stream_bytes,
            });
        }
        if status.is_none() {
            status = child.try_wait().map_err(|source| ProcessError::Wait {
                purpose: purpose.clone(),
                source,
            })?;
        }
        if status.is_some() && stdout.is_some() && stderr.is_some() {
            return Ok(BoundedOutput {
                status: status.take().expect("completed process status is present"),
                stdout: stdout.take().expect("completed stdout is present"),
                stderr: stderr.take().expect("completed stderr is present"),
            });
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            return Err(ProcessError::Timeout {
                purpose,
                timeout: limits.timeout,
            });
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    limit: usize,
    exceeded: Arc<AtomicBool>,
    events: Sender<ProcessEvent>,
    stream: ProcessStream,
) {
    thread::spawn(move || {
        let output = (|| {
            let mut captured = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                let read = reader.read(&mut buffer)?;
                if read == 0 {
                    return Ok(captured);
                }
                let remaining = limit.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..read.min(remaining)]);
                if read > remaining && !exceeded.swap(true, Ordering::AcqRel) {
                    let _ = events.send(ProcessEvent::OutputLimit);
                }
            }
        })();
        let _ = events.send(ProcessEvent::Stream { stream, output });
    });
}

fn store_stream_output(
    stream: ProcessStream,
    output: io::Result<Vec<u8>>,
    purpose: &str,
    stdout: &mut Option<Vec<u8>>,
    stderr: &mut Option<Vec<u8>>,
) -> Result<(), ProcessError> {
    let output = output.map_err(|error| ProcessError::Capture {
        purpose: purpose.to_owned(),
        message: error.to_string(),
    })?;
    match stream {
        ProcessStream::Stdout => *stdout = Some(output),
        ProcessStream::Stderr => *stderr = Some(output),
    }
    Ok(())
}

#[cfg(unix)]
fn terminate_process(process_id: u32) {
    let process_id = i32::try_from(process_id).unwrap_or(i32::MAX);
    #[cfg(target_os = "linux")]
    terminate_linux_descendants(process_id);
    // SAFETY: `process_id` is the positive child PID assigned as its PGID above.
    if unsafe { libc::kill(-process_id, libc::SIGKILL) } != 0 {
        // SAFETY: the positive PID identifies the spawned direct child.
        let _ = unsafe { libc::kill(process_id, libc::SIGKILL) };
    }
}

#[cfg(target_os = "linux")]
fn terminate_linux_descendants(process_id: i32) {
    for _ in 0..3 {
        let mut descendants = std::collections::BTreeSet::new();
        let mut pending = vec![process_id];
        while let Some(parent) = pending.pop() {
            for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
                let Ok(child) = entry.file_name().to_string_lossy().parse::<i32>() else {
                    continue;
                };
                if child == process_id || descendants.contains(&child) {
                    continue;
                }
                let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                    continue;
                };
                let Some(fields) = stat.rsplit_once(") ") else {
                    continue;
                };
                let Some(parent_id) = fields
                    .1
                    .split_whitespace()
                    .nth(1)
                    .and_then(|value| value.parse::<i32>().ok())
                else {
                    continue;
                };
                if parent_id == parent && descendants.insert(child) {
                    pending.push(child);
                }
            }
        }
        if descendants.is_empty() {
            thread::sleep(Duration::from_millis(5));
            continue;
        }
        for descendant in descendants {
            // SAFETY: `/proc` supplied a currently observed descendant of the owned child.
            let _ = unsafe { libc::kill(descendant, libc::SIGKILL) };
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(not(unix))]
fn terminate(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn subprocess_time_and_output_are_bounded() {
        let mut sleeper = Command::new("sh");
        sleeper.args(["-c", "sleep 30"]);
        let error = run_with_limits(
            &mut sleeper,
            "sleep probe".to_owned(),
            Limits {
                timeout: Duration::from_millis(50),
                stream_bytes: 1024,
            },
        )
        .expect_err("sleeping process must time out");
        assert!(matches!(error, ProcessError::Timeout { .. }));

        let mut flood = Command::new("sh");
        flood.args(["-c", "while :; do printf 0123456789; done"]);
        let error = run_with_limits(
            &mut flood,
            "flood probe".to_owned(),
            Limits {
                timeout: Duration::from_secs(5),
                stream_bytes: 1024,
            },
        )
        .expect_err("flooding process must exceed its output limit");
        assert!(matches!(error, ProcessError::OutputLimit { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn subprocess_timeout_includes_descendant_pipe_draining() {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30 &"]);
        let started = std::time::Instant::now();

        let error = run_with_limits(
            &mut command,
            "descendant pipe probe".to_owned(),
            Limits {
                timeout: Duration::from_millis(50),
                stream_bytes: 1024,
            },
        )
        .expect_err("pipe-holding descendant must time out");

        assert!(matches!(error, ProcessError::Timeout { .. }));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn subprocess_timeout_terminates_setsid_descendants() {
        let directory = tempfile::tempdir().expect("create PID directory");
        let pid_file = directory.path().join("escaped.pid");
        let script = format!(
            "setsid sh -c 'echo $$ > \"{}\"; sleep 30' & wait",
            pid_file.display()
        );
        let mut command = Command::new("sh");
        command.args(["-c", &script]);

        let error = run_with_limits(
            &mut command,
            "setsid descendant probe".to_owned(),
            Limits {
                timeout: Duration::from_millis(100),
                stream_bytes: 1024,
            },
        )
        .expect_err("setsid descendant probe must time out");
        assert!(matches!(error, ProcessError::Timeout { .. }));

        let pid = std::fs::read_to_string(&pid_file)
            .expect("read escaped descendant PID")
            .trim()
            .parse::<i32>()
            .expect("parse escaped descendant PID");
        let status = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        assert!(
            status.is_err() || status.is_ok_and(|status| status.contains(") Z ")),
            "escaped descendant {pid} remained runnable"
        );
    }
}
