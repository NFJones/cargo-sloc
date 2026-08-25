<div align="center">
<h1 align="center">Cargo SLoC</h1>
<p align="center">
  <a href="https://github.com/NFJones/cargo-sloc/stargazers"><img alt="GitHub stars" src="https://img.shields.io/github/stars/NFJones/cargo-sloc?style=flat-square"></a>
  <a href="https://github.com/NFJones/cargo-sloc/forks"><img alt="GitHub forks" src="https://img.shields.io/github/forks/NFJones/cargo-sloc?style=flat-square"></a>
  <a href="https://github.com/NFJones/cargo-sloc/issues"><img alt="GitHub issues" src="https://img.shields.io/github/issues/NFJones/cargo-sloc?style=flat-square"></a>
  <a href="https://github.com/NFJones/cargo-sloc/actions"><img alt="Build status" src="https://img.shields.io/github/actions/workflow/status/NFJones/cargo-sloc/ci.yml?style=flat-square"></a>
</p>
</div>
</div>

`cargo-sloc` is a source line counter for supported files beneath a directory.
It evaluates Cargo feature selections and Rust conditional-compilation
attributes for reachable package Rust, while retaining other recognized files
under their package or an explicit Root scope. It answers questions such as:

- How much production code remains when tests are excluded?
- How much source is active with a particular feature set?
- How do counts differ by Cargo package in a workspace?

The installed executable is named `cargo-sloc`, so Cargo invokes it as
`cargo sloc`.

## Installation

Install a published release with:

```sh
cargo install cargo-sloc
```

Install the current checkout with:

```sh
just install
cargo sloc --version
```

Rust and Cargo 1.95 or newer are required.

## Usage

Count all supported, non-ignored files plus every eligible Cargo package,
feature, and package target beneath the current directory:

```sh
cargo sloc
```

Count another Root, including directories with no Cargo manifest:

```sh
cargo sloc ../workspace
```

Select a Cargo feature configuration:

```sh
cargo sloc --no-default-features --features serde,simd
```

Select or exclude package targets and contexts:

```sh
cargo sloc --lib --bins --exclude-target test --exclude-target bench
```

Exclude files that resolve to the Root scope while retaining selected Package
rows:

```sh
cargo sloc --root-files exclude
```

Select a compilation target or emit schema-version 3 JSON:

```sh
cargo sloc --target wasm32-wasip1
cargo sloc --json
```

Package, feature, and standard target selectors follow Cargo syntax. Run
`cargo sloc --help` for the complete option list.

## What is counted

`cargo-sloc` counts supported, non-ignored source files below the requested
Root. It reports Cargo-owned files by package and files without a unique
selected-package owner as `<root>`. Rust reached from selected Cargo targets is
analyzed with Cargo features and Rust `cfg` attributes. Other Rust files and
other recognized languages use lexical accounting, so they do not claim Cargo
target, feature, cfg, import, or test provenance filtering.

The supported-language catalog is based on Tokei; Rust uses cargo-sloc's
configuration-aware accountant. See [SPEC.md](SPEC.md) for the full language,
ignore, ownership, and analysis rules.

## Output

The default report is a deterministic terminal table:

```text
╭──────────────┬─────────────────────┬───────┬────────┬────────┬────────┬──────────┬────────┬────────╮
│ Package      ┆ Language            ┆ Files ┆  Total ┆  Lines ┆ Blanks ┆ Comments ┆   Code ┆   Test │
╞══════════════╪═════════════════════╪═══════╪════════╪════════╪════════╪══════════╪════════╪════════╡
│ mezzanine    ┆ Rust                ┆   802 ┆ 412393 ┆ 412393 ┆  20351 ┆    47308 ┆ 182453 ┆ 162282 │
│              ┆ Rust (unconfigured) ┆     2 ┆    565 ┆    565 ┆     36 ┆       43 ┆    486 ┆    n/a │
│              ┆ Markdown            ┆    24 ┆    281 ┆    281 ┆     78 ┆      203 ┆      0 ┆    n/a │
│              ┆ TOML                ┆     1 ┆     55 ┆     55 ┆      4 ┆        0 ┆     51 ┆    n/a │
│ mez-agent    ┆ Rust                ┆   171 ┆  85770 ┆  85770 ┆   5291 ┆    13524 ┆  44689 ┆  22266 │
│              ┆ TOML                ┆     1 ┆     23 ┆     23 ┆      3 ┆        0 ┆     20 ┆    n/a │
│ mez-mux      ┆ Rust                ┆    70 ┆  42641 ┆  42641 ┆   2780 ┆     6631 ┆  23030 ┆  10200 │
│              ┆ TOML                ┆     1 ┆     26 ┆     26 ┆      4 ┆        0 ┆     22 ┆    n/a │
│ <root>       ┆ Markdown            ┆    54 ┆  16484 ┆  16484 ┆   2496 ┆    13921 ┆     67 ┆    n/a │
│              ┆ TOML                ┆     2 ┆    607 ┆    607 ┆     32 ┆      302 ┆    273 ┆    n/a │
│              ┆ Python              ┆     1 ┆    359 ┆    359 ┆     42 ┆        1 ┆    316 ┆    n/a │
│              ┆ YAML                ┆     1 ┆    130 ┆    130 ┆      0 ┆        0 ┆    130 ┆    n/a │
│              ┆ Just                ┆     1 ┆     74 ┆     74 ┆     16 ┆       23 ┆     35 ┆    n/a │
│              ┆ Shell               ┆     1 ┆     71 ┆     71 ┆     10 ┆        3 ┆     58 ┆    n/a │
│              ┆ JSON                ┆     1 ┆      1 ┆      1 ┆      0 ┆        0 ┆      1 ┆    n/a │
│ mez-terminal ┆ Rust                ┆    34 ┆  10374 ┆  10374 ┆    970 ┆     2429 ┆   4642 ┆   2345 │
│              ┆ TOML                ┆     1 ┆     12 ┆     12 ┆      2 ┆        0 ┆     10 ┆    n/a │
│ mez-core     ┆ Rust                ┆     2 ┆    362 ┆    362 ┆     28 ┆      165 ┆    130 ┆     39 │
│              ┆ TOML                ┆     1 ┆      8 ┆      8 ┆      1 ┆        0 ┆      7 ┆    n/a │
│ Total        ┆ All                 ┆  1171 ┆ 570236 ┆ 570236 ┆  32144 ┆    84553 ┆ 256420 ┆ 197132 │
╰──────────────┴─────────────────────┴───────┴────────┴────────┴────────┴──────────┴────────┴────────╯
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
