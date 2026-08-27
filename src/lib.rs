//! Library-owned command orchestration for `cargo-sloc`.

pub mod accountant;
pub mod app;
pub mod cli;
pub mod configuration;
pub mod discovery;
pub mod error;
pub mod generic_source;
pub mod metrics;
pub mod model;
mod process;
pub mod report;
mod routed_accounting;
pub mod rust_accounting;
mod rust_analysis;
pub mod rust_source;
mod snapshot;
pub mod tokei_accounting;

use std::ffi::OsString;

pub use app::ProcessOutput;
pub use metrics::MeasuredRun;
pub use snapshot::ResidentSession;

/// Installs Unix SIGINT and SIGTERM forwarding for owned Cargo and rustc subprocess groups.
#[cfg(unix)]
pub fn install_unix_cancellation_handler() {
    process::install_cancellation_handler();
}

/// Runs `cargo-sloc` without writing to process streams.
///
/// The returned output is complete and buffered, allowing the process entry
/// point to keep stdout empty after parsing, validation, or rendering errors.
pub fn run<I, T>(arguments: I) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    app::run(arguments)
}

/// Runs `cargo-sloc` while collecting opt-in performance observations.
///
/// Metrics are thread-local, bypass persistent snapshots, and are not included
/// in user-visible report bytes.
pub fn run_with_metrics<I, T>(arguments: I) -> MeasuredRun
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let arguments = arguments.into_iter().map(Into::into).collect::<Vec<_>>();
    metrics::capture(|| app::run_uncached(arguments))
}
