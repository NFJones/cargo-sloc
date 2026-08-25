//! Root-bounded Cargo Project, Package, and target discovery.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_metadata::{CargoOpt, Metadata, MetadataCommand, Package, Target};
use cargo_toml::{Manifest, Product};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::model::Selection;
use crate::report::Warning;

/// Deterministic inventory selected for later configuration analysis.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Inventory {
    /// Discovered Projects that contain at least one selected Package.
    pub projects: Vec<ProjectInventory>,
    /// Nonfatal selection diagnostics.
    pub warnings: Vec<Warning>,
}

/// One Cargo workspace or standalone package Project.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProjectInventory {
    /// Canonical workspace or standalone root.
    pub root: PathBuf,
    /// Project manifest used for Cargo queries.
    pub manifest_path: PathBuf,
    /// Owned Cargo metadata document shared with configuration resolution.
    pub metadata: ProjectMetadataSnapshot,
    /// Edition of the workspace root Package, or `None` for a virtual workspace.
    pub root_package_edition: Option<String>,
    /// Selected Packages owned by this Project.
    pub packages: Vec<PackageInventory>,
}

/// One owned all-feature Cargo metadata document for a Project.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProjectMetadataSnapshot {
    document: String,
}

impl ProjectMetadataSnapshot {
    pub(crate) fn document(&self) -> &str {
        &self.document
    }
}

/// One selected Cargo Package.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PackageInventory {
    /// Cargo's opaque package ID.
    pub id: String,
    /// Manifest package name.
    pub name: String,
    /// Absolute manifest path.
    pub manifest_path: PathBuf,
    /// Declared feature names.
    pub declared_features: BTreeSet<String>,
    /// Selected package targets.
    pub targets: Vec<TargetInventory>,
}

/// One selected Cargo package Target.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TargetInventory {
    /// Cargo target name.
    pub name: String,
    /// Canonical cargo-sloc target kind.
    pub kind: TargetKind,
    /// Absolute source root path.
    pub source_path: PathBuf,
    /// Rust edition reported by Cargo.
    pub edition: String,
    /// Cargo crate types, retaining Cargo's stable display names.
    pub crate_types: Vec<String>,
    /// Features required for target eligibility.
    pub required_features: BTreeSet<String>,
    /// Selected compilation-context classes for this accounting root.
    pub contexts: BTreeSet<TargetContext>,
    /// Whether the manifest enables a generated harness for this target.
    pub harness: bool,
}

/// Compilation-context classes selected for a Cargo Target.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum TargetContext {
    /// Normal non-harness compilation.
    Production,
    /// Unit, example, binary, or integration-test harness compilation.
    Test,
    /// Unit, example, binary, or benchmark harness compilation.
    Bench,
}

/// Target kinds exposed by cargo-sloc selectors.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum TargetKind {
    /// Library-like target, including procedural macros.
    Lib,
    /// Binary target.
    Bin,
    /// Example target.
    Example,
    /// Integration-test target.
    Test,
    /// Benchmark target.
    Bench,
    /// Custom build-script target.
    BuildScript,
}

impl TargetKind {
    /// Returns the stable selector spelling for this target kind.
    pub fn selector_name(self) -> &'static str {
        match self {
            Self::Lib => "lib",
            Self::Bin => "bin",
            Self::Example => "example",
            Self::Test => "test",
            Self::Bench => "bench",
            Self::BuildScript => "build-script",
        }
    }
}

