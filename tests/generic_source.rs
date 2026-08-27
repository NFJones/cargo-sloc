//! Integration coverage for package-aware non-Rust source inventory.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use cargo_sloc::configuration::{ConfiguredInventory, ConfiguredPackage, ConfiguredProject};
use cargo_sloc::discovery::{TargetContext, TargetInventory, TargetKind};
use cargo_sloc::generic_source::{INVENTORY_POLICY_VERSION, discover, discover_root};
use tempfile::TempDir;

#[test]
fn assigns_candidates_to_deepest_selected_package_and_retains_bytes() {
    let root = TempDir::new().expect("create Root");
    package(root.path().join("outer"), "outer");
    package(root.path().join("outer/nested"), "nested");
    package(root.path().join("outer/unselected"), "unselected");
    write(root.path().join("outer/web/app.js"), "const value = 1;\n");
    write(
        root.path().join("outer/nested/tool.py"),
        "print('nested')\n",
    );
    write(
        root.path().join("outer/unselected/ignored.js"),
        "const ignored = true;\n",
    );
    write(root.path().join("outer/src/extra.rs"), "fn ignored() {}\n");

    let inventory = discover(
        &configured(
            root.path(),
            [
                selected_package(root.path().join("outer"), "outer", true),
                selected_package(root.path().join("outer/nested"), "nested", true),
            ],
        ),
        candidate,
    )
    .expect("discover generic source");

    assert_eq!(INVENTORY_POLICY_VERSION, 2);
    assert!(inventory.warnings.is_empty());
    assert_eq!(inventory.packages.len(), 2);
    assert_eq!(inventory.packages[0].name, "nested");
    assert_eq!(inventory.packages[1].name, "outer");
    assert_eq!(
        relative_paths(root.path(), &inventory.packages[0].files),
        BTreeSet::from(["outer/nested/tool.py".to_owned()])
    );
    assert_eq!(
        relative_paths(root.path(), &inventory.packages[1].files),
        BTreeSet::from(["outer/web/app.js".to_owned()])
    );
    assert_eq!(
        inventory.packages[1].files[0].bytes.as_ref(),
        b"const value = 1;\n"
    );
}

#[test]
fn selected_package_without_retained_targets_owns_generic_files() {
    let root = TempDir::new().expect("create Root");
    package(root.path().join("app"), "app");
    write(
        root.path().join("app/web/app.js"),
        "const targetless = true;\n",
    );

    let inventory = discover(
        &configured(
            root.path(),
            [selected_package(root.path().join("app"), "app", false)],
        ),
        candidate,
    )
    .expect("discover targetless selected package source");

    assert_eq!(inventory.packages.len(), 1);
    assert_eq!(inventory.packages[0].name, "app");
    assert_eq!(
        relative_paths(root.path(), &inventory.packages[0].files),
        BTreeSet::from(["app/web/app.js".to_owned()])
    );
}

#[test]
fn honors_root_local_ignores_and_only_structural_exclusions() {
    let root = TempDir::new().expect("create Root");
    package(root.path().join("active"), "active");
    package(root.path().join("inactive"), "inactive");
    write(root.path().join(".gitignore"), "active/ignored.js\n");
    for path in [
        "active/kept.js",
        "active/ignored.js",
        "active/target/output.js",
        "active/vendor/dependency.js",
        "active/generated/output.js",
        "active/.git/internal.js",
        "inactive/not-selected.py",
    ] {
        write(root.path().join(path), "source\n");
    }

    let inventory = discover(
        &configured(
            root.path(),
            [selected_package(root.path().join("active"), "active", true)],
        ),
        candidate,
    )
    .expect("discover generic source");

    assert_eq!(inventory.packages.len(), 1);
    assert_eq!(inventory.packages[0].name, "active");
    assert_eq!(
        relative_paths(root.path(), &inventory.packages[0].files),
        BTreeSet::from([
            "active/generated/output.js".to_owned(),
            "active/kept.js".to_owned(),
            "active/target/output.js".to_owned(),
            "active/vendor/dependency.js".to_owned(),
        ])
    );
}

#[cfg(unix)]
#[test]
fn canonical_identity_deduplicates_file_symlinks() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("create Root");
    package(root.path().join("app"), "app");
    write(root.path().join("app/source.js"), "const source = true;\n");
    symlink("source.js", root.path().join("app/alias.js")).expect("create file symlink");

    let inventory = discover(
        &configured(
            root.path(),
            [selected_package(root.path().join("app"), "app", true)],
        ),
        candidate,
    )
    .expect("discover generic source");

    assert_eq!(inventory.packages.len(), 1);
    assert_eq!(inventory.packages[0].files.len(), 1);
    assert!(inventory.packages[0].files[0].path.ends_with("alias.js"));
    assert_eq!(inventory.root.files.len(), 2);
    let javascript = inventory
        .root
        .files
        .iter()
        .find(|file| {
            file.representative_path
                .extension()
                .and_then(|value| value.to_str())
                == Some("js")
        })
        .expect("JavaScript ledger record");
    assert_eq!(javascript.aliases.len(), 2);
}

