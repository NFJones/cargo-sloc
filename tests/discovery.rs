//! Integration coverage for Root-bounded Cargo Project and target inventory.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_sloc::cli::ParseOutcome;
use cargo_sloc::discovery::{Inventory, TargetContext, TargetKind};
use cargo_sloc::error::AppError;
use tempfile::TempDir;

#[test]
fn discovers_workspaces_and_standalone_projects_with_root_bounded_ignores() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join(".gitignore"),
        "ignored-standalone/\nworkspace/ignored-member/\n",
    );
    write(
        root.path().join("workspace/Cargo.toml"),
        "[workspace]\nmembers = [\"member\", \"ignored-member\"]\nresolver = \"3\"\n",
    );
    package(root.path().join("workspace/member"), "member");
    package(
        root.path().join("workspace/ignored-member"),
        "ignored-member",
    );
    package(root.path().join("standalone"), "standalone");
    write(
        root.path().join("ignored-standalone/Cargo.toml"),
        "this is deliberately malformed",
    );
    write(
        root.path().join("target/Cargo.toml"),
        "this is deliberately malformed",
    );

    let inventory = discover(root.path(), []);
    assert_eq!(inventory.projects.len(), 2);
    assert_eq!(
        package_names(&inventory),
        ["standalone", "ignored-member", "member"]
    );
    let canonical_root = root.path().canonicalize().expect("canonical Root");
    assert!(
        inventory
            .projects
            .iter()
            .all(|project| project.root.starts_with(&canonical_root))
    );
}

#[test]
fn malformed_nonignored_candidate_is_fatal() {
    let root = TempDir::new().expect("create Root");
    write(root.path().join("Cargo.toml"), "not valid TOML");

    let error = discover_result(root.path(), []).expect_err("malformed manifest must fail");
    assert!(matches!(error, AppError::CargoMetadata { .. }));
}

#[test]
fn root_dot_ignore_excludes_malformed_candidate_manifest() {
    let root = TempDir::new().expect("create Root");
    package(root.path().join("valid"), "valid");
    write(root.path().join(".ignore"), "ignored/\n");
    write(
        root.path().join("ignored/Cargo.toml"),
        "this is deliberately malformed",
    );

    let inventory = discover(root.path(), []);
    assert_eq!(package_names(&inventory), ["valid"]);
}

#[test]
fn package_selectors_and_workspace_exclusions_apply_across_projects() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("workspace/Cargo.toml"),
        "[workspace]\nmembers = [\"alpha\", \"beta\"]\nresolver = \"3\"\n",
    );
    package(root.path().join("workspace/alpha"), "alpha");
    package(root.path().join("workspace/beta"), "beta");
    package(root.path().join("standalone"), "standalone");

    let selected = discover(root.path(), ["--package", "beta"]);
    assert_eq!(package_names(&selected), ["beta"]);

    let union = discover(root.path(), ["--workspace", "--package", "beta"]);
    assert_eq!(package_names(&union), ["standalone", "alpha", "beta"]);

    let excluded = discover(root.path(), ["--workspace", "--exclude", "alpha"]);
    assert_eq!(package_names(&excluded), ["standalone", "beta"]);

    let union_excluded = discover(
        root.path(),
        ["--workspace", "--package", "alpha", "--exclude", "beta"],
    );
    assert_eq!(package_names(&union_excluded), ["standalone", "alpha"]);

    let error = discover_result(root.path(), ["--package", "missing"])
        .expect_err("unmatched package selector must fail");
    assert!(matches!(error, AppError::UnmatchedPackageSelector(_)));
}