/// Discovers and selects the inventory requested by the normalized CLI.
pub fn discover(selection: &Selection) -> Result<Inventory, AppError> {
    let candidates = candidate_manifests(selection.root.as_path())?;
    let mut projects = BTreeMap::<PathBuf, LoadedProject>::new();
    let mut known_manifests = BTreeSet::new();

    for manifest in candidates {
        let manifest = canonical_path(&manifest)?;
        if known_manifests.contains(&manifest) {
            continue;
        }
        let loaded = load_metadata(&manifest, selection.root.as_path())?;
        let project_root = canonical_path(loaded.metadata.workspace_root.as_std_path())?;
        if let std::collections::btree_map::Entry::Vacant(entry) =
            projects.entry(project_root.clone())
        {
            let project_manifest = project_root.join("Cargo.toml");
            for package in loaded.metadata.workspace_packages() {
                known_manifests.insert(canonical_path(package.manifest_path.as_std_path())?);
            }
            entry.insert(LoadedProject {
                manifest: project_manifest,
                metadata: loaded.metadata,
                metadata_snapshot: ProjectMetadataSnapshot {
                    document: loaded.document,
                },
            });
        }
    }

    let selected_ids = select_package_ids(&projects, selection)?;
    let mut warnings = Vec::new();
    let mut inventory_projects = Vec::new();
    let mut target_state = TargetSelectionState::default();

    for (project_root, loaded) in projects {
        let workspace_members: BTreeSet<_> = loaded
            .metadata
            .workspace_members
            .iter()
            .map(|id| id.repr.as_str())
            .collect();
        let mut packages = Vec::new();

        for package in &loaded.metadata.packages {
            let manifest_path = canonical_path(package.manifest_path.as_std_path())?;
            if !workspace_members.contains(package.id.repr.as_str())
                || !manifest_path.starts_with(selection.root.as_path())
                || !selected_ids.contains(&package.id.repr)
            {
                continue;
            }

            let manifest =
                Manifest::from_path(&manifest_path).map_err(|source| AppError::CargoManifest {
                    manifest: manifest_path.clone(),
                    source,
                })?;
            let targets = select_targets(package, &manifest, selection, &mut target_state);
            packages.push(PackageInventory {
                id: package.id.repr.clone(),
                name: package.name.to_string(),
                manifest_path,
                declared_features: package.features.keys().cloned().collect(),
                targets,
            });
        }
        packages.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.manifest_path.cmp(&right.manifest_path))
                .then_with(|| left.id.cmp(&right.id))
        });
        let root_package_edition = loaded
            .metadata
            .packages
            .iter()
            .find(|package| package.manifest_path.as_std_path() == loaded.manifest)
            .map(|package| package.edition.to_string());
        inventory_projects.push(ProjectInventory {
            manifest_path: loaded.manifest,
            root: project_root,
            metadata: loaded.metadata_snapshot,
            root_package_edition,
            packages,
        });
    }
    target_state.finish(selection, &mut warnings)?;
    warnings.sort();
    Ok(Inventory {
        projects: inventory_projects,
        warnings,
    })
}

#[derive(Debug)]
struct LoadedProject {
    manifest: PathBuf,
    metadata: Metadata,
    metadata_snapshot: ProjectMetadataSnapshot,
}

struct LoadedMetadata {
    metadata: Metadata,
    document: String,
}

fn candidate_manifests(root: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(false)
        .hidden(false)
        .parents(false)
        .ignore(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|left, right| left.cmp(right))
        .filter_entry(|entry| {
            if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                return true;
            }
            !matches!(
                entry.file_name().to_str(),
                Some("target" | ".cargo-sloc" | ".git" | ".hg" | ".svn")
            )
        });

    let mut manifests = Vec::new();
    for entry in builder.build() {
        let entry = entry.map_err(|source| AppError::Discovery {
            root: root.to_path_buf(),
            source,
        })?;
        if entry.file_type().is_some_and(|kind| kind.is_file())
            && entry.file_name() == OsStr::new("Cargo.toml")
        {
            manifests.push(entry.into_path());
        }
    }
    Ok(manifests)
}

fn load_metadata(manifest: &Path, root: &Path) -> Result<LoadedMetadata, AppError> {
    let mut command = MetadataCommand::new();
    command
        .manifest_path(manifest)
        .current_dir(manifest.parent().unwrap_or(root))
        .features(CargoOpt::AllFeatures);
    let document = crate::process::cargo_metadata_json(
        &command,
        format!("Cargo metadata for `{}`", manifest.display()),
    )
    .map_err(|error| AppError::CargoMetadata {
        manifest: manifest.to_path_buf(),
        message: error.to_string(),
    })?;
    let metadata = MetadataCommand::parse(&document).map_err(|error| AppError::CargoMetadata {
        manifest: manifest.to_path_buf(),
        message: error.to_string(),
    })?;
    Ok(LoadedMetadata { metadata, document })
}

