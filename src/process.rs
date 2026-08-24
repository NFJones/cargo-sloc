//! Bounded subprocess execution for Cargo and rustc queries.

use std::io::{self, Read};
use std::process::{Command, ExitStatus, Stdio};
#[cfg(unix)]
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::Duration;
#[cfg(not(unix))]
use std::time::Instant;

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
            timeout: std::env::var("CARGO_LOC_SUBPROCESS_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .map(Duration::from_millis)
                .unwrap_or(DEFAULT_TIMEOUT),
            stream_bytes: std::env::var("CARGO_LOC_SUBPROCESS_OUTPUT_LIMIT")
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

    #[cfg(unix)]
    {
        let (events, receiver) = mpsc::channel();
        let stdout_reader = spawn_reader(
            stdout,
            limits.stream_bytes,
            Arc::clone(&exceeded),
            Some(events.clone()),
        );
        let stderr_reader = spawn_reader(
            stderr,
            limits.stream_bytes,
            Arc::clone(&exceeded),
            Some(events.clone()),
        );
        wait_with_notifications(
            child,
            purpose,
            limits,
            exceeded,
            stdout_reader,
            stderr_reader,
            events,
            receiver,
        )
    }

    #[cfg(not(unix))]
    {
        let stdout_reader = spawn_reader(stdout, limits.stream_bytes, Arc::clone(&exceeded));
        let stderr_reader = spawn_reader(stderr, limits.stream_bytes, Arc::clone(&exceeded));
        wait_with_polling(
            child,
            purpose,
            limits,
            exceeded,
            stdout_reader,
            stderr_reader,
        )
    }
}

