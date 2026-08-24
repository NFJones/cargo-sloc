# cargo-loc

`cargo-loc` is a planned Cargo subcommand for counting Rust source lines under
a selected conditional-compilation configuration. It is intended to answer
questions that text-only line counters cannot, such as how much first-party
source is active when a feature is disabled or when tests are excluded.

The project is presently a scaffold. The proposed scope and semantic boundaries
are recorded in [SPEC.md](SPEC.md).

## Installation

Once published, Cargo will install the executable with:

```sh
cargo install cargo-loc
```

Cargo discovers external subcommands by executable name, so an executable named
`cargo-loc` on `PATH` is invoked as:

```sh
cargo loc --no-default-features --features serde
```

For development from a checkout:

```sh
just install
cargo loc --help
```

## Development

Requirements:

- a current stable Rust toolchain;
- [`just`](https://github.com/casey/just) for the convenience recipes.

```sh
just check
just test
just build
```

The project is licensed under the Apache License, Version 2.0. See
[COPYING](COPYING).

## References

- [Cargo external tools](https://doc.rust-lang.org/cargo/reference/external-tools.html)
- [Rust conditional compilation](https://doc.rust-lang.org/reference/conditional-compilation.html)
- [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html)
