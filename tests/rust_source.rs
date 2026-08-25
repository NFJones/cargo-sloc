//! Integration coverage for context-sensitive first-party Rust source discovery.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_sloc::cli::ParseOutcome;
use cargo_sloc::configuration::{ConfiguredInventory, resolve};
use cargo_sloc::discovery::discover;
use cargo_sloc::error::AppError;
use cargo_sloc::model::{ContextKind, Selection};
use cargo_sloc::rust_source::{SourceInventory, discover as discover_sources};
use tempfile::TempDir;

#[test]
fn discovers_only_context_reachable_modules_and_honors_active_paths() {
    let root = source_graph_fixture();
    let sources = source_inventory(root.path(), []);
    let package = &sources.packages[0];
    let paths = relative_paths(root.path(), package.files.iter().map(|source| &source.path));

    assert_eq!(
        paths,
        BTreeSet::from([
            "src/alternate.rs".to_owned(),
            "src/enabled.rs".to_owned(),
            "src/inline/child.rs".to_owned(),
            "src/lib.rs".to_owned(),
            "src/nested_style/mod.rs".to_owned(),
            "src/plain.rs".to_owned(),
            "src/plain/deep.rs".to_owned(),
            "src/shared.rs".to_owned(),
            "src/test_only.rs".to_owned(),
            "src/thread_files/tls.rs".to_owned(),
        ])
    );

    let test_only = package
        .files
        .iter()
        .find(|source| source.path.ends_with("test_only.rs"))
        .expect("test-only module");
    assert!(
        test_only
            .contexts
            .iter()
            .map(|context| package
                .semantic_context(*context)
                .expect("semantic context"))
            .all(|context| context.provenance() == ContextKind::Test)
    );
    assert_eq!(
        package
            .files
            .iter()
            .filter(|source| source.path.ends_with("shared.rs"))
            .count(),
        1,
        "one physical file reached through two module declarations is deduplicated"
    );
}

#[test]
fn feature_configuration_changes_module_reachability() {
    let root = source_graph_fixture();
    let sources = source_inventory(root.path(), ["--no-default-features"]);
    let paths = relative_paths(
        root.path(),
        sources.packages[0].files.iter().map(|source| &source.path),
    );

    assert!(paths.contains("src/disabled.rs"));
    assert!(paths.contains("src/redirected.rs"));
    assert!(!paths.contains("src/enabled.rs"));
    assert!(!paths.contains("src/alternate.rs"));
}

#[test]
fn production_discovery_ignores_modules_inside_test_and_bench_functions() {
    let root = simple_package(
        "harness-only-missing",
        r#"#[test]
fn test_case() {
    #[path = "missing_test_helper.rs"]
    mod helper;
}

#[cfg_attr(all(), bench)]
fn bench_case() {
    #[path = "missing_bench_helper.rs"]
    mod helper;
}
"#,
    );

    let sources = source_inventory(
        root.path(),
        ["--exclude-target", "test", "--exclude-target", "bench"],
    );
    assert_eq!(sources.packages[0].files.len(), 1);
}

#[test]
fn harness_discovery_reaches_test_and_bench_function_modules_as_test_only() {
    let root = simple_package(
        "harness-only-present",
        r#"#[test]
fn test_case() {
    #[path = "test_helper.rs"]
    mod helper;
}

#[cfg_attr(all(), bench)]
fn bench_case() {
    #[path = "bench_helper.rs"]
    mod helper;
}
"#,
    );
    write(
        root.path().join("src/test_helper.rs"),
        "pub fn test_helper() {}\n",
    );
    write(
        root.path().join("src/bench_helper.rs"),
        "pub fn bench_helper() {}\n",
    );

    let sources = source_inventory(root.path(), []);
    for helper in ["test_helper.rs", "bench_helper.rs"] {
        let source = sources.packages[0]
            .files
            .iter()
            .find(|source| source.path.ends_with(helper))
            .expect("harness-only helper");
        assert!(
            source
                .contexts
                .iter()
                .map(|context| {
                    sources.packages[0]
                        .semantic_context(*context)
                        .expect("semantic context")
                })
                .all(|context| { context.provenance() == ContextKind::Test && context.harness() })
        );
    }
}

