//! End-to-end golden coverage for terminal tables, JSON, warnings, and report failures.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

#[test]
fn table_renders_package_counts_and_total_exactly() {
    let root = package("ordinary", "// note\n\npub fn one() {}\n");
    let output = run(root.path(), std::iter::empty::<&str>());

    assert_success(&output);
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 table"),
        "╭──────────┬──────────┬───────┬───────┬────────┬──────────┬──────┬──────╮\n\
         │ Package  ┆ Language ┆ Files ┆ Lines ┆ Blanks ┆ Comments ┆ Code ┆ Test │\n\
         ╞══════════╪══════════╪═══════╪═══════╪════════╪══════════╪══════╪══════╡\n\
         │ ordinary ┆ Rust     ┆     1 ┆     3 ┆      1 ┆        1 ┆    1 ┆    0 │\n\
         │ Total    ┆ All      ┆     1 ┆     3 ┆      1 ┆        1 ┆    1 ┆    0 │\n\
         ╰──────────┴──────────┴───────┴───────┴────────┴──────────┴──────┴──────╯\n"
    );
}

#[test]
fn duplicate_package_names_receive_stable_table_qualifiers() {
    let root = TempDir::new().expect("create Root");
    package_at(root.path().join("a|pipe"), "duplicate", "pub fn a() {}\n");
    package_at(
        root.path().join("b界\\slash"),
        "duplicate",
        "pub fn b() {}\n",
    );

    let first = run(root.path(), std::iter::empty::<&str>());
    let second = run(root.path(), std::iter::empty::<&str>());
    assert_success(&first);
    assert_success(&second);
    assert_eq!(first.stdout, second.stdout, "table must be deterministic");
    assert_eq!(first.stderr, second.stderr);

    let table = String::from_utf8(first.stdout).expect("UTF-8 table");
    assert!(table.contains("duplicate (a|pipe)"));
    assert!(table.contains("duplicate (b界\\slash)"));
    assert!(table.contains("│ Total"));
    assert!(table.ends_with("╯\n"));
}

#[cfg(unix)]
#[test]
fn table_escapes_terminal_controls_from_package_qualifiers() {
    let root = TempDir::new().expect("create Root");
    package_at(root.path().join("ordinary"), "duplicate", "pub fn a() {}\n");
    package_at(
        root.path().join("ansi-\u{001b}[31m"),
        "duplicate",
        "pub fn b() {}\n",
    );

    let output = run(root.path(), std::iter::empty::<&str>());
    assert_success(&output);
    assert!(!output.stdout.contains(&0x1b));
    assert!(String::from_utf8_lossy(&output.stdout).contains(r"duplicate (ansi-\u{001B}[31m)"));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains("duplicate (ansi-"))
            .count(),
        1
    );
}

