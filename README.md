# cargo-loc

`cargo-loc` is a configuration-aware source line counter for Cargo projects.
It evaluates Cargo feature selections and Rust conditional-compilation
attributes before reporting first-party source, allowing questions such as:

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

Count all eligible packages, features, and package targets beneath the current
directory:

```sh
cargo loc
```

Count projects beneath another Root:

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

Select a compilation target or emit schema-version 1 JSON:

```sh
cargo loc --target wasm32-wasip1
cargo loc --json
```

Package, feature, and standard target selectors follow Cargo syntax. Run
`cargo loc --help` for the complete option list.

## Output

The default report is a deterministic terminal table:

```text
╭───────────┬──────────┬───────┬───────┬────────┬──────────┬──────┬──────╮
│ Package   ┆ Language ┆ Files ┆ Lines ┆ Blanks ┆ Comments ┆ Code ┆ Test │
╞═══════════╪══════════╪═══════╪═══════╪════════╪══════════╪══════╪══════╡
│ cargo-loc ┆ Rust     ┆    12 ┆  1842 ┆    307 ┆      214 ┆ 1321 ┆    0 │
│ Total     ┆ All      ┆    12 ┆  1842 ┆    307 ┆      214 ┆ 1321 ┆    0 │
╰───────────┴──────────┴───────┴───────┴────────┴──────────┴──────┴──────╯
```

Rows aggregate each selected Cargo package and language. `Code` and `Test` are
exclusive: source active in any production context is `Code`, while source
active only in test or benchmark contexts is `Test`. `Comments` may overlap
either category. The table is unstyled, contains no ANSI escape sequences, and
is safe to redirect to a file. JSON output includes normalized selection,
Project targets, context-specific features, Package identities, totals, and
warnings.

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
toolchain identity, target specifications, and symlink targets. Corrupt,
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
