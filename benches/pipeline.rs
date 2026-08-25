//! Phase and end-to-end benchmarks for representative cargo-sloc workloads.

mod support;

use std::ffi::OsString;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};
use std::{cell::Cell, env};

use cargo_sloc::ResidentSession;
use cargo_sloc::cli::ParseOutcome;
use cargo_sloc::metrics::PipelineMetrics;
use cargo_sloc::report::Report;
use criterion::{BatchSize, BenchmarkId, Criterion};
use support::{
    DivergentContextWorkspace, MixedLanguageWorkspace, SharedTargetWorkspace, SyntheticWorkspace,
};

const DEFAULT_SCENARIO_SAMPLES: usize = 10;

fn pipeline_benchmarks(criterion: &mut Criterion) {
    let fixture = SyntheticWorkspace::new(4, 8, 50);
    let shared_targets = SharedTargetWorkspace::new(24, 20, 20);
    let divergent_contexts = DivergentContextWorkspace::new(20, 50);
    let mixed_languages = MixedLanguageWorkspace::new(64, 8, 200);
    let resident_arguments = || {
        [
            OsString::from("--json"),
            fixture.root().as_os_str().to_owned(),
        ]
    };
    let mut resident =
        ResidentSession::new(resident_arguments()).expect("create resident benchmark session");
    let primed = resident.refresh();
    assert_eq!(primed.exit_code, 0, "prime resident benchmark session");
    let selection =
        match cargo_sloc::cli::parse([fixture.root().as_os_str().to_owned()], fixture.root())
            .expect("parse benchmark request")
        {
            ParseOutcome::Selection(selection) => selection,
            ParseOutcome::EarlyExit { .. } => panic!("unexpected benchmark CLI exit"),
        };
    let inventory =
        cargo_sloc::discovery::discover(&selection).expect("discover benchmark fixture");
    let configured = cargo_sloc::configuration::resolve(&selection, &inventory)
        .expect("configure benchmark fixture");
    let sources =
        cargo_sloc::rust_source::discover(&configured).expect("discover benchmark source");
    let accounting =
        cargo_sloc::rust_accounting::account(&sources).expect("account benchmark source");

    let mut group = criterion.benchmark_group("pipeline");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("project_discovery", |bencher| {
        bencher.iter(|| {
            cargo_sloc::discovery::discover(black_box(&selection))
                .expect("benchmark project discovery")
        });
    });
    group.bench_function("configuration", |bencher| {
        bencher.iter(|| {
            cargo_sloc::configuration::resolve(black_box(&selection), black_box(&inventory))
                .expect("benchmark configuration")
        });
    });
    group.bench_function("source_discovery", |bencher| {
        bencher.iter(|| {
            cargo_sloc::rust_source::discover(black_box(&configured))
                .expect("benchmark source discovery")
        });
    });
    group.bench_function("cfg_and_line_accounting", |bencher| {
        bencher.iter(|| {
            cargo_sloc::rust_accounting::account(black_box(&sources)).expect("benchmark accounting")
        });
    });
    group.bench_function("aggregation_and_json", |bencher| {
        bencher.iter(|| {
            let mut report = Report::empty(selection.clone());
            report
                .apply_configuration(black_box(&configured))
                .expect("apply benchmark configuration");
            report
                .apply_accounting(black_box(&accounting))
                .expect("apply benchmark accounting");
            report.render().expect("render benchmark report")
        });
    });
    group.bench_function("warm_no_change", |bencher| {
        bencher.iter(|| {
            let measured = resident.refresh_with_metrics();
            assert_eq!(measured.output.exit_code, 0, "benchmark command failed");
            black_box(measured)
        });
    });
    group.bench_function("cold_fresh_workspace", |bencher| {
        bencher.iter_batched(
            || SyntheticWorkspace::new(4, 8, 50),
            |cold_fixture| {
                let measured = cargo_sloc::run_with_metrics([
                    OsString::from("--json"),
                    cold_fixture.root().as_os_str().to_owned(),
                ]);
                assert_eq!(
                    measured.output.exit_code, 0,
                    "cold benchmark command failed"
                );
                black_box(measured)
            },
            BatchSize::PerIteration,
        );
    });
    let edited = Cell::new(false);
    group.bench_function("one_source_edit", |bencher| {
        bencher.iter_batched(
            || {
                let next = !edited.get();
                fixture.set_source_edit(next);
                edited.set(next);
                [
                    OsString::from("--json"),
                    fixture.root().as_os_str().to_owned(),
                ]
            },
            |arguments| {
                let _ = arguments;
                let measured = resident.refresh_with_metrics();
                assert_eq!(
                    measured.output.exit_code, 0,
                    "edited benchmark command failed"
                );
                black_box(measured)
            },
            BatchSize::PerIteration,
        );
    });
    group.bench_function("high_context_shared_source", |bencher| {
        bencher.iter(|| {
            let measured = cargo_sloc::run_with_metrics([
                OsString::from("--json"),
                shared_targets.root().as_os_str().to_owned(),
            ]);
            assert_eq!(
                measured.output.exit_code, 0,
                "shared-source benchmark command failed"
            );
            assert!(measured.metrics.workload.build_contexts >= 25);
            assert_eq!(measured.metrics.workload.semantic_contexts, 1);
            black_box(measured)
        });
    });
    group.bench_function("divergent_semantic_contexts", |bencher| {
        bencher.iter(|| {
            let measured = cargo_sloc::run_with_metrics([
                OsString::from("--json"),
                divergent_contexts.root().as_os_str().to_owned(),
            ]);
            assert_eq!(
                measured.output.exit_code, 0,
                "divergent-context benchmark command failed"
            );
            assert_eq!(measured.metrics.workload.semantic_contexts, 2);
            assert_eq!(
                measured.metrics.workload.file_context_evaluations,
                measured
                    .metrics
                    .workload
                    .reachable_source_files
                    .saturating_mul(2)
            );
            black_box(measured)
        });
    });
    group.bench_function("mixed_language_cold", |bencher| {
        bencher.iter_batched(
            || MixedLanguageWorkspace::new(64, 8, 200),
            |fixture| {
                let measured = cargo_sloc::run_with_metrics([
                    OsString::from("--json"),
                    fixture.root().as_os_str().to_owned(),
                ]);
                assert_eq!(measured.output.exit_code, 0, "mixed benchmark failed");
                black_box(measured)
            },
            BatchSize::PerIteration,
        );
    });
    let mut mixed_resident = ResidentSession::new([
        OsString::from("--json"),
        mixed_languages.root().as_os_str().to_owned(),
    ])
    .expect("create mixed-language resident benchmark session");
    assert_eq!(mixed_resident.refresh().exit_code, 0);
    group.bench_function("mixed_language_warm_no_change", |bencher| {
        bencher.iter(|| {
            let measured = mixed_resident.refresh_with_metrics();
            assert_eq!(measured.output.exit_code, 0, "mixed warm benchmark failed");
            black_box(measured)
        });
    });

    let binary = cargo_sloc_binary();
    for (size, dimensions) in [
        ("small", (1, 4, 20)),
        ("medium", (4, 8, 50)),
        ("large", (8, 16, 100)),
    ] {
        group.bench_with_input(
            BenchmarkId::new("external_direct_binary_cold", size),
            &dimensions,
            |bencher, &(packages, modules, lines)| {
                bencher.iter_batched(
                    || SyntheticWorkspace::new(packages, modules, lines),
                    |external_fixture| {
                        let sample = run_external(
                            ExternalInvocation::Direct(&binary),
                            external_fixture.root(),
                        );
                        assert_external_success(&sample.output);
                        black_box(sample)
                    },
                    BatchSize::PerIteration,
                );
            },
        );
    }

    let direct_warm = SyntheticWorkspace::new(4, 8, 50);
    let direct_expected = run_external(ExternalInvocation::Direct(&binary), direct_warm.root());
    assert_external_success(&direct_expected.output);
    group.bench_function("external_direct_binary_rendered_hit", |bencher| {
        bencher.iter(|| {
            let sample = run_external(ExternalInvocation::Direct(&binary), direct_warm.root());
            assert_same_output(&sample.output, &direct_expected.output);
            black_box(sample)
        });
    });

    let direct_edited = SyntheticWorkspace::new(4, 8, 50);
    let original_expected = run_external(ExternalInvocation::Direct(&binary), direct_edited.root());
    direct_edited.set_source_edit(true);
    let edited_expected = run_external(ExternalInvocation::Direct(&binary), direct_edited.root());
    direct_edited.set_source_edit(false);
    let process_edit = Cell::new(false);
    group.bench_function("external_direct_binary_source_edit", |bencher| {
        bencher.iter_batched(
            || {
                let edited = !process_edit.get();
                direct_edited.set_source_edit(edited);
                process_edit.set(edited);
                edited
            },
            |edited| {
                let sample =
                    run_external(ExternalInvocation::Direct(&binary), direct_edited.root());
                let expected = if edited {
                    &edited_expected.output
                } else {
                    &original_expected.output
                };
                assert_same_output(&sample.output, expected);
                black_box(sample)
            },
            BatchSize::PerIteration,
        );
    });

    let cargo_warm = SyntheticWorkspace::new(4, 8, 50);
    let cargo_expected = run_external(ExternalInvocation::Cargo(&binary), cargo_warm.root());
    assert_external_success(&cargo_expected.output);
    group.bench_function("external_cargo_subcommand_rendered_hit", |bencher| {
        bencher.iter(|| {
            let sample = run_external(ExternalInvocation::Cargo(&binary), cargo_warm.root());
            assert_same_output(&sample.output, &cargo_expected.output);
            black_box(sample)
        });
    });
    group.finish();
}