#[test]
fn raw_identifiers_use_semantic_module_and_cfg_names() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"raw-identifiers\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\nalpha = []\n",
    );
    write(
        root.path().join("src/lib.rs"),
        r#"mod r#type;
mod r#async {
    mod r#match;
}
#[cfg(r#feature = "alpha")]
mod r#feature;
#[cfg_attr(all(), cfg(r#feature = "alpha"))]
mod nested;
#[cfg(r#test)]
mod bare;
#[path = "r#literal.rs"]
mod literal;
"#,
    );
    for path in [
        "src/type.rs",
        "src/async/match.rs",
        "src/bare.rs",
        "src/feature.rs",
        "src/nested.rs",
        "src/r#literal.rs",
    ] {
        write(root.path().join(path), "pub fn marker() {}\n");
    }

    let sources = source_inventory(root.path(), ["--features", "alpha"]);
    let paths = relative_paths(
        root.path(),
        sources.packages[0].files.iter().map(|source| &source.path),
    );
    assert_eq!(
        paths,
        BTreeSet::from([
            "src/async/match.rs".to_owned(),
            "src/bare.rs".to_owned(),
            "src/feature.rs".to_owned(),
            "src/lib.rs".to_owned(),
            "src/nested.rs".to_owned(),
            "src/r#literal.rs".to_owned(),
            "src/type.rs".to_owned(),
        ])
    );
}

#[test]
fn one_physical_source_shared_by_same_name_packages_is_warned() {
    let root = TempDir::new().expect("create Root");
    for directory in ["a", "b"] {
        write(
            root.path().join(directory).join("Cargo.toml"),
            "[package]\nname = \"duplicate\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\npath = \"../shared.rs\"\n",
        );
    }
    write(root.path().join("shared.rs"), "pub fn shared() {}\n");

    let sources = source_inventory(root.path(), []);
    assert_eq!(sources.packages.len(), 2);
    assert!(
        sources
            .packages
            .iter()
            .all(|package| package.files.len() == 1)
    );
    assert_eq!(sources.warnings.len(), 1);
    assert_eq!(sources.warnings[0].code, "source-shared-between-packages");
    assert!(sources.warnings[0].message.contains("a/Cargo.toml"));
    assert!(sources.warnings[0].message.contains("b/Cargo.toml"));
}

#[test]
fn one_physical_source_shared_by_targets_in_one_package_does_not_warn() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"one-owner\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\npath = \"shared.rs\"\n\n[[bin]]\nname = \"shared-bin\"\npath = \"shared.rs\"\n",
    );
    write(root.path().join("shared.rs"), "pub fn shared() {}\n");

    let sources = source_inventory(root.path(), []);
    assert_eq!(sources.packages.len(), 1);
    assert_eq!(sources.packages[0].files.len(), 1);
    assert!(sources.warnings.is_empty());
}

#[test]
fn missing_and_ambiguous_active_modules_are_errors() {
    let missing = simple_package("missing", "mod absent;\n");
    assert!(matches!(
        source_inventory_result(missing.path(), []),
        Err(AppError::ModuleNotFound { .. })
    ));

    let ambiguous = simple_package("ambiguous", "mod duplicate;\n");
    write(
        ambiguous.path().join("src/duplicate.rs"),
        "pub fn flat() {}\n",
    );
    write(
        ambiguous.path().join("src/duplicate/mod.rs"),
        "pub fn nested() {}\n",
    );
    assert!(matches!(
        source_inventory_result(ambiguous.path(), []),
        Err(AppError::AmbiguousModule { .. })
    ));
}

#[test]
fn parse_failures_leave_command_stdout_empty() {
    let root = simple_package("broken", "pub fn broken(\n");
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-sloc"))
        .arg(root.path())
        .output()
        .expect("run cargo-sloc");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to parse selected Rust source")
    );
}

#[test]
fn malformed_cfg_diagnostics_name_the_real_source_file() {
    for (name, source) in [
        ("malformed-module-cfg", "#[cfg(not())]\nmod helper;\n"),
        ("malformed-item-cfg", "#[cfg(not())]\npub fn item() {}\n"),
    ] {
        let root = simple_package(name, source);
        let output = Command::new(env!("CARGO_BIN_EXE_cargo-sloc"))
            .arg(root.path())
            .output()
            .expect("run cargo-sloc");

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("src/lib.rs"), "stderr: {stderr}");
        assert!(!stderr.contains("<syntax>"), "stderr: {stderr}");
    }
}

