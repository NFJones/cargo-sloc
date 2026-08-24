//! Cfg-aware Rust source projection and physical-line accounting.

use std::path::PathBuf;
use std::thread;

use crate::error::AppError;
use crate::model::{ContextKind, Counts};
use crate::rust_source::{PackageSources, ReachableSource, SourceInventory};

/// Package-level Rust counts in deterministic source-inventory order.
#[derive(Clone, Debug, Default)]
pub struct AccountingInventory {
    /// One result for every selected Package with reachable Rust source.
    pub packages: Vec<PackageAccounting>,
    /// Checked arithmetic sum of all Package counts.
    pub total: Counts,
}

/// Rust counts for one selected Package.
#[derive(Clone, Debug)]
pub struct PackageAccounting {
    /// Cargo's opaque Package ID.
    pub id: String,
    /// Cargo Package name.
    pub name: String,
    /// Absolute Package manifest path.
    pub manifest_path: PathBuf,
    /// Common line-accounting measures.
    pub counts: Counts,
}

/// Accounts every source Package and computes checked totals.
pub fn account(sources: &SourceInventory) -> Result<AccountingInventory, AppError> {
    account_with_workers(sources, default_worker_count())
}

/// Accounts source Packages with an explicit bounded worker limit.
///
/// This is primarily useful for deterministic equivalence tests and benchmark
/// experiments. Values below one are treated as one, and values above the
/// implementation concurrency bound are clamped.
pub fn account_with_workers(
    sources: &SourceInventory,
    worker_limit: usize,
) -> Result<AccountingInventory, AppError> {
    let worker_limit = worker_limit.clamp(1, MAX_ACCOUNTING_WORKERS);
    let mut packages = sources
        .packages
        .iter()
        .map(|package| {
            Ok(PackageAccounting {
                id: package.id.clone(),
                name: package.name.clone(),
                manifest_path: package.manifest_path.clone(),
                counts: Counts {
                    files: u64::try_from(package.files.len())
                        .map_err(|_| AppError::CountOverflow("counting Rust source files"))?,
                    ..Counts::default()
                },
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    let jobs = sources
        .packages
        .iter()
        .enumerate()
        .flat_map(|(package_index, package)| {
            package.files.iter().map(move |source| AccountingJob {
                package_index,
                package,
                source,
            })
        })
        .collect::<Vec<_>>();
    let worker_count = adaptive_worker_count(jobs.len(), worker_limit);
    crate::metrics::record_accounting_workers(worker_count);

    let results = account_jobs(&jobs, worker_count)?;
    for (package_index, counts) in results {
        let package = packages.get_mut(package_index).ok_or_else(|| {
            AppError::ReportInvariant(format!(
                "accounting job references missing package {package_index}"
            ))
        })?;
        package.counts = package.counts.checked_add(counts)?;
    }
    let mut total = Counts::default();
    for package in &packages {
        total = total.checked_add(package.counts)?;
    }
    Ok(AccountingInventory { packages, total })
}

const MAX_ACCOUNTING_WORKERS: usize = 8;
const MIN_SOURCES_PER_WORKER: usize = 8;

fn default_worker_count() -> usize {
    thread::available_parallelism()
        .map_or(1, usize::from)
        .clamp(1, MAX_ACCOUNTING_WORKERS)
}

fn adaptive_worker_count(source_count: usize, worker_limit: usize) -> usize {
    source_count
        .div_ceil(MIN_SOURCES_PER_WORKER)
        .clamp(1, worker_limit)
}

#[derive(Clone, Copy)]
struct AccountingJob<'a> {
    package_index: usize,
    package: &'a PackageSources,
    source: &'a ReachableSource,
}

fn account_jobs(
    jobs: &[AccountingJob<'_>],
    worker_count: usize,
) -> Result<Vec<(usize, Counts)>, AppError> {
    if worker_count <= 1 {
        return jobs
            .iter()
            .map(|job| Ok((job.package_index, account_source(job.package, job.source)?)))
            .collect();
    }

    let chunk_size = jobs.len().div_ceil(worker_count);
    let partials = thread::scope(|scope| {
        jobs.chunks(chunk_size)
            .map(|jobs| {
                scope.spawn(move || {
                    jobs.iter()
                        .map(|job| {
                            Ok((job.package_index, account_source(job.package, job.source)?))
                        })
                        .collect::<Result<Vec<_>, AppError>>()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|worker| {
                worker.join().map_err(|_| {
                    AppError::ReportInvariant("accounting worker panicked".to_owned())
                })?
            })
            .collect::<Result<Vec<_>, AppError>>()
    })?;
    Ok(partials.into_iter().flatten().collect())
}

fn account_source(package: &PackageSources, source: &ReachableSource) -> Result<Counts, AppError> {
    let line_count = source
        .evaluations
        .values()
        .next()
        .map_or(0, |evaluation| evaluation.lines.len());
    if line_count == 0 {
        return Ok(Counts::default());
    }

    let mut flags = vec![LineFlags::default(); line_count];

    for context_id in &source.contexts {
        let context = package.semantic_contexts.get(context_id.0).ok_or_else(|| {
            AppError::ReportInvariant(format!(
                "source `{}` references missing semantic context {}",
                source.path.display(),
                context_id.0
            ))
        })?;
        let evaluation = source.evaluations.get(context_id).ok_or_else(|| {
            AppError::ReportInvariant(format!(
                "source `{}` has semantic context {} without a cached evaluation",
                source.path.display(),
                context_id.0
            ))
        })?;
        if evaluation.lines.len() != line_count {
            return Err(AppError::ReportInvariant(format!(
                "source `{}` has inconsistent evaluated line counts",
                source.path.display()
            )));
        }
        for (flags, projection) in flags.iter_mut().zip(&evaluation.lines) {
            flags.blank |= projection.blank;
            flags.comment |= projection.comment;
            if projection.code {
                if context.provenance == ContextKind::Production {
                    flags.production = true;
                } else {
                    flags.test = true;
                }
            }
        }
    }

    let mut counts = Counts::default();
    for line in flags {
        if line.production {
            counts.code = checked_increment(counts.code, "counting production code lines")?;
        } else if line.test {
            counts.test = checked_increment(counts.test, "counting test-only code lines")?;
        }
        if line.comment {
            counts.comments = checked_increment(counts.comments, "counting comment lines")?;
        }
        if line.blank {
            counts.blanks = checked_increment(counts.blanks, "counting blank lines")?;
        }
        if line.production || line.test || line.comment || line.blank {
            counts.lines = checked_increment(counts.lines, "counting included lines")?;
        }
    }
    Ok(counts)
}

#[derive(Clone, Copy, Debug, Default)]
struct LineFlags {
    blank: bool,
    comment: bool,
    production: bool,
    test: bool,
}

fn checked_increment(value: u64, operation: &'static str) -> Result<u64, AppError> {
    value
        .checked_add(1)
        .ok_or(AppError::CountOverflow(operation))
}
