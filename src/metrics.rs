//! Opt-in observational metrics for performance benchmarks and regression tests.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ProcessOutput;

/// One command result paired with measurements collected during that run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeasuredRun {
    /// Complete buffered command result.
    pub output: ProcessOutput,
    /// Pipeline work and elapsed-time observations.
    pub metrics: PipelineMetrics,
}

/// Measurements for one command invocation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PipelineMetrics {
    /// Wall-clock durations for major pipeline phases.
    pub phases: PhaseMetrics,
    /// External semantic query counts.
    pub queries: QueryMetrics,
    /// Total bounded subprocesses started by cargo-loc.
    pub subprocesses: u64,
    /// In-process cache outcomes.
    pub caches: CacheMetrics,
    /// Cardinalities describing the analyzed workload.
    pub workload: WorkloadMetrics,
    /// Process peak resident set size after the run, when supported.
    pub peak_rss_bytes: Option<u64>,
}

/// Wall-clock durations for major pipeline phases.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PhaseMetrics {
    /// Complete measured command duration, including parsing and error handling.
    pub total: Duration,
    /// Cargo Project and Package discovery.
    pub discovery: Duration,
    /// Cargo feature, target, toolchain, and cfg resolution.
    pub configuration: Duration,
    /// Context-sensitive Rust source-graph discovery.
    pub source_discovery: Duration,
    /// Cfg-aware physical-line accounting.
    pub accounting: Duration,
    /// Report aggregation and buffered rendering.
    pub rendering: Duration,
}

/// Counts of external semantic queries issued during a command.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueryMetrics {
    /// `cargo metadata` queries.
    pub cargo_metadata: u64,
    /// On-demand `cargo pkgid` selector queries.
    pub cargo_package_id: u64,
    /// `rustc -vV` host-target queries.
    pub rustc_host: u64,
    /// `rustc --print cfg` target/crate-type queries.
    pub rustc_cfg: u64,
}

impl QueryMetrics {
    /// Returns the checked-by-construction sum of all query categories.
    #[must_use]
    pub fn total(self) -> u64 {
        self.cargo_metadata + self.cargo_package_id + self.rustc_host + self.rustc_cfg
    }
}

/// Aggregate cache outcomes and stable reasons for those outcomes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CacheMetrics {
    /// Reuses of a parsed physical source and edition.
    pub parse_hits: u64,
    /// Parses required for a new physical source and edition.
    pub parse_misses: u64,
    /// Reuses of a rustc cfg result within one Project resolution.
    pub cfg_hits: u64,
    /// rustc cfg probes required for a new target/crate-type key.
    pub cfg_misses: u64,
    /// Validated persistent snapshot hits.
    pub snapshot_hits: u64,
    /// Persistent snapshot misses or rejected records.
    pub snapshot_misses: u64,
    /// Successfully written persistent snapshots.
    pub snapshot_writes: u64,
    /// Validated persistent preparation-state hits.
    pub preparation_hits: u64,
    /// Missing or rejected persistent preparation-state records.
    pub preparation_misses: u64,
    /// Successfully written persistent preparation-state records.
    pub preparation_writes: u64,
    /// Counts keyed as `<cache>.<outcome>.<reason>` for diagnostic stability.
    pub outcomes: BTreeMap<String, u64>,
}

/// Workload cardinalities observed at pipeline boundaries.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkloadMetrics {
    /// Selected Cargo Projects.
    pub projects: u64,
    /// Selected Cargo Packages.
    pub packages: u64,
    /// Distinct Package Build Context records.
    pub build_contexts: u64,
    /// Distinct package-local source-semantic contexts after interning.
    pub semantic_contexts: u64,
    /// Package-owned reachable physical source records.
    pub reachable_source_files: u64,
    /// Source-to-semantic-context associations traversed by accounting.
    pub source_contexts: u64,
    /// Physical source and edition pairs lowered into owned analysis.
    pub file_analysis_lowerings: u64,
    /// File-to-semantic-context evaluations performed from owned analysis.
    pub file_context_evaluations: u64,
    /// Configured bounded worker limit for file accounting.
    pub accounting_workers: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Discovery,
    Configuration,
    SourceDiscovery,
    Accounting,
    Rendering,
}

#[derive(Clone, Copy)]
pub(crate) enum Query {
    CargoMetadata,
    CargoPackageId,
    RustcHost,
    RustcCfg,
}

#[derive(Clone, Copy)]
pub(crate) enum Cache {
    Parse,
    Cfg,
}

#[derive(Clone, Copy)]
pub(crate) enum CacheOutcome {
    Hit,
    Miss,
}

thread_local! {
    static ACTIVE: RefCell<Option<PipelineMetrics>> = const { RefCell::new(None) };
}

pub(crate) fn capture(operation: impl FnOnce() -> ProcessOutput) -> MeasuredRun {
    let previous = ACTIVE.with(|active| active.replace(Some(PipelineMetrics::default())));
    let started = Instant::now();
    let output = operation();
    let mut metrics = ACTIVE.with(|active| {
        active
            .replace(previous)
            .expect("metrics capture remains active until the measured run completes")
    });
    metrics.phases.total = started.elapsed();
    metrics.peak_rss_bytes = peak_rss_bytes();
    MeasuredRun { output, metrics }
}