#[test]
fn discovers_supported_files_without_a_cargo_project() {
    let root = TempDir::new().expect("create Root");
    write(root.path().join("scripts/tool.py"), "print('root')\n");
    write(root.path().join("web/app.js"), "const root = true;\n");

    let inventory = discover_root(root.path(), &ConfiguredInventory::default(), candidate)
        .expect("discover root source");

    assert!(inventory.packages.is_empty());
    assert_eq!(
        inventory
            .root
            .files
            .iter()
            .map(|file| relative(root.path(), &file.representative_path))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["scripts/tool.py".to_owned(), "web/app.js".to_owned()])
    );
    assert!(
        inventory
            .root
            .files
            .iter()
            .all(|file| file.containing_packages.is_empty())
    );
}

#[test]
fn honors_nested_ignore_files_and_negation_from_the_requested_root() {
    let root = TempDir::new().expect("create Root");
    write(root.path().join(".ignore"), "ignored.js\n");
    write(root.path().join("nested/.gitignore"), "*.js\n!kept.js\n");
    write(root.path().join("ignored.js"), "ignored\n");
    write(root.path().join("nested/ignored.js"), "ignored\n");
    write(root.path().join("nested/kept.js"), "kept\n");

    let inventory = discover_root(root.path(), &ConfiguredInventory::default(), candidate)
        .expect("discover ignored root source");

    assert_eq!(inventory.root.files.len(), 1);
    assert_eq!(
        relative(root.path(), &inventory.root.files[0].representative_path),
        "nested/kept.js"
    );
}

#[cfg(unix)]
#[test]
fn physical_identity_deduplicates_hard_links_globally() {
    let root = TempDir::new().expect("create Root");
    write(root.path().join("a/source.js"), "const shared = true;\n");
    fs::create_dir_all(root.path().join("b")).expect("create alias directory");
    fs::hard_link(
        root.path().join("a/source.js"),
        root.path().join("b/alias.js"),
    )
    .expect("create hard link");

    let inventory = discover_root(root.path(), &ConfiguredInventory::default(), candidate)
        .expect("discover hard-linked source");

    assert_eq!(inventory.root.files.len(), 1);
    assert_eq!(inventory.root.files[0].aliases.len(), 2);
    assert_eq!(
        relative(root.path(), &inventory.root.files[0].representative_path),
        "a/source.js"
    );
}

fn candidate(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("js" | "py" | "rs")
    )
}

fn configured<const N: usize>(
    root: &Path,
    packages: [ConfiguredPackage; N],
) -> ConfiguredInventory {
    ConfiguredInventory {
        projects: vec![ConfiguredProject {
            root: root.to_path_buf(),
            host_target: "test-host".to_owned(),
            targets: vec!["test-host".to_owned()],
            packages: packages.into(),
        }],
        unselected_package_roots: Vec::new(),
        warnings: Vec::new(),
    }
}

fn selected_package(root: PathBuf, name: &str, has_target: bool) -> ConfiguredPackage {
    ConfiguredPackage {
        id: format!("path+file://{}#{name}@0.1.0", root.display()),
        name: name.to_owned(),
        manifest_path: root.join("Cargo.toml"),
        targets: has_target
            .then(|| target(root.join("src/lib.rs")))
            .into_iter()
            .collect(),
        contexts: Vec::new(),
    }
}

fn target(source_path: PathBuf) -> TargetInventory {
    TargetInventory {
        name: "fixture".to_owned(),
        kind: TargetKind::Lib,
        source_path,
        edition: "2024".to_owned(),
        crate_types: vec!["lib".to_owned()],
        required_features: BTreeSet::new(),
        contexts: BTreeSet::from([TargetContext::Production]),
        harness: false,
    }
}

fn package(root: PathBuf, name: &str) {
    write(
        root.join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
    );
    write(root.join("src/lib.rs"), "pub fn fixture() {}\n");
}

fn relative_paths(
    root: &Path,
    files: &[cargo_sloc::generic_source::GenericSource],
) -> BTreeSet<String> {
    let root = root.canonicalize().expect("canonical Root");
    files
        .iter()
        .map(|source| {
            source
                .path
                .strip_prefix(&root)
                .expect("source beneath Root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn relative(root: &Path, path: &Path) -> String {
    let root = root.canonicalize().expect("canonical Root");
    path.strip_prefix(root)
        .expect("path beneath Root")
        .to_string_lossy()
        .replace('\\', "/")
}

fn write(path: PathBuf, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture directory");
    }
    fs::write(path, contents).expect("write fixture file");
}
