//! Top-level parsing, validation, and buffered report orchestration.

use std::ffi::OsString;

use serde::{Deserialize, Serialize};

use crate::cli::ParseOutcome;
use crate::configuration::ConfiguredInventory;
use crate::discovery::Inventory;
use crate::error::AppError;
use crate::model::Selection;
use crate::report::Report;

/// Complete process output and status, buffered before stream writes.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessOutput {
    /// Complete stdout bytes.
    pub stdout: Vec<u8>,
    /// Complete stderr bytes.
    pub stderr: Vec<u8>,
    /// Process exit status.
    pub exit_code: u8,
}

/// Cargo and toolchain state reusable while configuration inputs remain valid.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PreparedExecution {
    inventory: Inventory,
    configured: ConfiguredInventory,
}

/// Runs the command against the process working directory.
pub fn run<I, T>(arguments: I) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    run_with_snapshot(arguments, true)
}

pub(crate) fn run_uncached<I, T>(arguments: I) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    run_with_snapshot(arguments, false)
}

fn run_with_snapshot<I, T>(arguments: I, snapshot: bool) -> ProcessOutput
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let current_directory = match std::env::current_dir() {
        Ok(directory) => directory,
        Err(error) => return operational_error(AppError::CurrentDirectory(error)),
    };
    match crate::cli::parse(arguments, &current_directory) {
        Ok(ParseOutcome::EarlyExit {
            stdout,
            stderr,
            exit_code,
        }) => ProcessOutput {
            stdout,
            stderr,
            exit_code,
        },
        Ok(ParseOutcome::Selection(selection)) => {
            if snapshot {
                crate::snapshot::run(selection)
            } else {
                execute(selection)
            }
        }
        Err(AppError::Usage(message)) => ProcessOutput {
            stdout: Vec::new(),
            stderr: format!("error: {message}\n\nFor more information, try '--help'.\n")
                .into_bytes(),
            exit_code: 2,
        },
        Err(error) => operational_error(error),
    }
}

pub(crate) fn execute(selection: Selection) -> ProcessOutput {
    match prepare(&selection) {
        Ok(prepared) => execute_prepared(selection, prepared),
        Err(error) => operational_error(error),
    }
}

pub(crate) fn prepare(selection: &Selection) -> Result<PreparedExecution, AppError> {
    let inventory = crate::metrics::phase(crate::metrics::Phase::Discovery, || {
        crate::discovery::discover(selection)
    })?;
    let configured = crate::metrics::phase(crate::metrics::Phase::Configuration, || {
        crate::configuration::resolve(selection, &inventory)
    })?;
    Ok(PreparedExecution {
        inventory,
        configured,
    })
}

pub(crate) fn execute_prepared(selection: Selection, prepared: PreparedExecution) -> ProcessOutput {
    let mut source_cache = crate::rust_source::SourceCache::default();
    let mut generic_source_cache = crate::generic_source::SourceCache::default();
    let mut generic_accounting_cache = crate::tokei_accounting::AccountingCache::default();
    execute_prepared_with_cache(
        selection,
        prepared,
        &mut source_cache,
        &mut generic_source_cache,
        &mut generic_accounting_cache,
    )
}

pub(crate) fn execute_prepared_with_cache(
    selection: Selection,
    prepared: PreparedExecution,
    source_cache: &mut crate::rust_source::SourceCache,
    generic_source_cache: &mut crate::generic_source::SourceCache,
    generic_accounting_cache: &mut crate::tokei_accounting::AccountingCache,
) -> ProcessOutput {
    let PreparedExecution {
        inventory,
        configured,
    } = prepared;
    crate::metrics::record_discovery(
        inventory.projects.len(),
        inventory
            .projects
            .iter()
            .map(|project| project.packages.len())
            .sum(),
    );
    crate::metrics::record_build_contexts(
        configured
            .projects
            .iter()
            .flat_map(|project| &project.packages)
            .map(|package| package.contexts.len())
            .sum(),
    );
    let sources = match crate::metrics::phase(crate::metrics::Phase::SourceDiscovery, || {
        crate::rust_source::discover_with_cache(
            &configured,
            Some(selection.root.as_path()),
            source_cache,
        )
    }) {
        Ok(sources) => sources,
        Err(error) => return operational_error(error),
    };
    crate::metrics::record_sources(
        sources
            .packages
            .iter()
            .map(|package| package.files.len())
            .sum(),
        sources
            .packages
            .iter()
            .map(|package| package.semantic_contexts.len())
            .sum(),
        sources
            .packages
            .iter()
            .flat_map(|package| &package.files)
            .map(|source| source.contexts.len())
            .sum(),
    );
    let generic_sources =
        match crate::metrics::phase(crate::metrics::Phase::SourceDiscovery, || {
            crate::generic_source::discover_root_with_cache(
                selection.root.as_path(),
                &configured,
                crate::tokei_accounting::is_candidate_path,
                generic_source_cache,
            )
        }) {
            Ok(sources) => sources,
            Err(error) => return operational_error(error),
        };
    let routed = match crate::metrics::phase(crate::metrics::Phase::Accounting, || {
        crate::routed_accounting::resolve(
            selection.root_files,
            &configured,
            &sources,
            &generic_sources,
            generic_accounting_cache,
        )
    }) {
        Ok(accounting) => accounting,
        Err(error) => return operational_error(error),
    };
    crate::metrics::phase(crate::metrics::Phase::Rendering, || {
        let mut report = Report::empty(selection);
        report.warnings = inventory.warnings;
        report.warnings.extend(configured.warnings.iter().cloned());
        report.warnings.extend(sources.warnings);
        report.warnings.extend(generic_sources.warnings);
        report.warnings.sort();
        if let Err(error) = report.apply_configuration(&configured) {
            return operational_error(error);
        }
        if let Err(error) = report.apply_contributions(&routed.contributions) {
            return operational_error(error);
        }
        match report.render() {
            Ok(stdout) => ProcessOutput {
                stdout,
                stderr: if report.selection.json {
                    Vec::new()
                } else {
                    report
                        .warnings
                        .iter()
                        .map(|warning| format!("warning[{}]: {}\n", warning.code, warning.message))
                        .collect::<String>()
                        .into_bytes()
                },
                exit_code: 0,
            },
            Err(error) => operational_error(error),
        }
    })
}

pub(crate) fn operational_error(error: AppError) -> ProcessOutput {
    ProcessOutput {
        stdout: Vec::new(),
        stderr: format!("cargo-sloc: {error}\n").into_bytes(),
        exit_code: 1,
    }
}
