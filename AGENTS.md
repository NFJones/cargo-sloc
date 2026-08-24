# Repository Guidelines

## Project Structure and Ownership

`cargo-loc` is a Rust 2024 Cargo subcommand. The installed `cargo-loc` binary
is invoked by Cargo as `cargo loc`. User-visible behavior MUST remain aligned
with `SPEC.md`; when implementation and specification disagree, either correct
the implementation or update the specification deliberately in the same
change.

- `SPEC.md`: normative command, discovery, cfg-filtering, accounting, output,
  diagnostics, compatibility, and performance requirements.
- `README.md`: concise user-facing overview, installation, and common usage.
- `Cargo.toml` and `Cargo.lock`: package metadata and reproducible dependency
  resolution. Keep `Cargo.lock` committed because this repository builds an
  executable.
- `src/main.rs`: thin process entry point. Keep argument-independent business
  logic out of this file.
- `src/lib.rs`: library-level application composition once implementation
  modules are introduced.
- `src/`: focused modules for CLI handling, Cargo/project discovery,
  configuration resolution, language Accountants, diagnostics, and reports.
- `tests/`: end-to-end command and fixture-based behavior tests when those no
  longer fit naturally beside their implementation owner.
- `Justfile`: canonical local build, validation, test, and installation entry
  points.
- `target/`: generated Cargo output; never edit or commit it.

As the implementation grows, keep language-neutral project discovery,
selection, aggregation, and reporting separate from Rust-specific syntax and
cfg analysis. Rust behavior belongs behind the Accountant boundary described
by `SPEC.md`. Prefer cohesive subsystem modules over a large `lib.rs` or
`main.rs`.

## Build, Test, and Development Commands

- `just`: build all targets and features in release mode.
- `just build`: build all targets and features in debug mode.
- `just build-release`: build all targets and features in release mode.
- `just run -- <args>`: run the release binary with arguments.
- `just check`: run `cargo check` for all targets and features.
- `just fmt`: apply Rust formatting.
- `just fmt-check`: verify Rust formatting without changing files.
- `just clippy`: run Clippy for all targets and features with warnings denied.
- `just test`: run all tests quietly without stopping after the first failing
  test binary.
- `just ci`: run formatting, checking, linting, and tests.
- `just install`: install or update the local `cargo-loc` executable.
- `just package`: verify that the crate can be packaged for publication.
- `just clean`: remove Cargo build artifacts.
- `just help`: list available recipes.

Automated test invocations MUST have a timeout of at least 120 seconds so hangs
are detected. Keep ordinary Cargo test output quiet unless diagnosing a
failure. Development recipes MUST remain usable on Linux and macOS and SHOULD
NOT rely on GNU-only shell extensions.

## Coding Style and Architecture

- Use Rust edition 2024 and standard `rustfmt` formatting.
- Use `snake_case` for modules, files, functions, and local variables;
  `UpperCamelCase` for types and traits; and `SCREAMING_SNAKE_CASE` for
  constants.
- Keep functions small and composable. Separate filesystem/process I/O,
  parsing, policy, accounting, and rendering where practical.
- Pass dependencies explicitly rather than introducing hidden global state.
- Prefer typed identifiers and domain records over loosely related strings and
  paths at subsystem boundaries.
- Use checked arithmetic for report counters and deterministic ordering for
  all user-visible output.
- Avoid `unwrap` and `expect` in production paths unless an invariant makes
  failure impossible and that invariant is documented. Add actionable context
  to propagated errors.
- New or substantially changed modules SHOULD have module-level rustdoc that
  explains their purpose, boundaries, and important invariants. Public items
  SHOULD document behavior, inputs, outputs, and failure conditions.
- Reuse established abstractions when they fit. Introduce a new abstraction
  only when it improves cohesion, clarity, or future Accountant support.

## Specification-Sensitive Requirements

- The normal accounting path MUST NOT compile selected packages, execute build
  scripts, or expand macros.
- Rust comments, tokens, attributes, and conditional source MUST be identified
  with Rust-aware lexical or syntactic analysis, not regular expressions.
- Context-specific Cargo feature sets MUST NOT be replaced with a
  package-level union when that changes cfg semantics.
- Package source MUST be counted once according to the deduplication and
  Production/Test precedence rules in `SPEC.md`.
- Parallel work MUST preserve deterministic output and MUST use bounded memory
  and concurrency. Elapsed time is the primary performance objective, but
  correctness takes precedence over speed.
- Unsupported-language files are ignored by default. Shared infrastructure
  MUST remain capable of supporting future language Accountants.
- Required report failures MUST leave stdout empty rather than emitting a
  partial Markdown or JSON document.

## Testing Requirements

- New behavior MUST include a happy-path test and at least one relevant edge or
  failure case.
- Bug fixes MUST include a regression test that fails before the fix and passes
  afterward.
- Prefer fixture-based end-to-end tests for Cargo discovery, feature/target
  selection, cfg filtering, diagnostics, and report contracts.
- Add focused unit tests for lexical classification, cfg predicate evaluation,
  path identity, checked aggregation, and deterministic sorting.
- Test Production/Test exclusivity, comment overlap, empty input, mixed line
  endings, final unterminated lines, nested block comments, strings containing
  comment markers, inactive cfg regions, and files shared across contexts.
- Use controlled fixtures or fake subprocess adapters for Cargo and rustc
  command construction. Tests MAY observe real compiler behavior as an oracle,
  but production accounting MUST retain the non-compiling baseline required by
  `SPEC.md`.
- All changes MUST pass `just fmt-check`, `just clippy`, and `just test` before
  handoff. Run `just check` early during implementation for faster feedback.

## Documentation and Compatibility

- Behavior or CLI changes MUST update `SPEC.md` and the relevant high-level
  `README.md` material in the same change.
- JSON schema changes MUST follow the versioning rules in `SPEC.md`; do not
  change required field types or meanings without incrementing the schema
  version.
- Each release MUST document its minimum supported Rust and Cargo versions.
- Avoid unnecessary platform assumptions in paths, process invocation, and
  byte handling. Add platform-gated tests when behavior genuinely differs.
- Do not preserve obsolete pre-1.0 behavior unless the specification or the
  active task explicitly requires compatibility.

## Commit and Review Guidance

- Keep changes focused and do not mix unrelated refactors with behavior work.
- Commit messages, when requested, should be short, imperative, and describe
  the user-visible or architectural result.
- Reviews and pull requests should summarize behavior changes, tests run,
  specification or schema effects, and known limitations.
- Never commit secrets, credentials, local environment files, generated build
  output, or editor state.