#[test]
fn unknown_cfgs_warn_while_recognized_inactive_cfgs_do_not() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"cfg-warnings\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\ndisabled = []\n",
    );
    write(
        root.path().join("src/lib.rs"),
        r#"#[cfg(mystery)]
pub fn hidden_name() {}
#[cfg(mystery)]
pub fn duplicate_hidden_name() {}
#[cfg(mystery = "value")]
pub fn hidden_value() {}
#[cfg(feature = "disabled")]
pub fn disabled_feature() {}
#[cfg(target_os = "windows")]
pub fn other_target() {}
"#,
    );

    let sources = source_inventory(root.path(), ["--no-default-features"]);
    let unknown: Vec<_> = sources
        .warnings
        .iter()
        .filter(|warning| warning.code == "unknown-cfg")
        .collect();
    assert_eq!(unknown.len(), 2);
    assert!(
        unknown
            .iter()
            .all(|warning| warning.message.contains("cfg-warnings"))
    );
    assert!(
        unknown
            .iter()
            .any(|warning| warning.message.contains("mystery"))
    );
    assert!(
        unknown
            .iter()
            .any(|warning| warning.message.contains("mystery = \"value\""))
    );
    assert!(
        unknown
            .iter()
            .all(|warning| !warning.message.contains("disabled"))
    );
    assert!(
        unknown
            .iter()
            .all(|warning| !warning.message.contains("target_os"))
    );
}

fn source_graph_fixture() -> TempDir {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"source-graph\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\nfeature-a = []\n",
    );
    write(
        root.path().join("src/lib.rs"),
        r#"mod plain;
mod nested_style;
#[cfg(feature = "feature-a")]
mod enabled;
#[cfg(not(feature = "feature-a"))]
mod disabled;
#[cfg(test)]
mod test_only;
#[cfg_attr(feature = "feature-a", path = "alternate.rs")]
mod redirected;
mod inline {
    mod child;
}
#[path = "thread_files"]
mod thread {
    #[path = "tls.rs"]
    mod local_data;
}
#[path = "shared.rs"]
mod first;
#[path = "shared.rs"]
mod second;
include!("included.rs");
macro_rules! declare_module {
    ($name:ident) => { mod $name; };
}
declare_module!(generated);
"#,
    );
    write(root.path().join("src/plain.rs"), "mod deep;\n");
    write(root.path().join("src/plain/deep.rs"), "pub fn deep() {}\n");
    write(
        root.path().join("src/nested_style/mod.rs"),
        "pub fn nested() {}\n",
    );
    for path in [
        "src/enabled.rs",
        "src/disabled.rs",
        "src/test_only.rs",
        "src/alternate.rs",
        "src/redirected.rs",
        "src/inline/child.rs",
        "src/thread_files/tls.rs",
        "src/shared.rs",
        "src/included.rs",
        "src/generated.rs",
        "src/unreferenced.rs",
    ] {
        write(root.path().join(path), "pub fn marker() {}\n");
    }
    root
}

fn simple_package(name: &str, source: &str) -> TempDir {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    );
    write(root.path().join("src/lib.rs"), source);
    root
}

fn source_inventory<const N: usize>(root: &Path, arguments: [&str; N]) -> SourceInventory {
    source_inventory_result(root, arguments).expect("discover Rust source graph")
}

fn source_inventory_result<const N: usize>(
    root: &Path,
    arguments: [&str; N],
) -> Result<SourceInventory, AppError> {
    let selection = selection(root, arguments)?;
    let inventory = discover(&selection)?;
    let configured: ConfiguredInventory = resolve(&selection, &inventory)?;
    discover_sources(&configured)
}

fn selection<const N: usize>(root: &Path, arguments: [&str; N]) -> Result<Selection, AppError> {
    let mut arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    arguments.push(root.as_os_str().to_owned());
    match cargo_sloc::cli::parse(arguments, Path::new(env!("CARGO_MANIFEST_DIR")))? {
        ParseOutcome::Selection(selection) => Ok(selection),
        ParseOutcome::EarlyExit { .. } => panic!("unexpected early CLI exit"),
    }
}

fn relative_paths<'a>(root: &Path, paths: impl Iterator<Item = &'a PathBuf>) -> BTreeSet<String> {
    let root = root.canonicalize().expect("canonical Root");
    paths
        .map(|path| {
            path.strip_prefix(&root)
                .expect("source beneath Root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture directory");
    fs::write(path, contents).expect("write fixture file");
}
