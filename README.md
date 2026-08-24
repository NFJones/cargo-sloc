# cargo-loc

`cargo-loc` is a source line counter for supported files beneath a directory.
It evaluates Cargo feature selections and Rust conditional-compilation
attributes for reachable package Rust, while retaining other recognized files
under their package or an explicit Root scope. It answers questions such as:

- How much production code remains when tests are excluded?
- How much source is active with a particular feature set?
- How do counts differ by Cargo package in a workspace?

The installed executable is named `cargo-loc`, so Cargo invokes it as
`cargo loc`.

## Installation

Install a published release with:

```sh
cargo install cargo-loc
```

Install the current checkout with:

```sh
just install
cargo loc --help
```

Rust and Cargo 1.95 or newer are required.

## Usage

Count all supported, non-ignored files plus every eligible Cargo package,
feature, and package target beneath the current directory:

```sh
cargo loc
```

Count another Root, including directories with no Cargo manifest:

```sh
cargo loc ../workspace
```

Select a Cargo feature configuration:

```sh
cargo loc --no-default-features --features serde,simd
```

Select or exclude package targets and contexts:

```sh
cargo loc --lib --bins --exclude-target test --exclude-target bench
```

Exclude files that resolve to the Root scope while retaining selected Package
rows:

```sh
cargo loc --root-files exclude
```

Select a compilation target or emit schema-version 3 JSON:

```sh
cargo loc --target wasm32-wasip1
cargo loc --json
```

Package, feature, and standard target selectors follow Cargo syntax. Run
`cargo loc --help` for the complete option list.

## Output

The default report is a deterministic terminal table:

```text
╭───────────┬──────────┬───────┬───────┬───────┬────────┬──────────┬──────┬──────╮
│ Package   ┆ Language ┆ Files ┆ Total ┆ Lines ┆ Blanks ┆ Comments ┆ Code ┆ Test │
╞═══════════╪══════════╪═══════╪═══════╪═══════╪════════╪══════════╪══════╪══════╡
│ cargo-loc ┆ Rust     ┆    12 ┆  1842 ┆  1842 ┆    307 ┆      214 ┆ 1321 ┆    0 │
│ Total     ┆ All      ┆    12 ┆  1842 ┆  1842 ┆    307 ┆      214 ┆ 1321 ┆    0 │
╰───────────┴──────────┴───────┴───────┴───────┴────────┴──────────┴──────┴──────╯
```

Rows aggregate each resolved Scope and language. Package-owned rows use their
Cargo package name; files with no unique selected-Package owner use `<root>`.
Each physical file contributes to exactly one Scope and one language route,
even through symlink, hard-link, module, or Package aliases. `Code` and `Test`
are exclusive for configuration-aware Rust: source active in any production
context is `Code`, while source active only in test or benchmark contexts is
`Test`. `Comments` may overlap either category. `Total` is the physical-line
count (and therefore equals `Lines`); Scopes are sorted by descending aggregate
Total, with their language rows sorted by descending Total. A Scope name is
printed only on its first language row. Language accountants that cannot
determine Cargo-aware test provenance display `n/a` in the `Test` column; a
total containing such a row is also `n/a`. JSON schema version 3 represents
those values as `null`, uses explicit Package/Root scope objects, and labels
each row's accounting engine and precision.
The table is unstyled, contains no ANSI escape sequences, and is safe to
redirect to a file. JSON output includes normalized selection, Project targets,
context-specific features, Package identities, totals, and warnings.

Rust reached from selected Cargo targets uses cargo-loc's configuration-aware
syntax and module analysis. Other `.rs` files appear as `Rust (unconfigured)`
with no claim of Cargo reachability or cfg/test filtering. Other recognized
languages use the pinned Tokei 14 catalog and byte-oriented lexical counting.
Those rows do not claim Cargo target, feature, cfg, import, or test provenance
filtering, so their `Test` value is `n/a`. Embedded source is summarized into
its host-language row and NUL-bearing binary files are ignored.

Discovery performs one Root-local walk and honors nested `.gitignore` and
`.ignore` files without inheriting ancestor or global excludes. VCS metadata
and `.cargo-loc` state are structural exceptions. Other supported files,
including files outside Cargo projects and files in generated, vendored, or
build-output directories, are included unless a supported ignore file excludes
them. Directory symlinks are not followed; in-Root file aliases are globally
deduplicated, including hard links where the operating system exposes physical
identity, and out-of-Root aliases are skipped.

## Analysis boundary

The baseline command reports configuration-aware **source LOC**, not expanded
or compiler-observed LOC. It does not compile selected packages, execute build
scripts, or expand macros. In particular:

- macro definitions and invocations are counted as written, but expansions are
  not counted;
- `include!` does not add included source to the discovered module graph;
- build-script-generated source and build-script-provided custom cfg values are
  not observed;
- arbitrary compiler flags and Cargo profile effects are not fully modeled;
- `cfg!` is counted as source and does not remove control-flow branches; and
- unsupported-language files and dependency source outside the Root are
  ignored.

See [SPEC.md](SPEC.md) for normative behavior and
[docs/PERFORMANCE.md](docs/PERFORMANCE.md) for benchmark methodology.

Successful reports are stored in a versioned, fail-closed snapshot beneath
`PATH/.cargo-loc/`. A snapshot is reused only after validating the normalized
selection, project inputs, Cargo configuration, relevant environment,
toolchain identity, target specifications, symlink targets, JSON schema,
generic inventory policy, and Tokei catalog and adapter versions. Corrupt,
outdated, or uncertain records are ignored and recomputed. Library clients
that serve repeated requests can use `ResidentSession` to retain validated
Cargo/toolchain state and unchanged per-file analysis between refreshes.

## Development

Install [`just`](https://github.com/casey/just), then use:

```sh
just ci
just bench-smoke
just package
just install-smoke
```

`just release-check` runs the complete local release-readiness suite. The
`just install-smoke` recipe validates the generated `.crate` archive, installs
only its extracted source in isolated Cargo state, and exercises both direct
and Cargo-subcommand invocation. The
project is licensed under the Apache License, Version 2.0; see
[COPYING](COPYING). Release history is recorded in [CHANGELOG.md](CHANGELOG.md).

## References

- [Cargo external tools](https://doc.rust-lang.org/cargo/reference/external-tools.html)
- [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html)
- [Rust conditional compilation](https://doc.rust-lang.org/reference/conditional-compilation.html)
