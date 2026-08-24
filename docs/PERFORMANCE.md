# Performance

Elapsed time is cargo-loc's primary performance objective after correctness.
The default pipeline remains non-compiling and does not execute build scripts.

## Benchmarking

Run all phase and end-to-end benchmarks with:

```sh
just bench
```

The harness first emits one `cargo-loc-scenario` JSON line for each of these
repeatable end-to-end scenarios:

- `cold_fresh_workspace`: each sample analyzes a newly generated workspace;
- `warm_no_change`: a resident session validates and returns one unchanged
  report after an untimed priming refresh; and
- `one_source_edit`: a resident session alternates one controlled source-file
  edit while retaining validated Cargo/toolchain and unchanged-file state.

Each JSON record includes p50 and p95 elapsed milliseconds for the total run
and every pipeline phase, Cargo and rustc query counts, bounded subprocess
counts, parse/cfg/persistent snapshot outcomes and reasons, source/context
cardinalities, and peak process RSS when the platform exposes it. Peak RSS is
a process-lifetime high-water mark, not an allocation total for an individual
sample. Set `CARGO_LOC_BENCH_SCENARIO_SAMPLES` to a positive integer to control
the scenario sample count; the default is 10.

The harness also emits `cargo-loc-external-scenario` JSON lines that include
process startup and invocation overhead for:

- a cold direct `target/release/cargo-loc loc` run;
- a direct-binary validated rendered-result hit;
- a direct-binary source edit with reusable Cargo/toolchain preparation state;
  and
- a validated rendered-result hit through `cargo loc`.

These records contain wall-time p50/p95 values, invocation and cache-state
labels, and output byte counts. Output is compared byte-for-byte within each
scenario. Filesystem page state remains OS-controlled, and child-process peak
RSS is not currently collected; use the in-process scenario records for
pipeline query, subprocess, cache, workload, and process-RSS attribution.

For a fast compile-and-execute smoke check:

```sh
just bench-smoke
```

The checked-in Criterion harness generates a deterministic workspace at run
time. It separately measures Project discovery, Cargo/toolchain configuration,
Rust source-graph discovery, cfg-aware line accounting, report serialization,
and the complete command pipeline. Warm and edited scenarios exercise a
`ResidentSession`; one-shot successful commands also use versioned fail-closed
disk snapshots. Criterion output and the budgets below are advisory; `SPEC.md`
currently defines no normative latency or throughput threshold.

Criterion also measures cold direct-binary runs over small, medium, and large
synthetic workspaces; direct-binary rendered hits and source edits; Cargo
subcommand rendered hits; and a shared-source fixture with genuinely divergent
production/test semantic contexts. Set `CARGO_LOC_BENCH_BINARY` to override
the release binary used by external scenarios.

## Baseline capture

Use a release build on an otherwise idle machine and retain the structured
scenario lines with the benchmark environment:

```sh
rustc -Vv
cargo -V
uname -a
CARGO_LOC_BENCH_SCENARIO_SAMPLES=20 just bench -- --noplot \
  | tee cargo-loc-benchmark.txt
```

Record the CPU, memory, operating system, toolchain, corpus shape, command,
git revision, p50/p95 values, query/cache/workload counts, and peak RSS.
Compare output bytes and workload metrics before interpreting timing changes;
a faster run that analyzes less work is not an equivalent result.

The performance program uses these engineering targets:

| Scenario | Target |
| --- | ---: |
| Validated persistent no-change result | less than 15 ms p95 |
| One ordinary source edit with incremental state | 15–30 ms p95 |
| Stateless cold run | materially below the recorded baseline |

On the documented synthetic fixture, a representative release smoke sample
measured about 0.42 ms for a validated no-change refresh and 2.85 ms for one
source edit. The edited refresh launched no subprocesses, reused 35 unchanged
file analyses, and lowered one changed file. These observations are not release
guarantees; use multiple samples on an idle machine for comparisons.

## Baseline methodology

The initial manual baseline used a release binary on x86_64 Linux with Rust and
Cargo 1.97.1. The generated workspace contained 8 Packages, 104 reachable Rust
files, and 7,792 included lines. Each measurement used `/usr/bin/time`, wrote a
JSON report to a regular file, and compared three report files byte-for-byte.

| Implementation | Elapsed time | Maximum RSS | Determinism |
| --- | ---: | ---: | --- |
| Initial serial pipeline | 0.47–0.48 s | about 35 MiB | identical across 3 runs |
| Workspace/parse cache reuse | 0.14 s | about 35 MiB | identical across 3 runs |

These measurements are representative observations, not release guarantees.
Record the machine, toolchain, corpus, command, and before/after values when
making future performance changes.

Cache metrics cover parse, rustc cfg, validated snapshot hit/miss/write, and
resident invalidation outcomes. Workload metrics also expose file-analysis
lowerings, file/context evaluations, and the bounded accounting worker limit.
Tests establish byte-identical repeated, selection-matrix, no-change, edited,
restored-source, disk-hit, resident-refresh, and serial/parallel reports.

The `high_context_shared_source` Criterion case exercises one library and 24
binary report contexts over the same 21-file module graph. Workload metrics
distinguish report-facing `build_contexts` from interned `semantic_contexts`
and source-to-semantic `source_contexts`, so regressions can detect repeated
source work even when report bytes remain unchanged.

## Design constraints

- One Cargo metadata result is reused for all members of a discovered
  workspace.
- Source bytes and lossless syntax trees are read/parsed once per physical
  identity and Rust edition, then lowered into versioned owned `FileAnalysis`
  records. Rust-analyzer trees are not retained by accounting.
- Report-facing Build Contexts retain target and toolchain labels, while source
  traversal and accounting share package-local semantic contexts when edition,
  cfg options, recognized cfg inputs, provenance, and harness state are equal.
- Lowered cfg predicates are evaluated across semantic contexts with chunked
  bitsets, and line/token projections are produced in one owned-analysis pass.
- rustc cfg probes are cached per Project, compilation target, and crate type.
- Accounting uses one adaptive inventory-wide pool of at most eight scoped
  workers over owned `Send + Sync` file analysis, retains a serial cutoff for
  small workloads, and reduces chunk results in deterministic source order.
- Successful one-shot reports are atomically stored in versioned project-local
  `.cargo-loc` snapshots. Full input fingerprints reject source, manifest,
  lockfile, Cargo configuration, ignore, environment, target-spec, toolchain,
  and symlink changes before returning bytes.
- A separately versioned preparation record persists the owned Cargo inventory
  and resolved toolchain/cfg contexts. Its preparation-only fingerprint excludes
  ordinary source contents, so one-shot source edits can avoid Cargo and rustc
  subprocesses while manifest, lockfile, Cargo configuration, environment,
  target-spec, and toolchain changes fail closed and rebuild the record.
- `ResidentSession` validates a path-indexed input manifest for each refresh,
  reuses content digests when strong file identity/change stamps are stable,
  and hands changed source bytes directly to analysis. It tracks both selected
  module files and missing flat/nested/explicit candidates so newly created
  alternatives invalidate reachability without rereading unchanged sources.
- User-visible rows, warnings, totals, terminal tables, and JSON remain deterministic.