pub(crate) fn phase<T>(phase: Phase, operation: impl FnOnce() -> T) -> T {
    if !is_active() {
        return operation();
    }
    let started = Instant::now();
    let result = operation();
    let elapsed = started.elapsed();
    with_active(|metrics| match phase {
        Phase::Discovery => metrics.phases.discovery += elapsed,
        Phase::Configuration => metrics.phases.configuration += elapsed,
        Phase::SourceDiscovery => metrics.phases.source_discovery += elapsed,
        Phase::Accounting => metrics.phases.accounting += elapsed,
        Phase::Rendering => metrics.phases.rendering += elapsed,
    });
    result
}

pub(crate) fn record_query(query: Query) {
    with_active(|metrics| match query {
        Query::CargoMetadata => metrics.queries.cargo_metadata += 1,
        Query::CargoPackageId => metrics.queries.cargo_package_id += 1,
        Query::RustcHost => metrics.queries.rustc_host += 1,
        Query::RustcCfg => metrics.queries.rustc_cfg += 1,
    });
}

pub(crate) fn record_subprocess() {
    with_active(|metrics| metrics.subprocesses += 1);
}

pub(crate) fn record_cache(cache: Cache, outcome: CacheOutcome, reason: &'static str) {
    with_active(|metrics| {
        let (cache_name, outcome_name) = match (cache, outcome) {
            (Cache::Parse, CacheOutcome::Hit) => {
                metrics.caches.parse_hits += 1;
                ("parse", "hit")
            }
            (Cache::Parse, CacheOutcome::Miss) => {
                metrics.caches.parse_misses += 1;
                ("parse", "miss")
            }
            (Cache::Cfg, CacheOutcome::Hit) => {
                metrics.caches.cfg_hits += 1;
                ("cfg", "hit")
            }
            (Cache::Cfg, CacheOutcome::Miss) => {
                metrics.caches.cfg_misses += 1;
                ("cfg", "miss")
            }
        };
        *metrics
            .caches
            .outcomes
            .entry(format!("{cache_name}.{outcome_name}.{reason}"))
            .or_default() += 1;
    });
}

pub(crate) fn record_discovery(projects: usize, packages: usize) {
    with_active(|metrics| {
        metrics.workload.projects = saturating_u64(projects);
        metrics.workload.packages = saturating_u64(packages);
    });
}

pub(crate) fn record_build_contexts(contexts: usize) {
    with_active(|metrics| metrics.workload.build_contexts = saturating_u64(contexts));
}

pub(crate) fn record_sources(files: usize, semantic_contexts: usize, contexts: usize) {
    with_active(|metrics| {
        metrics.workload.reachable_source_files = saturating_u64(files);
        metrics.workload.semantic_contexts = saturating_u64(semantic_contexts);
        metrics.workload.source_contexts = saturating_u64(contexts);
    });
}

pub(crate) fn record_file_analysis_lowering() {
    with_active(|metrics| metrics.workload.file_analysis_lowerings += 1);
}

pub(crate) fn record_file_context_evaluations(contexts: usize) {
    with_active(|metrics| {
        metrics.workload.file_context_evaluations = metrics
            .workload
            .file_context_evaluations
            .saturating_add(saturating_u64(contexts));
    });
}

pub(crate) fn record_accounting_workers(workers: usize) {
    with_active(|metrics| metrics.workload.accounting_workers = saturating_u64(workers));
}

pub(crate) fn record_snapshot_hit(reason: &str) {
    with_active(|metrics| {
        metrics.caches.snapshot_hits += 1;
        *metrics
            .caches
            .outcomes
            .entry(format!("snapshot.hit.{reason}"))
            .or_default() += 1;
    });
}

pub(crate) fn record_snapshot_miss(reason: &str) {
    with_active(|metrics| {
        metrics.caches.snapshot_misses += 1;
        *metrics
            .caches
            .outcomes
            .entry(format!("snapshot.miss.{reason}"))
            .or_default() += 1;
    });
}

pub(crate) fn record_snapshot_write() {
    with_active(|metrics| metrics.caches.snapshot_writes += 1);
}

pub(crate) fn record_preparation_hit(reason: &str) {
    with_active(|metrics| {
        metrics.caches.preparation_hits += 1;
        *metrics
            .caches
            .outcomes
            .entry(format!("preparation.hit.{reason}"))
            .or_default() += 1;
    });
}

pub(crate) fn record_preparation_miss(reason: &str) {
    with_active(|metrics| {
        metrics.caches.preparation_misses += 1;
        *metrics
            .caches
            .outcomes
            .entry(format!("preparation.miss.{reason}"))
            .or_default() += 1;
    });
}

pub(crate) fn record_preparation_write() {
    with_active(|metrics| metrics.caches.preparation_writes += 1);
}

fn is_active() -> bool {
    ACTIVE.with(|active| active.borrow().is_some())
}

fn with_active(operation: impl FnOnce(&mut PipelineMetrics)) {
    ACTIVE.with(|active| {
        if let Some(metrics) = active.borrow_mut().as_mut() {
            operation(metrics);
        }
    });
}

fn saturating_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(unix)]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` points to writable storage for `getrusage` to initialize.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: a successful `getrusage` initialized the complete record.
    let maximum = unsafe { usage.assume_init() }.ru_maxrss;
    let maximum = u64::try_from(maximum).ok()?;
    #[cfg(target_os = "macos")]
    let bytes = maximum;
    #[cfg(not(target_os = "macos"))]
    let bytes = maximum.saturating_mul(1024);
    Some(bytes)
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> Option<u64> {
    None
}