fn select_package_ids(
    projects: &BTreeMap<PathBuf, LoadedProject>,
    selection: &Selection,
) -> Result<BTreeSet<String>, AppError> {
    let eligible: BTreeSet<_> = projects
        .values()
        .flat_map(|project| project.metadata.workspace_packages())
        .filter(|package| {
            package
                .manifest_path
                .as_std_path()
                .canonicalize()
                .is_ok_and(|path| path.starts_with(selection.root.as_path()))
        })
        .map(|package| package.id.repr.clone())
        .collect();
    let mut selected = if selection.workspace || selection.package_selectors.is_empty() {
        eligible.clone()
    } else {
        BTreeSet::new()
    };
    if !selection.package_selectors.is_empty() {
        selected.extend(resolve_selectors(
            projects,
            &selection.package_selectors,
            &eligible,
        )?);
    }

    if selection.workspace && !selection.package_exclude_selectors.is_empty() {
        let excluded =
            resolve_selectors(projects, &selection.package_exclude_selectors, &eligible)?;
        selected.retain(|id| !excluded.contains(id));
    }
    Ok(selected)
}

fn resolve_selectors(
    projects: &BTreeMap<PathBuf, LoadedProject>,
    selectors: &BTreeSet<String>,
    eligible: &BTreeSet<String>,
) -> Result<BTreeSet<String>, AppError> {
    let mut resolved = BTreeSet::new();
    for selector in selectors {
        let mut matched = false;
        for (project_root, project) in projects {
            crate::metrics::record_query(crate::metrics::Query::CargoPackageId);
            let mut command =
                Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            command
                .args(["pkgid", "--manifest-path"])
                .arg(&project.manifest)
                .arg(selector)
                .current_dir(project_root);
            let output = crate::process::run(
                &mut command,
                format!(
                    "Cargo package selector `{selector}` in `{}`",
                    project_root.display()
                ),
            )
            .map_err(|error| AppError::PackageSelector {
                selector: selector.clone(),
                project: project_root.clone(),
                message: error.to_string(),
            })?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                if is_unmatched_package_spec(&stderr) {
                    continue;
                }
                return Err(AppError::PackageSelector {
                    selector: selector.clone(),
                    project: project_root.clone(),
                    message: format!("Cargo exited with {}: {stderr}", output.status),
                });
            }
            let id = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if eligible.contains(&id) {
                resolved.insert(id);
                matched = true;
            }
        }
        if !matched {
            return Err(AppError::UnmatchedPackageSelector(selector.clone()));
        }
    }
    Ok(resolved)
}

fn is_unmatched_package_spec(stderr: &str) -> bool {
    stderr.contains("package ID specification `") && stderr.contains("did not match any packages")
}

fn select_targets(
    package: &Package,
    manifest: &Manifest,
    selection: &Selection,
    state: &mut TargetSelectionState,
) -> Vec<TargetInventory> {
    let broad_all = selection.target_includes.contains("all-targets");
    let mut selected = Vec::new();

    for target in &package.targets {
        let Some(kind) = target_kind(target) else {
            continue;
        };
        let product = manifest_product(manifest, target, kind);
        let test_enabled = product.map_or(target.test, |product| product.test);
        let bench_enabled = product.is_some_and(|product| product.bench);
        let harness = product.is_none_or(|product| product.harness);
        let canonical = format!("{}:{}", kind.selector_name(), target.name);
        let production_requested = broad_all
            || selection
                .target_includes
                .contains(kind_plural_selector(kind))
            || selection.target_includes.contains(kind.selector_name())
            || selection.target_includes.contains(&canonical);
        let named_target = selection.target_includes.contains(&canonical);
        let test_requested = (named_target && kind == TargetKind::Test)
            || ((broad_all || selection.target_includes.contains("tests"))
                && (kind == TargetKind::Test
                    || (matches!(
                        kind,
                        TargetKind::Lib | TargetKind::Bin | TargetKind::Example
                    ) && test_enabled)));
        let bench_requested = (named_target && kind == TargetKind::Bench)
            || ((broad_all || selection.target_includes.contains("benches"))
                && (kind == TargetKind::Bench
                    || (matches!(
                        kind,
                        TargetKind::Lib | TargetKind::Bin | TargetKind::Example
                    ) && bench_enabled)));
        if !production_requested && !test_requested && !bench_requested {
            continue;
        }
        if selection.target_includes.contains(&canonical) {
            state.named_seen.insert(canonical.clone());
        }

        let mut contexts =
            target_contexts(kind, production_requested, test_requested, bench_requested);
        apply_target_exclusions(
            selection,
            kind,
            &canonical,
            &mut contexts,
            &mut state.exclusions,
        );
        if contexts.is_empty() {
            continue;
        }
        selected.push(target_inventory(target, kind, contexts, harness));
    }

    selected.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    selected
}