#[cfg(unix)]
#[test]
fn operational_pkgid_failures_preserve_cargo_diagnostics() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new().expect("create Root");
    package(root.path().join("app"), "app");
    let wrapper = root.path().join("cargo-wrapper.sh");
    write(
        wrapper.clone(),
        "#!/bin/sh\nif [ \"$1\" = pkgid ]; then\n  echo 'simulated pkgid failure' >&2\n  exit 73\nfi\nexec \"$REAL_CARGO\" \"$@\"\n",
    );
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))
        .expect("make Cargo wrapper executable");

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-sloc"))
        .args(["--package", "app"])
        .arg(root.path())
        .env("CARGO", &wrapper)
        .env("REAL_CARGO", env!("CARGO"))
        .output()
        .expect("run cargo-sloc with Cargo wrapper");

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("simulated pkgid failure"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("matched no eligible Package"),
        "stderr: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn cargo_metadata_time_and_output_are_bounded() {
    use std::os::unix::fs::PermissionsExt;

    for (mode, expected, timeout_ms) in [
        ("sleep", "timed out", "50"),
        ("flood", "output limit", "5000"),
    ] {
        let root = TempDir::new().expect("create Root");
        package(root.path().join("app"), "app");
        let wrapper = root.path().join("cargo-wrapper.sh");
        write(
            wrapper.clone(),
            "#!/bin/sh\nif [ \"$1\" = metadata ]; then\n  if [ \"$CARGO_SLOC_TEST_MODE\" = sleep ]; then\n    sleep 30\n  else\n    while :; do printf 0123456789; done\n  fi\nfi\nexec \"$REAL_CARGO\" \"$@\"\n",
        );
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))
            .expect("make Cargo wrapper executable");

        let output = Command::new(env!("CARGO_BIN_EXE_cargo-sloc"))
            .arg(root.path())
            .env("CARGO", &wrapper)
            .env("REAL_CARGO", env!("CARGO"))
            .env("CARGO_SLOC_TEST_MODE", mode)
            .env("CARGO_SLOC_SUBPROCESS_TIMEOUT_MS", timeout_ms)
            .env("CARGO_SLOC_SUBPROCESS_OUTPUT_LIMIT", "1024")
            .output()
            .expect("run cargo-sloc with bounded Cargo wrapper");

        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "stderr: {stderr}");
        assert!(stderr.contains("Cargo metadata"), "stderr: {stderr}");
    }
}

#[cfg(unix)]
#[test]
fn authoritative_metadata_runs_from_the_resolved_project_root() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = TempDir::new().expect("create workspace");
    write(
        workspace.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n",
    );
    write(workspace.path().join("Cargo.lock"), "version = 4\n");
    package(workspace.path().join("member"), "member");
    let wrapper = workspace.path().join("cargo-wrapper.sh");
    let log = workspace.path().join("metadata-cwds.txt");
    write(
        wrapper.clone(),
        "#!/bin/sh\nif [ \"$1\" = metadata ]; then pwd -P >> \"$CARGO_SLOC_METADATA_CWDS\"; fi\nexec \"$REAL_CARGO\" \"$@\"\n",
    );
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755))
        .expect("make Cargo wrapper executable");

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-sloc"))
        .arg(workspace.path().join("member"))
        .env("CARGO", &wrapper)
        .env("REAL_CARGO", env!("CARGO"))
        .env("CARGO_SLOC_METADATA_CWDS", &log)
        .output()
        .expect("run cargo-sloc through cwd wrapper");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let member = workspace
        .path()
        .join("member")
        .canonicalize()
        .expect("canonical member");
    let project = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let observed = fs::read_to_string(log)
        .expect("read metadata cwd log")
        .lines()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    assert!(!observed.is_empty());
    assert_eq!(observed.len() % 2, 0);
    assert!(
        observed
            .chunks_exact(2)
            .all(|pair| pair == [member.as_path(), project.as_path()])
    );
}

#[test]
fn default_inventory_contains_every_target_kind_and_build_script() {
    let root = target_fixture();
    let inventory = discover(root.path(), []);
    let package = &inventory.projects[0].packages[0];
    let targets: BTreeSet<_> = package
        .targets
        .iter()
        .map(|target| (target.kind, target.name.as_str()))
        .collect();

    assert_eq!(
        targets,
        BTreeSet::from([
            (TargetKind::Lib, "targets"),
            (TargetKind::Bin, "worker"),
            (TargetKind::Example, "demo"),
            (TargetKind::Test, "api"),
            (TargetKind::Bench, "speed"),
            (TargetKind::BuildScript, "build-script-build"),
        ])
    );
}

#[test]
fn required_features_are_retained_for_configuration_resolution() {
    let root = target_fixture();

    let broad = discover(root.path(), ["--no-default-features"]);
    let targets = &broad.projects[0].packages[0].targets;
    let worker = targets
        .iter()
        .find(|target| target.name == "worker")
        .expect("gated target declaration");
    assert_eq!(worker.required_features, BTreeSet::from(["cli".to_owned()]));

    let enabled = discover(
        root.path(),
        [
            "--no-default-features",
            "--features",
            "cli",
            "--bin",
            "worker",
        ],
    );
    assert_eq!(enabled.projects[0].packages[0].targets.len(), 1);
    assert_eq!(enabled.projects[0].packages[0].targets[0].name, "worker");
}

