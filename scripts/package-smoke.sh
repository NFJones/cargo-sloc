#!/usr/bin/env bash

set -euo pipefail

allow_dirty=()
if [[ ${1-} == "--allow-dirty" ]]; then
    allow_dirty=(--allow-dirty)
elif [[ $# -ne 0 ]]; then
    printf 'usage: %s [--allow-dirty]\n' "$0" >&2
    exit 2
fi

package_name="$(sed -n 's/^name = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
package_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
archive="target/package/${package_name}-${package_version}.crate"

cargo package --locked "${allow_dirty[@]}"
[[ -f "$archive" ]] || {
    printf 'expected package archive was not created: %s\n' "$archive" >&2
    exit 1
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
contents="$work/package-files.txt"

tar -tzf "$archive" \
    | sed 's|^[^/]*/||' \
    | sed '/^$/d' \
    | LC_ALL=C sort -u > "$contents"

while IFS= read -r path; do
    case "$path" in
        .cargo_vcs_info.json|.gitignore|AGENTS.md|CHANGELOG.md|COPYING|Cargo.lock|Cargo.toml|Cargo.toml.orig|Justfile|README.md|SPEC.md) ;;
        benches/pipeline.rs|benches/support/mod.rs) ;;
        docs/PERFORMANCE.md|docs/TRACEABILITY.md|docs/design/0001-analysis-feasibility.md) ;;
        src/accountant.rs|src/app.rs|src/cli.rs|src/configuration.rs|src/discovery.rs|src/error.rs|src/lib.rs|src/main.rs|src/model.rs|src/process.rs|src/report.rs|src/rust_accounting.rs|src/rust_source.rs) ;;
        tests/cargo_contexts.rs|tests/cfg_spans.rs|tests/cli.rs|tests/configuration.rs|tests/discovery.rs|tests/performance.rs|tests/reporting.rs|tests/rust_accounting.rs|tests/rust_source.rs|tests/support/mod.rs|tests/traceability.rs) ;;
        tests/fixtures/cfg-spans/all_positions.rs) ;;
        tests/fixtures/cargo-context/resolver-v1/*|tests/fixtures/cargo-context/resolver-v2/*|tests/fixtures/cargo-context/resolver-v3/*) ;;
        *)
            printf 'unexpected file in package archive: %s\n' "$path" >&2
            exit 1
            ;;
    esac
done < "$contents"

for required in \
    COPYING \
    README.md \
    SPEC.md \
    src/main.rs \
    src/process.rs \
    tests/fixtures/cfg-spans/all_positions.rs \
    tests/fixtures/cargo-context/resolver-v1/app/src/lib.rs \
    tests/fixtures/cargo-context/resolver-v2/app/src/lib.rs \
    tests/fixtures/cargo-context/resolver-v3/app/src/lib.rs
do
    grep -Fqx "$required" "$contents" || {
        printf 'required file missing from package archive: %s\n' "$required" >&2
        exit 1
    }
done

tar -xzf "$archive" -C "$work"
source_root="$work/${package_name}-${package_version}"
install_root="$work/install"
cargo_home="$work/cargo-home"
target_dir="$work/target"
mkdir -p "$install_root" "$cargo_home" "$target_dir"

CARGO_HOME="$cargo_home" CARGO_TARGET_DIR="$target_dir" \
    cargo install --path "$source_root" --locked --root "$install_root"

PATH="$install_root/bin:$PATH" cargo-loc --help >/dev/null
PATH="$install_root/bin:$PATH" cargo loc --help >/dev/null

fixture="$work/fixture"
mkdir -p "$fixture/src"
cat > "$fixture/Cargo.toml" <<'EOF'
[package]
name = "packaged-smoke"
version = "0.0.0"
edition = "2024"
EOF
cat > "$fixture/src/lib.rs" <<'EOF'
// packaged smoke

pub fn smoke() {}
EOF

"$install_root/bin/cargo-loc" "$fixture" > "$work/report.md"
grep -Fq '| packaged-smoke | Rust | 1 | 3 | 1 | 1 | 1 | 0 |' "$work/report.md"

"$install_root/bin/cargo-loc" --json "$fixture" > "$work/report.json"
grep -Fq '"name": "packaged-smoke"' "$work/report.json"
grep -Fq '"files": 1' "$work/report.json"
grep -Fq '"code": 1' "$work/report.json"

printf 'validated packaged artifact: %s\n' "$archive"
