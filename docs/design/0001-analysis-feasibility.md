# Analysis Feasibility and Dependency Decisions

## Status

Accepted for the initial implementation. The executable conformance tests that
support this decision live in `tests/cargo_contexts.rs` and
`tests/cfg_spans.rs`.

## Context

Two parts of `cargo-loc` are architecture-defining:

1. Cargo can compile one Package with different feature sets in host, target,
   normal, build, dev, and target-specific contexts. Stable
   `cargo metadata` exposes a package-level feature union, which is not enough
   to evaluate negative cfg predicates correctly.
2. Rust conditional attributes can govern syntax smaller than an item. Correct
   line accounting needs exact source ranges, retained comments and whitespace,
   and original line boundaries. A lossy AST or text deletion cannot provide
   those properties reliably.

The product path must remain non-compiling. Tests may compile controlled
fixtures to establish Cargo and rustc behavior as an oracle.

## Decision

### Cargo feature contexts

Use `cargo metadata` as the package/dependency input and use Guppy's Cargo
resolver model as the first implementation mechanism for context-specific
feature resolution.

The checked-in resolver fixtures exercise Cargo resolver versions 1, 2, and 3.
A test-only `RUSTC_WRAPPER` records the exact `--cfg feature="..."` arguments
from `cargo check --all-targets`. The tests establish these facts:

- resolver 1 unifies normal, build, dev, active-target, and inactive-target
  dependency features in the fixture;
- resolvers 2 and 3 keep host/build features separate from target features and
  omit the inactive target dependency;
- stable metadata reports a package-level union that is not any actual rustc
  context for the resolver 2 and 3 fixtures; and
- Guppy 0.17.26 reproduces the observed host and target feature sets for all
  three fixtures without running Cargo itself.

Guppy is therefore suitable as the initial resolver engine, but it is not
treated as infallible. Product code must key results by the complete modeled
context, configure the Project's resolver and host/target platform explicitly,
and add conformance fixtures as target, dependency, and feature behavior grows.
If a required Cargo context cannot be reproduced faithfully, `cargo-loc` must
fail rather than substitute metadata's union.

The Cargo observation harness is test-only and Unix-only at present because it
uses a shell wrapper. This does not place shell or compilation in the product
path. A portable wrapper executable may replace it when non-Unix CI is added.

### Rust syntax and lexical spans

Use `ra_ap_syntax` 0.0.349 as the lossless Rust concrete syntax tree. Pin the
version because rust-analyzer crates are published as a coordinated set without
the compatibility guarantees expected from ordinary semver releases.

The cfg-span fixture proves that this parser:

- round-trips accepted Rust source byte-for-byte;
- associates conditional attributes with source-file, module, item, field,
  variant, generic-parameter, statement, block-expression, match-arm,
  macro-expression, and same-line item nodes;
- exposes node ranges that leave independently active source before and after a
  governed same-line construct outside that construct's range;
- keeps a standalone leading comment outside the following item's range; and
- leaves attribute-like input inside an unexpanded macro token tree as tokens,
  rather than promoting it to source attributes.

The selected toolchain separately compiles the stable-position fixture. Parser
acceptance alone must never be used to claim that a syntax position is
supported by rustc.

Use the rustc-derived lexer coordinated with the selected rust-analyzer release
for token categories and byte ranges. Before production lexical accounting is
implemented, add the lexer crate as a direct, exactly pinned dependency rather
than relying on a transitive dependency. The accountant must keep original
source bytes and line indexes; it must project activity as byte intervals
instead of constructing filtered source text.

Parse cfg predicates into a small cargo-loc-owned structural representation for
bare names, name-value pairs, `all`, `any`, and `not`. Recursively process
`cfg_attr` from the lossless token-tree representation. Rust-analyzer provides
syntax ownership and delimiters; cargo-loc remains responsible for semantic
evaluation, generated attributes, and inactive-range policy. Regex deletion and
substring feature matching remain prohibited.

### Minimum toolchain

The initial minimum supported Rust and Cargo versions are 1.95. The package
uses Rust edition 2024, and `ra_ap_syntax` 0.0.349 declares Rust 1.95 as its
minimum. Guppy 0.17.26 requires an older compiler and does not raise that
minimum. `Cargo.toml` declares `rust-version = "1.95"`.

Dependency upgrades that raise the minimum toolchain require an explicit
compatibility decision, documentation update, and CI coverage. Parsing newer
syntax may require a coordinated rust-analyzer upgrade even when cargo-loc's
own Rust source still compiles on the documented minimum.

## Consequences

- Cargo-context correctness is tested against observed compiler invocations,
  while normal accounting remains non-compiling.
- The implementation may use Guppy rather than reimplementing Cargo's feature
  resolver immediately.
- Source projection can preserve trivia and same-line active material.
- Conditional-attribute semantics still require product implementation and
  exhaustive conformance tests; this decision proves feasibility, not feature
  completion.
- The parser pin intentionally creates a dependency-update maintenance task in
  exchange for stable syntax behavior and a clear minimum toolchain.