#[test]
fn target_includes_exclusions_and_unmatched_warnings_are_deterministic() {
    let root = target_fixture();
    let inventory = discover(
        root.path(),
        [
            "--tests",
            "--exclude-target",
            "test:api",
            "--exclude-target",
            "example:missing",
        ],
    );
    let package = &inventory.projects[0].packages[0];

    assert!(
        package
            .targets
            .iter()
            .any(|target| target.kind == TargetKind::Lib)
    );
    assert!(
        package
            .targets
            .iter()
            .any(|target| target.kind == TargetKind::Bin)
    );
    assert!(!package.targets.iter().any(|target| target.name == "api"));
    assert_eq!(inventory.warnings.len(), 1);
    assert_eq!(inventory.warnings[0].code, "unmatched-target-exclusion");
}

#[test]
fn named_test_and_bench_selectors_choose_their_target_contexts() {
    let root = target_fixture();

    let test = discover(root.path(), ["--test", "api"]);
    let test_target = &test.projects[0].packages[0].targets[0];
    assert_eq!(test_target.kind, TargetKind::Test);
    assert_eq!(test_target.contexts, BTreeSet::from([TargetContext::Test]));

    let bench = discover(root.path(), ["--bench", "speed"]);
    let bench_target = &bench.projects[0].packages[0].targets[0];
    assert_eq!(bench_target.kind, TargetKind::Bench);
    assert_eq!(
        bench_target.contexts,
        BTreeSet::from([TargetContext::Bench])
    );
    assert!(!bench_target.harness);
}

#[test]
fn named_target_glob_selectors_match_each_supported_target_kind() {
    let root = target_fixture();

    for (argument, pattern, kind, name, context) in [
        (
            "--bin",
            "work*",
            TargetKind::Bin,
            "worker",
            TargetContext::Production,
        ),
        (
            "--example",
            "dem*",
            TargetKind::Example,
            "demo",
            TargetContext::Production,
        ),
        (
            "--test",
            "ap*",
            TargetKind::Test,
            "api",
            TargetContext::Test,
        ),
        (
            "--bench",
            "spe*",
            TargetKind::Bench,
            "speed",
            TargetContext::Bench,
        ),
    ] {
        let inventory = discover(root.path(), [argument, pattern]);
        let target = &inventory.projects[0].packages[0].targets[0];
        assert_eq!(target.kind, kind);
        assert_eq!(target.name, name);
        assert_eq!(target.contexts, BTreeSet::from([context]));
    }

    let error = discover_result(root.path(), ["--bin", "missing-*"])
        .expect_err("unmatched target pattern must fail");
    assert!(matches!(error, AppError::UnmatchedTargetSelector(_)));
}

#[test]
fn plural_test_and_bench_selectors_honor_manifest_flags() {
    let root = harness_fixture();

    let tests = discover(root.path(), ["--tests"]);
    let test_contexts: BTreeSet<_> = tests.projects[0].packages[0]
        .targets
        .iter()
        .map(|target| (target.kind, target.name.as_str(), target.contexts.clone()))
        .collect();
    assert_eq!(
        test_contexts,
        BTreeSet::from([
            (
                TargetKind::Bin,
                "worker",
                BTreeSet::from([TargetContext::Test]),
            ),
            (
                TargetKind::Test,
                "api",
                BTreeSet::from([TargetContext::Test]),
            ),
        ])
    );

    let benches = discover(root.path(), ["--benches"]);
    let bench_contexts: BTreeSet<_> = benches.projects[0].packages[0]
        .targets
        .iter()
        .map(|target| (target.kind, target.name.as_str(), target.contexts.clone()))
        .collect();
    assert_eq!(
        bench_contexts,
        BTreeSet::from([
            (
                TargetKind::Example,
                "demo",
                BTreeSet::from([TargetContext::Bench]),
            ),
            (
                TargetKind::Bench,
                "speed",
                BTreeSet::from([TargetContext::Bench]),
            ),
        ])
    );
}

#[test]
fn implicit_library_is_bench_enabled_unless_explicitly_disabled() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"implicit-bench\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(root.path().join("src/lib.rs"), "pub fn library() {}\n");

    let implicit = discover(root.path(), ["--benches"]);
    let targets = &implicit.projects[0].packages[0].targets;
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].kind, TargetKind::Lib);
    assert_eq!(targets[0].contexts, BTreeSet::from([TargetContext::Bench]));

    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"implicit-bench\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\nbench = false\n",
    );
    let disabled = discover(root.path(), ["--benches"]);
    assert!(disabled.projects[0].packages[0].targets.is_empty());
}

