//! End-to-end golden coverage for terminal tables, JSON, warnings, and report failures.

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
        " Package  ┆ Language ┆ Files ┆ Total ┆ Lines ┆ Blanks ┆ Comments ┆ Code ┆ Test \n\
         ══════════╪══════════╪═══════╪═══════╪═══════╪════════╪══════════╪══════╪══════\n\
         \x20ordinary ┆ TOML     ┆     1 ┆     4 ┆     4 ┆      0 ┆        0 ┆    4 ┆  n/a \n\
         ╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌┼╌╌╌╌╌╌\n\
         \x20         ┆ Rust     ┆     1 ┆     3 ┆     3 ┆      1 ┆        1 ┆    1 ┆    0 \n\
         ╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌┼╌╌╌╌╌╌\n\
         \x20Total    ┆ All      ┆     2 ┆     7 ┆     7 ┆      1 ┆        1 ┆    5 ┆    0 \n"
    );
}

#[test]
fn root_only_reports_use_one_structural_root_scope() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("standalone.rs"),
        "// note\nfn standalone() {}\n",
    );
    write(root.path().join("tool.py"), "print('root')\n");

    let json = run(root.path(), ["--json"]);
    assert_success(&json);
    let report: Value = serde_json::from_slice(&json.stdout).expect("parse Root JSON report");
    assert_eq!(report["schema_version"], 3);
    assert_eq!(report["configuration"]["root_files"], "include");
    let rows = report["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| { row["scope"] == serde_json::json!({"kind": "root", "path": "."}) })
    );
    let rust = rows
        .iter()
        .find(|row| row["language"] == "Rust (unconfigured)")
        .expect("unconfigured Rust row");
    assert_eq!(rust["accounting_engine"], "rust");
    assert_eq!(rust["accounting_precision"], "unconfigured");
    assert_eq!(rust["test"], Value::Null);

    let table = run(root.path(), std::iter::empty::<&str>());
    assert_success(&table);
    let table = String::from_utf8(table.stdout).expect("UTF-8 Root table");
    assert_eq!(table.matches("<root>").count(), 1);
    assert!(table.contains("Rust (unconfigured)"));
    assert!(table.contains("Python"));
}

#[test]
fn mixed_package_and_root_scopes_are_both_visible_and_filterable() {
    let root = TempDir::new().expect("create Root");
    package_at(root.path().join("member"), "member", "pub fn member() {}\n");
    write(root.path().join("tool.py"), "print('root')\n");

    let included = run(root.path(), ["--json"]);
    assert_success(&included);
    let report: Value =
        serde_json::from_slice(&included.stdout).expect("parse mixed Scope JSON report");
    let rows = report["rows"].as_array().expect("rows array");
    assert!(rows.iter().any(|row| row["scope"]["kind"] == "package"));
    assert!(rows.iter().any(|row| row["scope"]["kind"] == "root"));

    let excluded = run(root.path(), ["--json", "--root-files", "exclude"]);
    assert_success(&excluded);
    let report: Value =
        serde_json::from_slice(&excluded.stdout).expect("parse filtered Scope JSON report");
    assert_eq!(report["configuration"]["root_files"], "exclude");
    assert!(
        report["rows"]
            .as_array()
            .expect("rows array")
            .iter()
            .all(|row| row["scope"]["kind"] == "package")
    );
}

