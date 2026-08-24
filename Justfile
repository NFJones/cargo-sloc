# Default recipe builds all targets and features in release mode.
default: build-release

# Build all targets and features in debug mode.
build:
    cargo build --all-targets --all-features

# Build all targets and features in release mode.
build-release:
    cargo build --all-targets --all-features --release

# Install or update cargo-loc from this checkout.
install:
    cargo install --path . --force

# Run the release binary with optional arguments.
run *args:
    RUST_BACKTRACE=1 cargo run --release -- {{args}}

# Type-check all targets and features.
check:
    cargo check --all-targets --all-features

# Apply standard Rust formatting.
fmt:
    cargo fmt --all

# Verify formatting without changing files.
fmt-check:
    cargo fmt --all --check

# Lint all targets and features, denying warnings.
clippy:
    cargo clippy --all-targets --all-features -- -D warnings

# Run all tests quietly without stopping after the first failing test binary.
test:
    cargo test --all-targets --all-features --no-fail-fast --quiet

# Run the complete local validation suite.
ci: fmt-check check clippy test

# Verify the crate can be packaged for publication.
package:
    cargo package

# Remove Cargo build artifacts.
clean:
    cargo clean

# List available recipes.
help:
    @just --list