#[test]
fn out_of_root_path_dependencies_are_not_inventory_packages() {
    let parent = TempDir::new().expect("create parent");
    let root = parent.path().join("root");
    let dependency = parent.path().join("dependency");
    package(dependency.clone(), "dependency");
    write(
        root.join("Cargo.toml"),
        "[package]\nname = \"owner\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ndependency = { path = \"../dependency\" }\n",
    );
    write(root.join("src/lib.rs"), "pub fn owner() {}\n");

    let inventory = discover(&root, []);
    assert_eq!(package_names(&inventory), ["owner"]);
}

#[cfg(unix)]
#[test]
fn manifest_discovery_does_not_follow_directory_symlinks() {
    use std::os::unix::fs::symlink;

    let parent = TempDir::new().expect("create parent");
    let root = parent.path().join("root");
    let external = parent.path().join("external");
    fs::create_dir_all(&root).expect("create Root");
    package(external.clone(), "external");
    symlink(&external, root.join("linked")).expect("create directory symlink");

    let inventory = discover(&root, []);
    assert!(inventory.projects.is_empty());
}

fn discover<const N: usize>(root: &Path, arguments: [&str; N]) -> Inventory {
    discover_result(root, arguments).expect("discover fixture")
}

fn discover_result<const N: usize>(
    root: &Path,
    arguments: [&str; N],
) -> Result<Inventory, AppError> {
    let mut arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    arguments.push(root.as_os_str().to_owned());
    let selection = match cargo_sloc::cli::parse(arguments, Path::new(env!("CARGO_MANIFEST_DIR")))?
    {
        ParseOutcome::Selection(selection) => selection,
        ParseOutcome::EarlyExit { .. } => panic!("unexpected early CLI exit"),
    };
    cargo_sloc::discovery::discover(&selection)
}

fn package_names(inventory: &Inventory) -> Vec<&str> {
    inventory
        .projects
        .iter()
        .flat_map(|project| project.packages.iter())
        .map(|package| package.name.as_str())
        .collect()
}

fn target_fixture() -> TempDir {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"targets\"\nversion = \"0.1.0\"\nedition = \"2024\"\nbuild = \"build.rs\"\n\n[features]\ncli = []\n\n[[bin]]\nname = \"worker\"\npath = \"src/main.rs\"\nrequired-features = [\"cli\"]\n\n[[example]]\nname = \"demo\"\npath = \"examples/demo.rs\"\n\n[[test]]\nname = \"api\"\npath = \"tests/api.rs\"\n\n[[bench]]\nname = \"speed\"\npath = \"benches/speed.rs\"\nharness = false\n",
    );
    write(root.path().join("src/lib.rs"), "pub fn library() {}\n");
    write(root.path().join("src/main.rs"), "fn main() {}\n");
    write(root.path().join("examples/demo.rs"), "fn main() {}\n");
    write(root.path().join("tests/api.rs"), "#[test] fn api() {}\n");
    write(root.path().join("benches/speed.rs"), "fn main() {}\n");
    write(root.path().join("build.rs"), "fn main() {}\n");
    root
}

fn harness_fixture() -> TempDir {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"harnesses\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\ntest = false\nbench = false\n\n[[bin]]\nname = \"worker\"\npath = \"src/main.rs\"\ntest = true\nbench = false\n\n[[example]]\nname = \"demo\"\npath = \"examples/demo.rs\"\ntest = false\nbench = true\n\n[[test]]\nname = \"api\"\npath = \"tests/api.rs\"\n\n[[bench]]\nname = \"speed\"\npath = \"benches/speed.rs\"\nharness = false\n",
    );
    write(root.path().join("src/lib.rs"), "pub fn library() {}\n");
    write(root.path().join("src/main.rs"), "fn main() {}\n");
    write(root.path().join("examples/demo.rs"), "fn main() {}\n");
    write(root.path().join("tests/api.rs"), "#[test] fn api() {}\n");
    write(root.path().join("benches/speed.rs"), "fn main() {}\n");
    root
}

fn package(root: PathBuf, name: &str) {
    write(
        root.join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    );
    write(root.join("src/lib.rs"), "pub fn marker() {}\n");
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("fixture file parent"))
        .expect("create fixture directory");
    fs::write(path, contents).expect("write fixture file");
}