#[cfg(unix)]
enum ProcessEvent {
    Exited(io::Result<ExitStatus>),
    OutputLimit,
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn wait_with_notifications(
    mut child: std::process::Child,
    purpose: String,
    limits: Limits,
    exceeded: Arc<AtomicBool>,
    stdout_reader: thread::JoinHandle<io::Result<Vec<u8>>>,
    stderr_reader: thread::JoinHandle<io::Result<Vec<u8>>>,
    events: Sender<ProcessEvent>,
    receiver: Receiver<ProcessEvent>,
) -> Result<BoundedOutput, ProcessError> {
    let process_id = child.id();
    let waiter = thread::spawn(move || {
        let result = child.wait();
        let _ = events.send(ProcessEvent::Exited(result));
    });

    let status = match receiver.recv_timeout(limits.timeout) {
        Ok(ProcessEvent::Exited(result)) => result.map_err(|source| ProcessError::Wait {
            purpose: purpose.clone(),
            source,
        })?,
        Ok(ProcessEvent::OutputLimit) => {
            terminate_process(process_id);
            let _ = receive_exit(&receiver, &purpose)?;
            join_waiter(waiter, &purpose)?;
            join_reader(stdout_reader, &purpose)?;
            join_reader(stderr_reader, &purpose)?;
            return Err(ProcessError::OutputLimit {
                purpose,
                limit: limits.stream_bytes,
            });
        }
        Err(RecvTimeoutError::Timeout) => {
            terminate_process(process_id);
            let _ = receive_exit(&receiver, &purpose)?;
            join_waiter(waiter, &purpose)?;
            join_reader(stdout_reader, &purpose)?;
            join_reader(stderr_reader, &purpose)?;
            return Err(ProcessError::Timeout {
                purpose,
                timeout: limits.timeout,
            });
        }
        Err(RecvTimeoutError::Disconnected) => {
            terminate_process(process_id);
            return Err(ProcessError::Capture {
                purpose,
                message: "subprocess wait channel disconnected".to_owned(),
            });
        }
    };

    join_waiter(waiter, &purpose)?;
    let stdout = join_reader(stdout_reader, &purpose)?;
    let stderr = join_reader(stderr_reader, &purpose)?;
    if exceeded.load(Ordering::Acquire) {
        return Err(ProcessError::OutputLimit {
            purpose,
            limit: limits.stream_bytes,
        });
    }
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

#[cfg(unix)]
fn receive_exit(
    receiver: &Receiver<ProcessEvent>,
    purpose: &str,
) -> Result<ExitStatus, ProcessError> {
    loop {
        match receiver.recv() {
            Ok(ProcessEvent::Exited(result)) => {
                return result.map_err(|source| ProcessError::Wait {
                    purpose: purpose.to_owned(),
                    source,
                });
            }
            Ok(ProcessEvent::OutputLimit) => {}
            Err(_) => {
                return Err(ProcessError::Capture {
                    purpose: purpose.to_owned(),
                    message: "subprocess wait channel disconnected".to_owned(),
                });
            }
        }
    }
}

#[cfg(unix)]
fn join_waiter(waiter: thread::JoinHandle<()>, purpose: &str) -> Result<(), ProcessError> {
    waiter.join().map_err(|_| ProcessError::Capture {
        purpose: purpose.to_owned(),
        message: "subprocess waiter panicked".to_owned(),
    })
}

#[cfg(not(unix))]
fn wait_with_polling(
    mut child: std::process::Child,
    purpose: String,
    limits: Limits,
    exceeded: Arc<AtomicBool>,
    stdout_reader: thread::JoinHandle<io::Result<Vec<u8>>>,
    stderr_reader: thread::JoinHandle<io::Result<Vec<u8>>>,
) -> Result<BoundedOutput, ProcessError> {
    let deadline = Instant::now() + limits.timeout;

    let status = loop {
        if exceeded.load(Ordering::Acquire) {
            terminate(&mut child);
            let _ = child.wait();
            join_reader(stdout_reader, &purpose)?;
            join_reader(stderr_reader, &purpose)?;
            return Err(ProcessError::OutputLimit {
                purpose,
                limit: limits.stream_bytes,
            });
        }
        if let Some(status) = child.try_wait().map_err(|source| ProcessError::Wait {
            purpose: purpose.clone(),
            source,
        })? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate(&mut child);
            let _ = child.wait();
            join_reader(stdout_reader, &purpose)?;
            join_reader(stderr_reader, &purpose)?;
            return Err(ProcessError::Timeout {
                purpose,
                timeout: limits.timeout,
            });
        }
        thread::sleep(POLL_INTERVAL);
    };

    let stdout = join_reader(stdout_reader, &purpose)?;
    let stderr = join_reader(stderr_reader, &purpose)?;
    if exceeded.load(Ordering::Acquire) {
        return Err(ProcessError::OutputLimit {
            purpose,
            limit: limits.stream_bytes,
        });
    }
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
}

fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    limit: usize,
    exceeded: Arc<AtomicBool>,
    #[cfg(unix)] events: Option<Sender<ProcessEvent>>,
) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                return Ok(captured);
            }
            let remaining = limit.saturating_sub(captured.len());
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
            if read > remaining {
                exceeded.store(true, Ordering::Release);
                #[cfg(unix)]
                if let Some(events) = &events {
                    let _ = events.send(ProcessEvent::OutputLimit);
                }
            }
        }
    })
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<Vec<u8>>>,
    purpose: &str,
) -> Result<Vec<u8>, ProcessError> {
    reader
        .join()
        .map_err(|_| ProcessError::Capture {
            purpose: purpose.to_owned(),
            message: "output reader panicked".to_owned(),
        })?
        .map_err(|error| ProcessError::Capture {
            purpose: purpose.to_owned(),
            message: error.to_string(),
        })
}

#[cfg(unix)]
fn terminate_process(process_id: u32) {
    let process_id = i32::try_from(process_id).unwrap_or(i32::MAX);
    // SAFETY: `process_id` is the positive child PID assigned as its PGID above.
    if unsafe { libc::kill(-process_id, libc::SIGKILL) } != 0 {
        // SAFETY: the positive PID identifies the spawned direct child.
        let _ = unsafe { libc::kill(process_id, libc::SIGKILL) };
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
}
