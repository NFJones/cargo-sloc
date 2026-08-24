# Specification Traceability

This matrix assigns one stable identifier to every paragraph in `SPEC.md` that
contains a normative `MUST` or `MUST NOT`. `tests/traceability.rs` verifies that
the identifiers are complete, unique, consecutive, and have no unverified
planned rows. A row marked **Current** has executable test coverage or an
explicitly reviewed implementation or documentation owner.

When a normative paragraph is added, removed, split, or merged, update this
matrix and its identifiers in the same change. Detailed tests may cover more
than one requirement, but every requirement retains its own row.

| ID | Section | State | Verification owner |
| --- | --- | --- | --- |
| SPEC-001 | 1 | Current | reviewed: `src/accountant.rs`, `tests/reporting.rs` |
| SPEC-002 | 2 | Current | `traceability::normative_paragraphs_have_entries` and documentation review |
| SPEC-003 | 3 | Current | reviewed: `src/model.rs`, `tests/cli.rs` |
| SPEC-004 | 3 | Current | reviewed: `src/model.rs`, `tests/cli.rs` |
| SPEC-005 | 4 | Current | reviewed: `src/app.rs`, `src/accountant.rs`, `tests/cargo_contexts.rs` |
| SPEC-006 | 4 | Current | reviewed: `src/app.rs`, `src/accountant.rs`, `tests/cargo_contexts.rs` |
| SPEC-007 | 4 | Current | `cargo_contexts::resolver_v2_keeps_host_features_separate_and_filters_inactive_targets` |
| SPEC-008 | 4 | Current | reviewed: `src/app.rs`, `src/accountant.rs`, `tests/cargo_contexts.rs` |
| SPEC-009 | 4 | Current | `cfg_spans::selected_parser_accepts_the_cfg_position_fixture_losslessly`; `tests/rust_accounting.rs` |
| SPEC-010 | 5 | Current | `src/cli.rs`, `tests/cli.rs` |
| SPEC-011 | 5 | Current | `src/cli.rs`, `tests/cli.rs` |
| SPEC-012 | 5 | Current | `src/cli.rs`, `tests/cli.rs` |
| SPEC-013 | 5 | Current | `src/cli.rs`, `tests/cli.rs` |
| SPEC-014 | 5 | Current | `src/cli.rs`, `tests/cli.rs` |
| SPEC-015 | 5 | Current | `src/cli.rs`, `tests/cli.rs` |
| SPEC-016 | 5 | Current | `src/cli.rs`, `tests/cli.rs` |
| SPEC-017 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-018 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-019 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-020 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-021 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-022 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-023 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-024 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-025 | 6.1 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-026 | 6.2 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-027 | 6.2 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-028 | 6.2 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-029 | 6.3 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-030 | 6.3 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-031 | 6.3 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-032 | 6.3 | Current | `src/discovery.rs`, `tests/discovery.rs` |
| SPEC-033 | 6.4 | Current | `src/discovery.rs`, `tests/discovery.rs`, `tests/reporting.rs` |
| SPEC-034 | 6.4 | Current | `src/discovery.rs`, `tests/discovery.rs`, `tests/reporting.rs` |
| SPEC-035 | 6.4 | Current | `src/discovery.rs`, `tests/discovery.rs`, `tests/reporting.rs` |
| SPEC-036 | 7.1 | Current | `src/configuration.rs`, `tests/cargo_contexts.rs`, `tests/configuration.rs` |
| SPEC-037 | 7.1 | Current | `src/configuration.rs`, `tests/cargo_contexts.rs`, `tests/configuration.rs` |
| SPEC-038 | 7.1 | Current | `src/configuration.rs`, `tests/cargo_contexts.rs`, `tests/configuration.rs` |
| SPEC-039 | 7.1 | Current | `src/configuration.rs`, `tests/cargo_contexts.rs`, `tests/configuration.rs` |
| SPEC-040 | 7.1 | Current | `src/configuration.rs`, `tests/cargo_contexts.rs`, `tests/configuration.rs` |
| SPEC-041 | 7.1 | Current | `src/configuration.rs`, `tests/cargo_contexts.rs`, `tests/configuration.rs` |
| SPEC-042 | 7.1 | Current | `src/configuration.rs`, `tests/cargo_contexts.rs`, `tests/configuration.rs` |
| SPEC-043 | 7.1 | Current | `cargo_contexts::resolver_v1_unifies_host_target_dev_and_inactive_target_features`; `cargo_contexts::resolver_v2_keeps_host_features_separate_and_filters_inactive_targets`; `cargo_contexts::resolver_v3_keeps_host_features_separate_and_filters_inactive_targets` |
| SPEC-044 | 7.1 | Current | `cargo_contexts::guppy_reproduces_the_observed_resolver_contexts` |
| SPEC-045 | 7.1 | Current | `src/configuration.rs`, `tests/cargo_contexts.rs`, `tests/configuration.rs` |
| SPEC-046 | 7.2 | Current | `src/configuration.rs`, `tests/configuration.rs` |
| SPEC-047 | 7.2 | Current | `src/configuration.rs`, `tests/configuration.rs` |
| SPEC-048 | 7.2 | Current | `src/configuration.rs`, `tests/configuration.rs` |
| SPEC-049 | 7.2 | Current | `src/configuration.rs`, `tests/configuration.rs` |
| SPEC-050 | 7.2 | Current | `src/configuration.rs`, `tests/configuration.rs` |
| SPEC-051 | 7.3 | Current | `src/configuration.rs`, `tests/configuration.rs`, `tests/rust_accounting.rs` |
| SPEC-052 | 7.3 | Current | `cargo_contexts::*` for Cargo-observed cfg/feature contexts; `tests/configuration.rs` |
| SPEC-053 | 7.3 | Current | `src/configuration.rs`, `tests/configuration.rs`, `tests/rust_accounting.rs` |
| SPEC-054 | 7.4 | Current | reviewed: `src/configuration.rs`, `README.md`, `tests/configuration.rs` |
| SPEC-055 | 7.4 | Current | reviewed: `src/configuration.rs`, `README.md`, `tests/configuration.rs` |
| SPEC-056 | 7.4 | Current | reviewed: `src/configuration.rs`, `README.md`, `tests/configuration.rs` |
| SPEC-057 | 7.4 | Current | reviewed: `src/configuration.rs`, `README.md`, `tests/configuration.rs` |
| SPEC-058 | 7.4 | Current | reviewed: `src/configuration.rs`, `README.md`, `tests/configuration.rs` |
| SPEC-059 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-060 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-061 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-062 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-063 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-064 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-065 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-066 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-067 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-068 | 8 | Current | `src/rust_source.rs`, `tests/rust_source.rs` |
| SPEC-069 | 9 | Current | `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/cfg_spans.rs`, `tests/rust_accounting.rs` |
| SPEC-070 | 9 | Current | `cfg_spans::conditional_attributes_have_precise_governed_nodes` and `cfg_spans::selected_toolchain_accepts_the_cfg_position_fixture` |
| SPEC-071 | 9 | Current | `cfg_spans::selected_parser_accepts_the_cfg_position_fixture_losslessly`; `tests/cfg_spans.rs` |
| SPEC-072 | 9 | Current | `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/cfg_spans.rs`, `tests/rust_accounting.rs` |
| SPEC-073 | 9 | Current | `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/cfg_spans.rs`, `tests/rust_accounting.rs` |
| SPEC-074 | 9 | Current | `cfg_spans::governed_ranges_preserve_independent_comments_and_same_line_source`; `tests/rust_accounting.rs` |
| SPEC-075 | 9 | Current | `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/cfg_spans.rs`, `tests/rust_accounting.rs` |
| SPEC-076 | 9 | Current | `cfg_spans::governed_ranges_preserve_independent_comments_and_same_line_source` |
| SPEC-077 | 9 | Current | `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/cfg_spans.rs`, `tests/rust_accounting.rs` |
| SPEC-078 | 9 | Current | `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/cfg_spans.rs`, `tests/rust_accounting.rs` |
| SPEC-079 | 9 | Current | `cfg_spans::attribute_like_tokens_inside_macros_are_not_parsed_as_attributes` |
| SPEC-080 | 9 | Current | `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/cfg_spans.rs`, `tests/rust_accounting.rs` |
| SPEC-081 | 10.1 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-082 | 10.1 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-083 | 10.1 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-084 | 10.1 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-085 | 10.2 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-086 | 10.2 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-087 | 10.3 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-088 | 10.3 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-089 | 10.3 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-090 | 10.3 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-091 | 10.3 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-092 | 10.3 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-093 | 10.3 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-094 | 10.4 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-095 | 10.4 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-096 | 10.4 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-097 | 10.4 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-098 | 10.4 | Current | `src/rust_accounting.rs`, `tests/rust_accounting.rs` |
| SPEC-099 | 10.5 | Current | `src/model.rs`, `src/report.rs`, `tests/reporting.rs` |
| SPEC-100 | 11.1 | Current | `src/report.rs`, `tests/reporting.rs` |
| SPEC-101 | 11.1 | Current | `src/report.rs`, `tests/reporting.rs` |
| SPEC-102 | 11.1 | Current | `src/report.rs`, `tests/reporting.rs` |
| SPEC-103 | 11.1 | Current | `src/report.rs`, `tests/reporting.rs` |
| SPEC-104 | 11.1 | Current | `src/report.rs`, `tests/reporting.rs` |
| SPEC-105 | 11.2 | Current | `src/report.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/reporting.rs` |
| SPEC-106 | 11.2 | Current | `src/report.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/reporting.rs` |
| SPEC-107 | 11.2 | Current | `src/report.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/reporting.rs` |
| SPEC-108 | 11.2 | Current | `src/report.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/reporting.rs` |
| SPEC-109 | 11.2 | Current | `src/report.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/reporting.rs` |
| SPEC-110 | 11.3 | Current | `src/report.rs`, `tests/cli.rs` |
| SPEC-111 | 11.3 | Current | `src/report.rs`, `tests/cli.rs` |
| SPEC-112 | 11.4 | Current | `src/app.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/reporting.rs` |
| SPEC-113 | 11.4 | Current | `src/app.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/reporting.rs` |
| SPEC-114 | 12 | Current | `src/app.rs`, `src/error.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/rust_source.rs` |
| SPEC-115 | 12 | Current | `src/app.rs`, `src/error.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/rust_source.rs` |
| SPEC-116 | 12 | Current | `src/app.rs`, `src/error.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/rust_source.rs` |
| SPEC-117 | 12 | Current | `src/app.rs`, `src/error.rs`, `tests/cli.rs`, `tests/configuration.rs`, `tests/rust_source.rs` |
| SPEC-118 | 13 | Current | `docs/PERFORMANCE.md`, `benches/pipeline.rs`, `tests/performance.rs` |
| SPEC-119 | 13 | Current | `src/snapshot.rs`, `src/discovery.rs`, `.gitignore`, `tests/traceability.rs` |
| SPEC-120 | 13 | Current | `docs/PERFORMANCE.md`, `benches/pipeline.rs`, `tests/performance.rs` |
| SPEC-121 | 14 | Current | reviewed: `README.md`, `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/rust_source.rs` |
| SPEC-122 | 14 | Current | reviewed: `README.md`, `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/rust_source.rs` |
| SPEC-123 | 14 | Current | reviewed: `README.md`, `src/rust_source.rs`, `src/rust_accounting.rs`, `tests/rust_source.rs` |
| SPEC-124 | 15 | Current | reviewed: `Cargo.toml`, `README.md`, `CHANGELOG.md`, `.github/workflows/ci.yml`, `tests/traceability.rs` |
| SPEC-125 | 15 | Current | reviewed: `Cargo.toml`, `README.md`, `CHANGELOG.md`, `.github/workflows/ci.yml`, `tests/traceability.rs` |
| SPEC-126 | 15 | Current | `Cargo.toml` and `README.md` minimum-version declarations; `.github/workflows/ci.yml` |