fn main() {
    report_scenario_metrics();
    report_external_scenario_metrics();
    let mut criterion = Criterion::default().configure_from_args();
    pipeline_benchmarks(&mut criterion);
    criterion.final_summary();
}

fn report_scenario_metrics() {
    let sample_count = env::var("CARGO_SLOC_BENCH_SCENARIO_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|samples| *samples > 0)
        .unwrap_or(DEFAULT_SCENARIO_SAMPLES);

    let cold = (0..sample_count)
        .map(|_| {
            let fixture = SyntheticWorkspace::new(4, 8, 50);
            measure(&fixture)
        })
        .collect::<Vec<_>>();
    print_scenario("cold_fresh_workspace", &cold);

    let warm_fixture = SyntheticWorkspace::new(4, 8, 50);
    let mut warm_session = resident(&warm_fixture);
    let _ = warm_session.refresh();
    let warm = (0..sample_count)
        .map(|_| warm_session.refresh_with_metrics().metrics)
        .collect::<Vec<_>>();
    print_scenario("warm_no_change", &warm);

    let edited_fixture = SyntheticWorkspace::new(4, 8, 50);
    let mut edited_session = resident(&edited_fixture);
    let _ = edited_session.refresh();
    let edited = (0..sample_count)
        .map(|index| {
            edited_fixture.set_source_edit(index % 2 == 0);
            edited_session.refresh_with_metrics().metrics
        })
        .collect::<Vec<_>>();
    print_scenario("one_source_edit", &edited);

    let mixed_fixture = MixedLanguageWorkspace::new(64, 8, 200);
    let mixed_arguments = || {
        [
            OsString::from("--json"),
            mixed_fixture.root().as_os_str().to_owned(),
        ]
    };
    let mixed_cold = cargo_sloc::run_with_metrics(mixed_arguments());
    assert_eq!(mixed_cold.output.exit_code, 0);
    print_scenario("mixed_language_cold", &[mixed_cold.metrics]);

    let mut mixed_session =
        ResidentSession::new(mixed_arguments()).expect("create mixed-language scenario session");
    let _ = mixed_session.refresh();
    let mixed_warm = (0..sample_count)
        .map(|_| mixed_session.refresh_with_metrics().metrics)
        .collect::<Vec<_>>();
    print_scenario("mixed_language_warm_no_change", &mixed_warm);

    let mixed_edited = (0..sample_count)
        .map(|index| {
            mixed_fixture.set_source_edit(index % 2 == 0);
            mixed_session.refresh_with_metrics().metrics
        })
        .collect::<Vec<_>>();
    print_scenario("mixed_language_one_source_edit", &mixed_edited);
}

fn resident(fixture: &SyntheticWorkspace) -> ResidentSession {
    ResidentSession::new([
        OsString::from("--json"),
        fixture.root().as_os_str().to_owned(),
    ])
    .expect("create resident scenario session")
}

fn measure(fixture: &SyntheticWorkspace) -> PipelineMetrics {
    let measured = cargo_sloc::run_with_metrics([
        OsString::from("--json"),
        fixture.root().as_os_str().to_owned(),
    ]);
    assert_eq!(measured.output.exit_code, 0, "scenario measurement failed");
    measured.metrics
}

fn print_scenario(name: &str, runs: &[PipelineMetrics]) {
    let first = runs.first().expect("scenario has at least one sample");
    for run in &runs[1..] {
        assert_eq!(run.queries, first.queries, "query counts changed in {name}");
        assert_eq!(
            run.subprocesses, first.subprocesses,
            "subprocess counts changed in {name}"
        );
        assert_eq!(run.caches, first.caches, "cache outcomes changed in {name}");
        assert_eq!(run.workload, first.workload, "workload changed in {name}");
    }
    let peak_rss_bytes = runs.iter().filter_map(|run| run.peak_rss_bytes).max();
    let summary = serde_json::json!({
        "scenario": name,
        "samples": runs.len(),
        "phases": {
            "total": phase_percentiles(runs, |run| run.phases.total),
            "discovery": phase_percentiles(runs, |run| run.phases.discovery),
            "configuration": phase_percentiles(runs, |run| run.phases.configuration),
            "source_discovery": phase_percentiles(runs, |run| run.phases.source_discovery),
            "accounting": phase_percentiles(runs, |run| run.phases.accounting),
            "rendering": phase_percentiles(runs, |run| run.phases.rendering),
        },
        "queries": {
            "cargo_metadata": first.queries.cargo_metadata,
            "cargo_package_id": first.queries.cargo_package_id,
            "rustc_host": first.queries.rustc_host,
            "rustc_cfg": first.queries.rustc_cfg,
        },
        "subprocesses": first.subprocesses,
        "caches": {
            "parse_hits": first.caches.parse_hits,
            "parse_misses": first.caches.parse_misses,
            "generic_source_hits": first.caches.generic_source_hits,
            "generic_source_misses": first.caches.generic_source_misses,
            "generic_accounting_hits": first.caches.generic_accounting_hits,
            "generic_accounting_misses": first.caches.generic_accounting_misses,
            "cfg_hits": first.caches.cfg_hits,
            "cfg_misses": first.caches.cfg_misses,
            "snapshot_hits": first.caches.snapshot_hits,
            "snapshot_misses": first.caches.snapshot_misses,
            "snapshot_writes": first.caches.snapshot_writes,
            "preparation_hits": first.caches.preparation_hits,
            "preparation_misses": first.caches.preparation_misses,
            "preparation_writes": first.caches.preparation_writes,
            "outcomes": first.caches.outcomes,
        },
        "workload": {
            "projects": first.workload.projects,
            "packages": first.workload.packages,
            "build_contexts": first.workload.build_contexts,
            "semantic_contexts": first.workload.semantic_contexts,
            "reachable_source_files": first.workload.reachable_source_files,
            "source_contexts": first.workload.source_contexts,
            "file_analysis_lowerings": first.workload.file_analysis_lowerings,
            "file_context_evaluations": first.workload.file_context_evaluations,
            "accounting_workers": first.workload.accounting_workers,
        },
        "peak_rss_bytes": peak_rss_bytes,
    });
    println!(
        "cargo-sloc-scenario {}",
        serde_json::to_string(&summary).expect("serialize scenario")
    );
}

fn phase_percentiles(
    runs: &[PipelineMetrics],
    phase: impl Fn(&PipelineMetrics) -> Duration,
) -> serde_json::Value {
    let mut durations = runs.iter().map(phase).collect::<Vec<_>>();
    durations.sort_unstable();
    serde_json::json!({
        "p50_ms": duration_millis(percentile(&durations, 50)),
        "p95_ms": duration_millis(percentile(&durations, 95)),
    })
}

fn percentile(durations: &[Duration], percentile: usize) -> Duration {
    let rank = durations.len().saturating_mul(percentile).div_ceil(100);
    durations[rank.saturating_sub(1).min(durations.len() - 1)]
}

fn duration_millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[derive(Clone, Copy)]
enum ExternalInvocation<'a> {
    Direct(&'a Path),
    Cargo(&'a Path),
}

struct ExternalSample {
    elapsed: Duration,
    output: Output,
}

fn cargo_sloc_binary() -> PathBuf {
    if let Some(path) = env::var_os("CARGO_SLOC_BENCH_BINARY") {
        return PathBuf::from(path);
    }
    let benchmark = env::current_exe().expect("resolve benchmark executable");
    let profile = benchmark
        .parent()
        .and_then(Path::parent)
        .expect("benchmark executable is beneath the Cargo profile directory");
    profile.join(format!("cargo-sloc{}", env::consts::EXE_SUFFIX))
}

fn run_external(invocation: ExternalInvocation<'_>, root: &Path) -> ExternalSample {
    let mut command = match invocation {
        ExternalInvocation::Direct(binary) => {
            let mut command = Command::new(binary);
            command.arg("sloc");
            command
        }
        ExternalInvocation::Cargo(binary) => {
            let mut command = Command::new("cargo");
            command.arg("sloc");
            let binary_directory = binary.parent().expect("cargo-sloc binary directory");
            let mut paths = vec![binary_directory.to_path_buf()];
            if let Some(path) = env::var_os("PATH") {
                paths.extend(env::split_paths(&path));
            }
            command.env(
                "PATH",
                env::join_paths(paths).expect("construct Cargo subcommand PATH"),
            );
            command
        }
    };
    command
        .arg("--json")
        .arg(root)
        .current_dir(root)
        .env("CARGO_SLOC_CACHE_DIR", root.join(".cargo-sloc"));
    let started = Instant::now();
    let output = command.output().expect("run external cargo-sloc benchmark");
    ExternalSample {
        elapsed: started.elapsed(),
        output,
    }
}

fn assert_external_success(output: &Output) {
    assert!(
        output.status.success(),
        "external cargo-sloc failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_same_output(actual: &Output, expected: &Output) {
    assert_external_success(actual);
    assert_eq!(actual.status.code(), expected.status.code());
    assert_eq!(actual.stdout, expected.stdout);
    assert_eq!(actual.stderr, expected.stderr);
}

fn report_external_scenario_metrics() {
    let sample_count = env::var("CARGO_SLOC_BENCH_SCENARIO_SAMPLES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|samples| *samples > 0)
        .unwrap_or(DEFAULT_SCENARIO_SAMPLES);
    let binary = cargo_sloc_binary();
    assert!(
        binary.is_file(),
        "release cargo-sloc binary not found at {}; set CARGO_SLOC_BENCH_BINARY to override",
        binary.display()
    );

    let cold = (0..sample_count)
        .map(|_| {
            let fixture = SyntheticWorkspace::new(4, 8, 50);
            let sample = run_external(ExternalInvocation::Direct(&binary), fixture.root());
            assert_external_success(&sample.output);
            sample
        })
        .collect::<Vec<_>>();
    print_external_scenario(
        "direct_binary_cold_fresh_workspace",
        "direct",
        "cold",
        &cold,
    );

    let warm_fixture = SyntheticWorkspace::new(4, 8, 50);
    let expected = run_external(ExternalInvocation::Direct(&binary), warm_fixture.root());
    assert_external_success(&expected.output);
    let warm = (0..sample_count)
        .map(|_| {
            let sample = run_external(ExternalInvocation::Direct(&binary), warm_fixture.root());
            assert_same_output(&sample.output, &expected.output);
            sample
        })
        .collect::<Vec<_>>();
    print_external_scenario(
        "direct_binary_rendered_result_hit",
        "direct",
        "rendered-result-hit",
        &warm,
    );

    let edited_fixture = SyntheticWorkspace::new(4, 8, 50);
    let original = run_external(ExternalInvocation::Direct(&binary), edited_fixture.root());
    edited_fixture.set_source_edit(true);
    let edited = run_external(ExternalInvocation::Direct(&binary), edited_fixture.root());
    let source_edits = (0..sample_count)
        .map(|index| {
            let is_edited = index % 2 == 0;
            edited_fixture.set_source_edit(is_edited);
            let sample = run_external(ExternalInvocation::Direct(&binary), edited_fixture.root());
            assert_same_output(
                &sample.output,
                if is_edited {
                    &edited.output
                } else {
                    &original.output
                },
            );
            sample
        })
        .collect::<Vec<_>>();
    print_external_scenario(
        "direct_binary_source_edit",
        "direct",
        "source-edit-preparation-cache-hit",
        &source_edits,
    );

    let cargo_fixture = SyntheticWorkspace::new(4, 8, 50);
    let cargo_expected = run_external(ExternalInvocation::Cargo(&binary), cargo_fixture.root());
    assert_external_success(&cargo_expected.output);
    let cargo = (0..sample_count)
        .map(|_| {
            let sample = run_external(ExternalInvocation::Cargo(&binary), cargo_fixture.root());
            assert_same_output(&sample.output, &cargo_expected.output);
            sample
        })
        .collect::<Vec<_>>();
    print_external_scenario(
        "cargo_subcommand_rendered_result_hit",
        "cargo-subcommand",
        "rendered-result-hit",
        &cargo,
    );
}

fn print_external_scenario(
    name: &str,
    invocation: &str,
    cache_state: &str,
    samples: &[ExternalSample],
) {
    let first = samples.first().expect("external scenario has samples");
    let mut elapsed = samples
        .iter()
        .map(|sample| sample.elapsed)
        .collect::<Vec<_>>();
    elapsed.sort_unstable();
    let summary = serde_json::json!({
        "scenario": name,
        "samples": samples.len(),
        "invocation": invocation,
        "cache_state": cache_state,
        "wall_time": {
            "p50_ms": duration_millis(percentile(&elapsed, 50)),
            "p95_ms": duration_millis(percentile(&elapsed, 95)),
        },
        "output": {
            "stdout_bytes": first.output.stdout.len(),
            "stderr_bytes": first.output.stderr.len(),
        },
        "filesystem_page_state": "os-controlled",
        "peak_rss_bytes": null,
        "pipeline_metrics": "reported by cargo-sloc-scenario records",
    });
    println!(
        "cargo-sloc-external-scenario {}",
        serde_json::to_string(&summary).expect("serialize external scenario")
    );
}
