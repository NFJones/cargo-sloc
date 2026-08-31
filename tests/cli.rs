//! End-to-end coverage for the command shell and empty reports.

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const TABLE_EMPTY: &str = " Package ┆ Language ┆ Files ┆ Lines ┆ Blanks ┆ Comments ┆ Code ┆ Test \n\
═════════╪══════════╪═══════╪═══════╪════════╪══════════╪══════╪══════\n\
\x20Total   ┆ All      ┆     0 ┆     0 ┆      0 ┆        0 ┆    0 ┆    0 \n";

#[test]
fn direct_and_cargo_style_invocation_are_equivalent() {
    let root = tempfile::tempdir().expect("create Root");
    let direct = run(["--json", root.path().to_str().expect("UTF-8 Root")]);
    let cargo_style = run(["sloc", "--json", root.path().to_str().expect("UTF-8 Root")]);

    assert_success(&direct);
    assert_success(&cargo_style);
    assert_eq!(direct.stdout, cargo_style.stdout);
    assert_eq!(direct.stderr, cargo_style.stderr);
}

#[test]
fn default_root_produces_the_empty_table_contract() {
    let root = tempfile::tempdir().expect("create Root");
    let output = run_in(std::iter::empty::<&str>(), root.path());

    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 table"),
        TABLE_EMPTY
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn json_normalizes_repeated_feature_target_and_selector_options() {
    let root = tempfile::tempdir().expect("create Root");
    std::fs::create_dir_all(root.path().join("src")).expect("create source directory");
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"normalized\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\nlogging = []\nserde = []\nsimd = []\n\n[[bin]]\nname = \"worker\"\npath = \"src/main.rs\"\n",
    )
    .expect("write manifest");
    std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").expect("write binary source");
    let output = run([
        "--json",
        "--features",
        "simd,serde",
        "-F",
        "serde logging",
        "--target",
        "wasm32-wasip1",
        "--target",
        "x86_64-unknown-linux-gnu",
        "--bins",
        "--tests",
        "--exclude-target",
        "bench",
        root.path().to_str().expect("UTF-8 Root"),
    ]);

    assert_success(&output);
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("parse JSON report");
    let configuration = &value["configuration"];
    assert_eq!(configuration["all_features"], false);
    assert_eq!(
        configuration["features"],
        serde_json::json!(["logging", "serde", "simd"])
    );
    assert_eq!(
        configuration["requested_targets"],
        serde_json::json!(["wasm32-wasip1", "x86_64-unknown-linux-gnu"])
    );
    assert_eq!(
        configuration["target_includes"],
        serde_json::json!(["bins", "tests"])
    );
    assert_eq!(
        configuration["target_excludes"],
        serde_json::json!(["bench"])
    );
    let rows = value["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    let rust = rows
        .iter()
        .find(|package| package["language"] == "Rust")
        .expect("Rust row");
    let toml = rows
        .iter()
        .find(|package| package["language"] == "TOML")
        .expect("TOML row");
    assert_eq!(rust["scope"]["name"], "normalized");
    let manifest_path = root
        .path()
        .canonicalize()
        .expect("canonical Root")
        .join("Cargo.toml");
    assert_eq!(
        rust["scope"]["manifest_path"].as_str(),
        manifest_path.to_str()
    );
    assert_eq!(rust["files"], 1);
    assert_eq!(rust["lines"], 1);
    assert_eq!(rust["code"], 1);
    assert_eq!(toml["files"], 1);
    assert_eq!(toml["lines"], 13);
    assert_eq!(toml["code"], 11);
    assert!(toml["test"].is_null());
    assert_eq!(
        value["total"],
        serde_json::json!({
            "files": 2,
            "lines": 14,
            "blanks": 2,
            "comments": 0,
            "code": 12,
            "test": null
        })
    );
}

#[test]
fn explicit_empty_features_do_not_enable_implicit_all_features() {
    let root = tempfile::tempdir().expect("create Root");
    std::fs::create_dir_all(root.path().join("src")).expect("create source directory");
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"feature-modes\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\ndefault = [\"default-feature\"]\ndefault-feature = []\nextra = []\n",
    )
    .expect("write manifest");
    std::fs::write(root.path().join("src/lib.rs"), "pub fn library() {}\n")
        .expect("write library source");

    let cases = [
        (
            vec!["--json"],
            true,
            vec!["default", "default-feature", "extra"],
        ),
        (
            vec!["--json", "--features", ""],
            false,
            vec!["default", "default-feature"],
        ),
        (
            vec!["--json", "--no-default-features", "--features", ""],
            false,
            Vec::new(),
        ),
    ];

    for (mut arguments, all_features, expected_features) in cases {
        arguments.push(root.path().to_str().expect("UTF-8 Root"));
        let output = run(arguments);
        assert_success(&output);
        let report: Value = serde_json::from_slice(&output.stdout).expect("parse JSON report");
        assert_eq!(report["configuration"]["all_features"], all_features);
        assert_eq!(report["configuration"]["features"], serde_json::json!([]));
        assert!(
            report["configuration"]["feature_contexts"]
                .as_array()
                .is_some_and(|contexts| contexts.iter().all(|context| {
                    context["features"] == serde_json::json!(expected_features)
                }))
        );
    }
}

#[test]
fn help_and_version_exit_successfully_on_stdout() {
    let help = run(["--help"]);
    assert_success(&help);
    let help_text = String::from_utf8(help.stdout).expect("UTF-8 help");
    assert!(help_text.contains("Count supported, non-ignored source beneath PATH"));
    assert!(help_text.contains("By default, every eligible Package"));
    assert!(help_text.contains("all features, all package targets, and supported Root files"));
    assert!(help_text.contains("--root-files <ROOT_FILES>"));
    assert!(help_text.contains("--totals"));
    assert!(help_text.contains("cargo sloc --no-default-features --features serde"));
    assert!(help_text.contains("Emit schema-version 3 JSON instead of the terminal table"));
    assert!(help.stderr.is_empty());

    let version = run(["--version"]);
    assert_success(&version);
    assert_eq!(
        String::from_utf8(version.stdout).expect("UTF-8 version"),
        format!("cargo-sloc {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(version.stderr.is_empty());
}

#[test]
fn usage_errors_use_status_two_and_leave_stdout_empty() {
    for arguments in [
        vec!["--exclude", "missing"],
        vec!["--exclude-target", "build-script:named"],
        vec!["--json", "--totals"],
        vec!["--unknown-option"],
    ] {
        let output = run(arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn invalid_roots_use_status_one_and_leave_stdout_empty() {
    let parent = TempDir::new().expect("create parent");
    let missing = parent.path().join("missing");
    let output = run(["--json", missing.to_str().expect("UTF-8 missing path")]);

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid Root"));
}

#[test]
fn a_root_named_sloc_is_unambiguous_with_a_path_prefix() {
    let parent = TempDir::new().expect("create parent");
    std::fs::create_dir(parent.path().join("sloc")).expect("create sloc Root");
    let output = run_in(["./sloc"], parent.path());

    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 table"),
        TABLE_EMPTY
    );
}

fn run<I, S>(arguments: I) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    run_in(arguments, Path::new(env!("CARGO_MANIFEST_DIR")))
}

fn run_in<I, S>(arguments: I, current_directory: &Path) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(env!("CARGO_BIN_EXE_cargo-sloc"))
        .args(arguments)
        .current_dir(current_directory)
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
