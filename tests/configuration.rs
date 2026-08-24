//! Integration coverage for Project-local Build Context resolution.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_loc::cli::ParseOutcome;
use cargo_loc::configuration::{BuildContext, ConfiguredInventory};
use cargo_loc::discovery::{TargetKind, discover};
use cargo_loc::error::AppError;
use cargo_loc::model::{BuildRole, CfgOption, ContextKind, Selection};
use tempfile::TempDir;

#[test]
fn all_features_cfgs_and_test_provenance_are_context_specific() {
    let root = package_fixture("app", false, true);
    let configured = configure(root.path(), []);
    let project = &configured.projects[0];
    let package = &project.packages[0];

    let production = context(package.contexts.iter(), "app", ContextKind::Production);
    assert_eq!(production.role, BuildRole::Target);
    assert!(production.features.is_superset(&BTreeSet::from([
        "alpha".to_owned(),
        "beta".to_owned(),
        "default".to_owned(),
        "default-feature".to_owned(),
    ])));
    assert!(production.cfg_options.contains(&CfgOption::KeyValue {
        name: "feature".to_owned(),
        value: "alpha".to_owned(),
    }));
    assert!(
        !production
            .cfg_options
            .contains(&CfgOption::Name("test".to_owned()))
    );
    assert!(production.cfg_options.iter().any(|option| {
        matches!(option, CfgOption::KeyValue { name, .. } if name == "target_arch")
    }));
    assert_eq!(
        production.recognized_features,
        BTreeSet::from([
            "alpha".to_owned(),
            "beta".to_owned(),
            "default".to_owned(),
            "default-feature".to_owned(),
        ])
    );

    let test = context(package.contexts.iter(), "app", ContextKind::Test);
    assert!(
        test.cfg_options
            .contains(&CfgOption::Name("test".to_owned()))
    );

    let build_script = package
        .contexts
        .iter()
        .find(|context| {
            context.target_kind == TargetKind::BuildScript
                && context.provenance == ContextKind::Production
        })
        .expect("build-script production context");
    assert_eq!(build_script.role, BuildRole::Host);
    assert_eq!(build_script.compilation_target, project.host_target);
    assert!(build_script.features.contains("alpha"));
}

#[test]
fn explicit_targets_override_project_build_target_configuration() {
    let root = package_fixture("targets", false, false);
    write(
        root.path().join(".cargo/config.toml"),
        "[build]\ntarget = [\"wasm32-unknown-unknown\"]\n",
    );

    let configured = configure(root.path(), ["--target", "x86_64-unknown-linux-gnu"]);
    assert_eq!(configured.projects[0].targets, ["x86_64-unknown-linux-gnu"]);
    assert!(
        configured.projects[0].packages[0]
            .contexts
            .iter()
            .filter(|context| context.role == BuildRole::Target)
            .all(|context| context.compilation_target == "x86_64-unknown-linux-gnu")
    );
}

#[test]
fn proc_macro_contexts_are_host_built_and_deduplicated_across_targets() {
    let root = package_fixture("proc-macros", true, false);
    let configured = configure(
        root.path(),
        [
            "--target",
            "x86_64-unknown-linux-gnu",
            "--target",
            "wasm32-unknown-unknown",
        ],
    );
    let project = &configured.projects[0];
    let contexts = &project.packages[0].contexts;

    assert_eq!(
        contexts.len(),
        2,
        "equivalent test and bench contexts are deduplicated"
    );
    assert!(contexts.iter().all(|context| {
        context.role == BuildRole::Host && context.compilation_target == project.host_target
    }));
    assert!(contexts.iter().all(|context| {
        context
            .cfg_options
            .contains(&CfgOption::Name("proc_macro".to_owned()))
    }));
}

#[test]
fn feature_requests_are_forwarded_only_to_matching_projects() {
    let root = TempDir::new().expect("create Root");
    package_at(
        root.path().join("with-feature"),
        "with-feature",
        false,
        false,
    );
    package_at(
        root.path().join("without-feature"),
        "without-feature",
        false,
        false,
    );
    let manifest = root.path().join("without-feature/Cargo.toml");
    let contents = fs::read_to_string(&manifest).expect("read manifest");
    fs::write(&manifest, contents.replace("alpha = []\n", "")).expect("remove feature declaration");

    for requested in ["alpha", "with-feature/alpha"] {
        let configured = configure(root.path(), ["--features", requested]);
        assert_eq!(configured.projects.len(), 2);
        let with_feature = configured
            .projects
            .iter()
            .flat_map(|project| &project.packages)
            .find(|package| package.name == "with-feature")
            .expect("feature-owning package");
        assert!(
            with_feature
                .contexts
                .iter()
                .all(|context| context.features.contains("alpha"))
        );
        let without_feature = configured
            .projects
            .iter()
            .flat_map(|project| &project.packages)
            .find(|package| package.name == "without-feature")
            .expect("unrelated package");
        assert!(
            without_feature
                .contexts
                .iter()
                .all(|context| !context.features.contains("alpha"))
        );
    }
}

