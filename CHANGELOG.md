# Changelog

All notable changes to cargo-loc are documented here. The project follows
[Semantic Versioning](https://semver.org/).

## Unreleased

### Added

- `cargo loc [OPTIONS] [PATH]` external Cargo subcommand interface.
- Root-bounded workspace and standalone-package discovery with Cargo-compatible
  package, feature, compilation-target, and package-target selectors.
- Context-specific Cargo feature resolution and Rust target cfg probing without
  compiling selected packages or executing build scripts.
- Rust module graph discovery with `cfg`, recursive `cfg_attr`, and active
  `path` attribute handling.
- Rust-aware physical-line classification for blanks, comments, production
  code, and test-only code.
- Deterministic Package-level terminal-table reports and schema-version 1 JSON
  with Project, Package, target, feature-context, total, and warning provenance.
- Checked counter arithmetic, buffered failure output, and structured
  diagnostics.
- Cargo/rustc conformance fixtures, cfg-span tests, source/accounting/report
  golden coverage, deterministic performance tests, and Criterion benchmarks.

### Performance

- Reused workspace Cargo metadata and shared source text/lossless parses across
  source discovery and accounting. On the documented synthetic workload this
  reduced observed release-mode elapsed time from 0.47–0.48 seconds to 0.14
  seconds with maximum RSS remaining near 35 MiB.
- Added versioned fail-closed disk snapshots and a resident refresh engine.
  Representative release smoke samples returned a validated unchanged report
  in about 0.42 ms and recomputed one source edit in about 2.85 ms without
  Cargo or rustc subprocesses, while preserving byte-identical output.

### Compatibility

- Initial minimum supported Rust and Cargo version: 1.95.
- Initial JSON schema version: 1.

### Known limitations

- Macro expansions, `include!`-only source, generated source, build-script cfg
  output, arbitrary compiler flags, and compiler dead-code elimination are not
  included in baseline source LOC.
