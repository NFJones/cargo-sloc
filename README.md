<div align="center">
<h1 align="center">Cargo LoC</h1>
<p align="center">
  <a href="https://github.com/NFJones/cargo-loc/stargazers"><img alt="GitHub stars" src="https://img.shields.io/github/stars/NFJones/cargo-loc?style=flat-square"></a>
  <a href="https://github.com/NFJones/cargo-loc/forks"><img alt="GitHub forks" src="https://img.shields.io/github/forks/NFJones/cargo-loc?style=flat-square"></a>
  <a href="https://github.com/NFJones/cargo-loc/issues"><img alt="GitHub issues" src="https://img.shields.io/github/issues/NFJones/cargo-loc?style=flat-square"></a>
  <a href="https://github.com/NFJones/cargo-loc/actions"><img alt="Build status" src="https://img.shields.io/github/actions/workflow/status/NFJones/cargo-loc/ci.yml?style=flat-square"></a>
</p>
</div>
</div>

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
cargo loc --version
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

## What is counted

`cargo-loc` counts supported, non-ignored source files below the requested
Root. It reports Cargo-owned files by package and files without a unique
selected-package owner as `<root>`. Rust reached from selected Cargo targets is
analyzed with Cargo features and Rust `cfg` attributes. Other Rust files and
other recognized languages use lexical accounting, so they do not claim Cargo
target, feature, cfg, import, or test provenance filtering.

The supported-language catalog is based on Tokei; Rust uses cargo-loc's
configuration-aware accountant. See [SPEC.md](SPEC.md) for the full language,
ignore, ownership, and analysis rules.

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

Rows aggregate a scope (a Cargo package or `<root>`) and language. The columns
mean:

- **Files:** unique source files in the row.
- **Total / Lines:** physical lines; `Total` is not a sum of the categories.
- **Blanks:** blank lines.
- **Comments:** lines containing comments; they can also contain code.
- **Code:** production code lines.
- **Test:** Rust lines active only for test or benchmark targets.

Lexically counted languages show `n/a` for **Test** because that provenance is
not available. The table's final Test total sums the rows with known Test
counts. JSON represents unavailable per-row Test counts as `null`. Output is
deterministic, unstyled, and safe to redirect; use `--json` for machine-readable
output.

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

See [SPEC.md](SPEC.md) for normative behavior, including the complete JSON
contract and detailed ignore, identity, cache, and scanner rules.

## Development

Install [`just`](https://github.com/casey/just), then use:

```sh
just ci
just bench-smoke
just package
just install-smoke
```

`just release-check` runs the complete local release-readiness suite. The
project is licensed under the Apache License, Version 2.0; see
[COPYING](COPYING). Release history is recorded in [CHANGELOG.md](CHANGELOG.md).

## References

- [Cargo external tools](https://doc.rust-lang.org/cargo/reference/external-tools.html)
- [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html)
- [Rust conditional compilation](https://doc.rust-lang.org/reference/conditional-compilation.html)