#[test]
fn harness_free_test_and_bench_targets_do_not_enable_cfg_test() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"manual-harnesses\"\nversion = \"0.1.0\"\nedition = \"2024\"\nautobins = false\n\n[[test]]\nname = \"manual-test\"\npath = \"tests/manual.rs\"\nharness = false\n\n[[bench]]\nname = \"manual-bench\"\npath = \"benches/manual.rs\"\nharness = false\n",
    );
    write(root.path().join("tests/manual.rs"), "fn main() {}\n");
    write(root.path().join("benches/manual.rs"), "fn main() {}\n");

    let configured = configure(root.path(), []);
    let contexts = &configured.projects[0].packages[0].contexts;
    for target_name in ["manual-test", "manual-bench"] {
        let context = context(contexts.iter(), target_name, ContextKind::Test);
        assert!(!context.harness);
        assert!(
            !context
                .cfg_options
                .contains(&CfgOption::Name("test".to_owned()))
        );
    }
}

#[test]
fn required_features_follow_selected_package_contexts() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"helper\"]\nresolver = \"2\"\n",
    );
    write(
        root.path().join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\ngated = []\n\n[[bin]]\nname = \"worker\"\npath = \"src/main.rs\"\nrequired-features = [\"gated\"]\n",
    );
    write(root.path().join("app/src/lib.rs"), "pub fn app() {}\n");
    write(root.path().join("app/src/main.rs"), "fn main() {}\n");
    write(
        root.path().join("helper/Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\napp = { path = \"../app\", features = [\"gated\"] }\n",
    );
    write(
        root.path().join("helper/src/lib.rs"),
        "pub fn helper() {}\n",
    );

    let configured = configure(root.path(), ["--package", "app", "--no-default-features"]);
    let package = &configured.projects[0].packages[0];
    assert_eq!(package.name, "app");
    assert!(package.targets.iter().all(|target| target.name != "worker"));
    assert!(
        package
            .contexts
            .iter()
            .all(|context| context.target_name != "worker")
    );

    let error = configure_result(
        root.path(),
        [
            "--package",
            "app",
            "--no-default-features",
            "--bin",
            "worker",
        ],
    )
    .expect_err("named ineligible target must fail");
    assert!(matches!(error, AppError::IneligibleNamedTarget { .. }));
}

#[test]
fn ordinary_library_host_dependency_contexts_are_not_test_harnesses() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\", \"shared\"]\nresolver = \"2\"\n",
    );
    write(
        root.path().join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n\n[dependencies]\nshared = { path = \"../shared\", features = [\"normal\"] }\n\n[build-dependencies]\nshared = { path = \"../shared\", features = [\"host\"] }\n",
    );
    write(root.path().join("app/src/lib.rs"), "pub fn app() {}\n");
    write(root.path().join("app/build.rs"), "fn main() {}\n");
    write(
        root.path().join("shared/Cargo.toml"),
        "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nhost = []\nnormal = []\n",
    );
    write(
        root.path().join("shared/src/lib.rs"),
        "pub fn shared() {}\n",
    );

    let configured = configure(root.path(), ["--no-default-features"]);
    let shared = configured.projects[0]
        .packages
        .iter()
        .find(|package| package.name == "shared")
        .expect("shared package");
    assert!(shared.contexts.iter().any(|context| {
        context.role == BuildRole::Host
            && context.provenance == ContextKind::Production
            && context.features.contains("host")
    }));
    assert!(shared.contexts.iter().any(|context| {
        context.role == BuildRole::Target
            && context.provenance == ContextKind::Test
            && context.features.contains("normal")
    }));
    assert!(
        shared
            .contexts
            .iter()
            .all(|context| !(context.role == BuildRole::Host
                && context.provenance == ContextKind::Test)),
        "ordinary host dependency units must not be crossed with test provenance"
    );
}

#[test]
fn invalid_compilation_targets_fail_without_approximating_cfgs() {
    let root = package_fixture("invalid-target", false, false);
    let error = configure_result(root.path(), ["--target", "not-a-real-rust-target"])
        .expect_err("invalid target must fail");
    assert!(matches!(
        error,
        AppError::FeatureResolution { .. } | AppError::RustcQuery { .. }
    ));
}