#[derive(Default)]
struct TargetSelectionState {
    named_seen: BTreeSet<String>,
    exclusions: BTreeSet<String>,
}

impl TargetSelectionState {
    fn finish(self, selection: &Selection, warnings: &mut Vec<Warning>) -> Result<(), AppError> {
        for selector in selection
            .target_includes
            .iter()
            .filter(|selector| selector.contains(':'))
        {
            if !self.named_seen.contains(selector) {
                return Err(AppError::UnmatchedTargetSelector(selector.clone()));
            }
        }
        for selector in &selection.target_excludes {
            if !self.exclusions.contains(selector) {
                warnings.push(Warning {
                    code: "unmatched-target-exclusion".to_owned(),
                    message: format!("target exclusion `{selector}` matched nothing"),
                });
            }
        }
        Ok(())
    }
}

fn target_kind(target: &Target) -> Option<TargetKind> {
    if target.is_custom_build() {
        Some(TargetKind::BuildScript)
    } else if target.is_test() {
        Some(TargetKind::Test)
    } else if target.is_bench() {
        Some(TargetKind::Bench)
    } else if target.is_example() {
        Some(TargetKind::Example)
    } else if target.is_bin() {
        Some(TargetKind::Bin)
    } else if target.is_lib()
        || target.is_proc_macro()
        || target.is_rlib()
        || target.is_dylib()
        || target.is_cdylib()
        || target.is_staticlib()
    {
        Some(TargetKind::Lib)
    } else {
        None
    }
}

fn kind_plural_selector(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Lib => "lib",
        TargetKind::Bin => "bins",
        TargetKind::Example => "examples",
        TargetKind::Test => "tests",
        TargetKind::Bench => "benches",
        TargetKind::BuildScript => "build-script",
    }
}

fn manifest_product<'a>(
    manifest: &'a Manifest,
    target: &Target,
    kind: TargetKind,
) -> Option<&'a Product> {
    let products: &[Product] = match kind {
        TargetKind::Lib => return manifest.lib.as_ref(),
        TargetKind::Bin => &manifest.bin,
        TargetKind::Example => &manifest.example,
        TargetKind::Test => &manifest.test,
        TargetKind::Bench => &manifest.bench,
        TargetKind::BuildScript => return None,
    };
    products
        .iter()
        .find(|product| product.name.as_deref() == Some(target.name.as_str()))
}

fn target_contexts(
    kind: TargetKind,
    production_requested: bool,
    test_requested: bool,
    bench_requested: bool,
) -> BTreeSet<TargetContext> {
    let mut contexts = BTreeSet::new();
    if production_requested
        && matches!(
            kind,
            TargetKind::Lib | TargetKind::Bin | TargetKind::Example | TargetKind::BuildScript
        )
    {
        contexts.insert(TargetContext::Production);
    }
    if test_requested {
        contexts.insert(TargetContext::Test);
    }
    if bench_requested {
        contexts.insert(TargetContext::Bench);
    }
    contexts
}

fn apply_target_exclusions(
    selection: &Selection,
    kind: TargetKind,
    canonical: &str,
    contexts: &mut BTreeSet<TargetContext>,
    matched: &mut BTreeSet<String>,
) {
    for selector in &selection.target_excludes {
        let did_match = match selector.as_str() {
            "test" if contexts.remove(&TargetContext::Test) => true,
            "bench" if contexts.remove(&TargetContext::Bench) => true,
            value
                if value == kind.selector_name()
                    && !matches!(kind, TargetKind::Test | TargetKind::Bench) =>
            {
                contexts.clear();
                true
            }
            value if value == canonical => {
                contexts.clear();
                true
            }
            _ => false,
        };
        if did_match {
            matched.insert(selector.clone());
        }
    }
}

fn target_inventory(
    target: &Target,
    kind: TargetKind,
    contexts: BTreeSet<TargetContext>,
    harness: bool,
) -> TargetInventory {
    TargetInventory {
        name: target.name.clone(),
        kind,
        source_path: target.src_path.as_std_path().to_path_buf(),
        edition: target.edition.to_string(),
        crate_types: target.crate_types.iter().map(ToString::to_string).collect(),
        required_features: target.required_features.iter().cloned().collect(),
        contexts,
        harness,
    }
}

fn canonical_path(path: &Path) -> Result<PathBuf, AppError> {
    path.canonicalize()
        .map_err(|source| AppError::CanonicalPath {
            path: path.to_path_buf(),
            source,
        })
}