#[test]
fn json_contains_typed_package_and_context_provenance() {
    let root = package_with_features("json-package", "pub fn counted() {}\n");
    let output = run(root.path(), ["--json", "--features", "alpha"]);

    assert_success(&output);
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).expect("parse JSON report");
    assert_eq!(report["schema_version"], 1);
    let packages = report["packages"].as_array().expect("packages array");
    assert_eq!(packages.len(), 1);
    let package = &packages[0];
    assert_eq!(package["name"], "json-package");
    assert_eq!(package["language"], "Rust");
    assert!(
        package["package_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert_eq!(package["project_root"].as_str(), root.path().to_str());
    assert_eq!(
        package["manifest_path"].as_str(),
        root.path().join("Cargo.toml").to_str()
    );
    assert_eq!(package["files"], 1);
    assert_eq!(package["lines"], 1);
    assert_eq!(package["code"], 1);

    let contexts = report["configuration"]["feature_contexts"]
        .as_array()
        .expect("feature contexts");
    assert!(!contexts.is_empty());
    assert!(contexts.iter().all(|context| {
        context["package_name"] == "json-package"
            && context["features"]
                .as_array()
                .is_some_and(|features| features.iter().any(|feature| feature == "alpha"))
    }));
    assert_eq!(report["total"], package_counts(package));
}

#[test]
fn warnings_are_stable_on_stderr_and_in_json() {
    let root = package("warnings", "pub fn counted() {}\n");
    let table = run(root.path(), ["--exclude-target", "example:missing"]);
    assert_success(&table);
    assert_eq!(
        String::from_utf8(table.stderr).expect("UTF-8 warning"),
        "warning[unmatched-target-exclusion]: target exclusion `example:missing` matched nothing\n"
    );

    let json = run(
        root.path(),
        ["--json", "--exclude-target", "example:missing"],
    );
    assert_success(&json);
    assert!(json.stderr.is_empty());
    let report: Value = serde_json::from_slice(&json.stdout).expect("parse JSON report");
    assert_eq!(
        report["warnings"],
        serde_json::json!([{
            "code": "unmatched-target-exclusion",
            "message": "target exclusion `example:missing` matched nothing"
        }])
    );
}

#[test]
fn valid_features_survive_an_empty_target_selection() {
    let root = package_with_features("empty-targets", "pub fn counted() {}\n");

    let table = run(
        root.path(),
        ["--features", "alpha", "--exclude-target", "lib"],
    );
    assert_success(&table);
    assert_eq!(
        String::from_utf8(table.stdout).expect("UTF-8 table"),
        "╭─────────┬──────────┬───────┬───────┬────────┬──────────┬──────┬──────╮\n\
         │ Package ┆ Language ┆ Files ┆ Lines ┆ Blanks ┆ Comments ┆ Code ┆ Test │\n\
         ╞═════════╪══════════╪═══════╪═══════╪════════╪══════════╪══════╪══════╡\n\
         │ Total   ┆ All      ┆     0 ┆     0 ┆      0 ┆        0 ┆    0 ┆    0 │\n\
         ╰─────────┴──────────┴───────┴───────┴────────┴──────────┴──────┴──────╯\n"
    );

    let json = run(
        root.path(),
        ["--json", "--features", "alpha", "--exclude-target", "lib"],
    );
    assert_success(&json);
    let report: Value = serde_json::from_slice(&json.stdout).expect("parse JSON report");
    assert_eq!(report["packages"], serde_json::json!([]));
    assert_eq!(
        report["configuration"]["features"],
        serde_json::json!(["alpha"])
    );
    assert_eq!(report["configuration"]["all_features"], false);
    assert_eq!(
        report["total"],
        serde_json::json!({
            "files": 0,
            "lines": 0,
            "blanks": 0,
            "comments": 0,
            "code": 0,
            "test": 0
        })
    );

    let unknown = run(
        root.path(),
        ["--features", "missing", "--exclude-target", "lib"],
    );
    assert!(!unknown.status.success());
    assert!(unknown.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("feature `missing`"));
}

#[cfg(unix)]
#[test]
fn non_utf8_json_root_fails_without_partial_stdout() {
    use std::os::unix::ffi::OsStringExt;

    let parent = TempDir::new().expect("create parent");
    let root = parent
        .path()
        .join(OsString::from_vec(b"non-utf8-\xff".to_vec()));
    fs::create_dir(&root).expect("create non-UTF-8 Root");

    let output = run(&root, ["--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("losslessly in JSON"));
}

fn package(name: &str, source: &str) -> TempDir {
    let root = TempDir::new().expect("create Root");
    package_at(root.path().to_path_buf(), name, source);
    root
}

fn package_with_features(name: &str, source: &str) -> TempDir {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        &format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\nalpha = []\n"
        ),
    );
    write(root.path().join("src/lib.rs"), source);
    root
}

fn package_at(root: PathBuf, name: &str, source: &str) {
    write(
        root.join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    );
    write(root.join("src/lib.rs"), source);
}

fn package_counts(package: &Value) -> Value {
    serde_json::json!({
        "files": package["files"],
        "lines": package["lines"],
        "blanks": package["blanks"],
        "comments": package["comments"],
        "code": package["code"],
        "test": package["test"]
    })
}

fn run<I, S>(root: &Path, arguments: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_cargo-loc"))
        .args(arguments)
        .arg(root)
        .output()
        .expect("run cargo-loc")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "cargo-loc failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture directory");
    fs::write(path, contents).expect("write fixture file");
}