#[test]
fn mixed_language_reports_merge_rust_extensions_and_shebang_sources() {
    let root = package("mixed", "pub fn rust() {}\n");
    write(
        root.path().join("web/app.js"),
        "// note\nconst value = 1;\n",
    );
    write(
        root.path().join("tool"),
        "#!/usr/bin/env python3\nprint('ok')\n",
    );

    let first = run(root.path(), ["--json"]);
    let second = run(root.path(), ["--json"]);
    assert_success(&first);
    assert_success(&second);
    assert_eq!(first.stdout, second.stdout, "JSON must be deterministic");

    let report: Value = serde_json::from_slice(&first.stdout).expect("parse mixed JSON report");
    let packages = report["rows"].as_array().expect("rows array");
    assert_eq!(packages.len(), 4);
    assert_eq!(
        packages
            .iter()
            .map(|row| row["language"].as_str().expect("language"))
            .collect::<Vec<_>>(),
        ["TOML", "JavaScript", "Python", "Rust"]
    );
    assert_eq!(packages[0]["accounting_precision"], "lexical");
    assert_eq!(packages[0]["test"], Value::Null);
    assert_eq!(packages[1]["accounting_precision"], "lexical");
    assert_eq!(packages[1]["test"], Value::Null);
    assert_eq!(packages[2]["accounting_precision"], "lexical");
    assert_eq!(packages[2]["test"], Value::Null);
    assert_eq!(packages[3]["accounting_precision"], "configuration-aware");
    assert_eq!(packages[3]["test"], 0);
    assert_eq!(report["total"]["files"], 4);
    assert_eq!(report["total"]["lines"], 9);
    assert_eq!(report["total"]["test"], Value::Null);

    let table = run(root.path(), std::iter::empty::<&str>());
    assert_success(&table);
    let table = String::from_utf8(table.stdout).expect("UTF-8 mixed table");
    assert!(table.contains(" mixed   ┆ TOML       "));
    assert!(table.contains("         ┆ JavaScript "));
    assert!(table.contains("         ┆ Python     "));
    assert!(table.contains("         ┆ Rust       "));
    assert_eq!(
        table.lines().filter(|line| line.ends_with(" n/a ")).count(),
        3,
        "only lexical rows have unavailable Test counts"
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
    assert!(table.contains(" Total"));
    assert!(!table.contains('│'));
    assert!(table.ends_with("\n"));
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
    assert_eq!(report["schema_version"], 3);
    let packages = report["rows"].as_array().expect("rows array");
    assert_eq!(packages.len(), 2);
    let package = packages
        .iter()
        .find(|package| package["language"] == "Rust")
        .expect("Rust row");
    let manifest = packages
        .iter()
        .find(|package| package["language"] == "TOML")
        .expect("TOML row");
    assert_eq!(package["scope"]["kind"], "package");
    assert_eq!(package["scope"]["name"], "json-package");
    assert_eq!(package["language"], "Rust");
    assert_eq!(package["accounting_engine"], "rust");
    assert_eq!(package["accounting_precision"], "configuration-aware");
    assert!(
        package["scope"]["package_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    let project_root = root.path().canonicalize().expect("canonical Root");
    let manifest_path = project_root.join("Cargo.toml");
    assert_eq!(
        package["scope"]["project_root"].as_str(),
        project_root.to_str()
    );
    assert_eq!(
        package["scope"]["manifest_path"].as_str(),
        manifest_path.to_str()
    );
    assert_eq!(package["files"], 1);
    assert_eq!(package["lines"], 1);
    assert_eq!(package["code"], 1);
    assert_eq!(manifest["files"], 1);
    assert_eq!(manifest["lines"], 7);
    assert_eq!(manifest["code"], 6);
    assert_eq!(manifest["test"], Value::Null);

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
    assert_eq!(
        report["total"],
        serde_json::json!({
            "files": 2,
            "lines": 8,
            "blanks": 1,
            "comments": 0,
            "code": 7,
            "test": null
        })
    );
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
        [
            "--features",
            "alpha",
            "--exclude-target",
            "lib",
            "--root-files",
            "exclude",
        ],
    );
    assert_success(&table);
    assert_eq!(
        String::from_utf8(table.stdout).expect("UTF-8 table"),
        " Package ┆ Language ┆ Files ┆ Total ┆ Lines ┆ Blanks ┆ Comments ┆ Code ┆ Test \n\
         ═════════╪══════════╪═══════╪═══════╪═══════╪════════╪══════════╪══════╪══════\n\
         \x20Total   ┆ All      ┆     0 ┆     0 ┆     0 ┆      0 ┆        0 ┆    0 ┆    0 \n"
    );

    let json = run(
        root.path(),
        [
            "--json",
            "--features",
            "alpha",
            "--exclude-target",
            "lib",
            "--root-files",
            "exclude",
        ],
    );
    assert_success(&json);
    let report: Value = serde_json::from_slice(&json.stdout).expect("parse JSON report");
    assert_eq!(report["rows"], serde_json::json!([]));
    assert_eq!(
        report["configuration"]["features"],
        serde_json::json!(["alpha"])
    );
    assert_eq!(report["configuration"]["all_features"], false);
    assert_eq!(report["configuration"]["root_files"], "exclude");
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
        [
            "--features",
            "missing",
            "--exclude-target",
            "lib",
            "--root-files",
            "exclude",
        ],
    );
    assert!(!unknown.status.success());
    assert!(unknown.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("feature `missing`"));
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_json_root_fails_without_partial_stdout() {
    use std::ffi::OsString;
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

fn run<I, S>(root: &Path, arguments: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_cargo-sloc"))
        .args(arguments)
        .arg(root)
        .output()
        .expect("run cargo-sloc")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "cargo-sloc failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture directory");
    fs::write(path, contents).expect("write fixture file");
}