#[test]
fn command_json_reports_resolved_project_targets() {
    let root = package_fixture("reported-targets", false, false);
    let host = host_target();
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-loc"))
        .args(["--json", "--target", &host])
        .arg(root.path())
        .output()
        .expect("run cargo-loc");

    assert!(
        output.status.success(),
        "cargo-loc failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse JSON report");
    assert_eq!(
        report["configuration"]["host_targets"],
        serde_json::json!([host])
    );
    assert_eq!(
        report["configuration"]["targets"],
        serde_json::json!([host])
    );
    assert_eq!(
        report["configuration"]["project_targets"],
        serde_json::json!([{
            "project_root": root.path().canonicalize().expect("canonical Root"),
            "host_target": host,
            "targets": [host]
        }])
    );
}

#[test]
fn command_target_resolution_failures_leave_stdout_empty() {
    let root = package_fixture("failed-target", false, false);
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-loc"))
        .args(["--target", "not-a-real-rust-target"])
        .arg(root.path())
        .output()
        .expect("run cargo-loc");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not-a-real-rust-target"));
}

#[test]
fn target_specific_and_environment_rustflags_are_reported_as_unmodeled() {
    let root = package_fixture("rustflags", false, false);
    let host = host_target();
    write(
        root.path().join(".cargo/config.toml"),
        &format!("[target.{host}]\nrustflags = [\"--cfg\", \"configured\"]\n"),
    );
    let configured = configure(root.path(), []);
    assert!(
        configured
            .warnings
            .iter()
            .any(|warning| warning.code == "unmodeled-rustflags")
    );

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-loc"))
        .arg("--json")
        .arg(root.path())
        .env("RUSTFLAGS", "--cfg environment")
        .output()
        .expect("run cargo-loc with RUSTFLAGS");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse JSON report");
    assert!(report["warnings"].as_array().is_some_and(|warnings| {
        warnings
            .iter()
            .any(|warning| warning["code"] == "unmodeled-rustflags")
    }));
}

fn context<'a>(
    mut contexts: impl Iterator<Item = &'a BuildContext>,
    target_name: &str,
    provenance: ContextKind,
) -> &'a BuildContext {
    contexts
        .find(|context| {
            context.target_name == target_name
                && context.provenance == provenance
                && context.role == BuildRole::Target
        })
        .expect("requested Build Context")
}

fn configure<const N: usize>(root: &Path, arguments: [&str; N]) -> ConfiguredInventory {
    configure_result(root, arguments).expect("configure fixture")
}

fn configure_result<const N: usize>(
    root: &Path,
    arguments: [&str; N],
) -> Result<ConfiguredInventory, AppError> {
    let selection = selection(root, arguments)?;
    let inventory = discover(&selection)?;
    cargo_loc::configuration::resolve(&selection, &inventory)
}

fn selection<const N: usize>(root: &Path, arguments: [&str; N]) -> Result<Selection, AppError> {
    let mut arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    arguments.push(root.as_os_str().to_owned());
    match cargo_loc::cli::parse(arguments, Path::new(env!("CARGO_MANIFEST_DIR")))? {
        ParseOutcome::Selection(selection) => Ok(selection),
        ParseOutcome::EarlyExit { .. } => panic!("unexpected early CLI exit"),
    }
}

fn package_fixture(name: &str, proc_macro: bool, build_script: bool) -> TempDir {
    let root = TempDir::new().expect("create Root");
    package_at(root.path().to_path_buf(), name, proc_macro, build_script);
    root
}

fn package_at(root: PathBuf, name: &str, proc_macro: bool, build_script: bool) {
    let mut manifest =
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
    if build_script {
        manifest.push_str("build = \"build.rs\"\n");
    }
    if proc_macro {
        manifest.push_str("\n[lib]\nproc-macro = true\n");
    }
    manifest.push_str(
        "\n[features]\ndefault = [\"default-feature\"]\ndefault-feature = []\nalpha = []\nbeta = []\n",
    );
    write(root.join("Cargo.toml"), &manifest);
    let source = if proc_macro {
        "extern crate proc_macro;\n#[proc_macro] pub fn generated(_: proc_macro::TokenStream) -> proc_macro::TokenStream { \"\".parse().unwrap() }\n"
    } else {
        "pub fn library() {}\n"
    };
    write(root.join("src/lib.rs"), source);
    if build_script {
        write(root.join("build.rs"), "fn main() {}\n");
    }
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture directory");
    fs::write(path, contents).expect("write fixture file");
}

fn host_target() -> String {
    let output = Command::new(OsStr::new("rustc"))
        .arg("-vV")
        .output()
        .expect("run rustc -vV");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("UTF-8 rustc output")
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc host line")
        .to_owned()
}

#[test]
fn test_helper_observes_the_active_host() {
    assert!(!host_target().is_empty());
}
