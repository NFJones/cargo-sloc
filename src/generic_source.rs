//! Root-wide inventory for supported source candidates.
//!
//! This module owns one Root-local filesystem traversal, invocation-wide
//! physical-file deduplication, Package-containment claims, and one-read handoff
//! to language Accountants. Final report ownership and accounting-route
//! selection are resolved by later orchestration phases.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ignore::WalkBuilder;

use crate::configuration::{ConfiguredInventory, ConfiguredPackage};
use crate::error::AppError;
use crate::report::Warning;

/// Version of the Root traversal, identity, and eligibility policy.
pub const INVENTORY_POLICY_VERSION: u32 = 2;

/// Supported source candidates discovered beneath one Root.
#[derive(Clone, Debug, Default)]
pub struct GenericSourceInventory {
    /// Invocation-wide physical-file ledger before final ownership or routing.
    pub root: RootSourceInventory,
    /// Package inventories in deterministic Project/Package order.
    ///
    /// This compatibility projection is derived from `root` for the current
    /// generic Accountant and will be removed when routing consumes records.
    pub packages: Vec<GenericPackageSources>,
    /// Nonfatal filesystem and identity diagnostics.
    pub warnings: Vec<Warning>,
}

/// Invocation-wide ledger of unique supported physical files.
#[derive(Clone, Debug, Default)]
pub struct RootSourceInventory {
    /// Canonical absolute requested Root.
    pub root: PathBuf,
    /// Globally deduplicated files in representative-path order.
    pub files: Vec<FileRecord>,
}

/// Stable physical identity used to collapse aliases globally.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PhysicalFileId {
    /// Unix device and inode identity.
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
    /// Canonical-path fallback on platforms without a stronger implementation.
    Canonical(PathBuf),
}

/// Current inventory disposition before owner and route resolution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileDisposition {
    /// The file is eligible and awaits final owner/route resolution.
    Pending,
}

/// One globally unique supported file and its discovery claims.
#[derive(Clone, Debug)]
pub struct FileRecord {
    /// Stable physical identity.
    pub identity: PhysicalFileId,
    /// Lexicographically first absolute alias beneath the Root.
    pub representative_path: PathBuf,
    /// Every discovered in-Root alias in deterministic order.
    pub aliases: BTreeSet<PathBuf>,
    /// Selected Package IDs whose roots contain the representative path.
    pub containing_packages: BTreeSet<String>,
    /// Retained contents read once for subsequent recognition/accounting.
    pub bytes: Arc<[u8]>,
    /// Auditable current disposition.
    pub disposition: FileDisposition,
}

/// Generic source candidates owned by one selected Cargo Package.
#[derive(Clone, Debug)]
pub struct GenericPackageSources {
    /// Canonical root of the owning Cargo Project.
    pub project_root: PathBuf,
    /// Cargo's opaque Package ID.
    pub id: String,
    /// Cargo Package name.
    pub name: String,
    /// Absolute Package manifest path.
    pub manifest_path: PathBuf,
    /// Unique eligible physical files in canonical path order.
    pub files: Vec<GenericSource>,
}

/// One physical non-Rust source candidate and its already-read bytes.
#[derive(Clone, Debug)]
pub struct GenericSource {
    /// Canonical absolute physical-file identity.
    pub path: PathBuf,
    /// File contents retained for in-memory language accounting.
    pub bytes: Arc<[u8]>,
}

/// Resident cache for content-validated generic-source bytes.
#[derive(Default)]
pub(crate) struct SourceCache {
    bytes: BTreeMap<PathBuf, Arc<[u8]>>,
    touched: BTreeSet<PathBuf>,
    validated_unchanged: BTreeSet<PathBuf>,
    validated_bytes: BTreeMap<PathBuf, Arc<[u8]>>,
    dependencies: BTreeSet<PathBuf>,
}

impl SourceCache {
    pub(crate) fn set_validation(
        &mut self,
        unchanged: BTreeSet<PathBuf>,
        bytes: BTreeMap<PathBuf, Arc<[u8]>>,
    ) {
        self.validated_unchanged = unchanged;
        self.validated_bytes = bytes;
    }

    pub(crate) fn dependencies(&self) -> &BTreeSet<PathBuf> {
        &self.dependencies
    }

    fn begin_refresh(&mut self) {
        self.touched.clear();
        self.dependencies.clear();
    }

    fn finish_refresh(&mut self) {
        self.bytes.retain(|path, _| self.touched.contains(path));
        self.validated_unchanged.clear();
        self.validated_bytes.clear();
    }

    fn load(&mut self, actual_path: &Path, identity: &Path) -> Result<Arc<[u8]>, std::io::Error> {
        self.touched.insert(identity.to_path_buf());
        self.dependencies.insert(actual_path.to_path_buf());
        if self.validated_unchanged.contains(actual_path)
            && let Some(bytes) = self.bytes.get(identity)
        {
            crate::metrics::record_cache(
                crate::metrics::Cache::GenericSource,
                crate::metrics::CacheOutcome::Hit,
                "validated-unchanged-source",
            );
            return Ok(Arc::clone(bytes));
        }
        let bytes = self
            .validated_bytes
            .remove(actual_path)
            .map_or_else(|| fs::read(actual_path).map(Arc::<[u8]>::from), Ok)?;
        if let Some(cached) = self
            .bytes
            .get(identity)
            .filter(|cached| cached.as_ref() == bytes.as_ref())
        {
            crate::metrics::record_cache(
                crate::metrics::Cache::GenericSource,
                crate::metrics::CacheOutcome::Hit,
                "physical-source-content",
            );
            return Ok(Arc::clone(cached));
        }
        crate::metrics::record_cache(
            crate::metrics::Cache::GenericSource,
            crate::metrics::CacheOutcome::Miss,
            "physical-source-content",
        );
        self.bytes
            .insert(identity.to_path_buf(), Arc::clone(&bytes));
        Ok(bytes)
    }
}

/// Discovers supported files beneath the configured Root.
///
/// This compatibility entry point derives the Root from the configured
/// inventory. Callers that retain the user's requested Root should use
/// [`discover_root`] so files outside Cargo Projects remain discoverable.
pub fn discover(
    configured: &ConfiguredInventory,
    is_candidate: impl Fn(&Path) -> bool,
) -> Result<GenericSourceInventory, AppError> {
    let root = configured
        .projects
        .iter()
        .map(|project| project.root.as_path())
        .min()
        .ok_or_else(|| {
            AppError::ReportInvariant(
                "root-wide source discovery requires an explicit Root when no Cargo Project exists"
                    .to_owned(),
            )
        })?;
    discover_root(root, configured, is_candidate)
}

/// Discovers supported files with one deterministic walk of `root`.
pub fn discover_root(
    root: &Path,
    configured: &ConfiguredInventory,
    is_candidate: impl Fn(&Path) -> bool,
) -> Result<GenericSourceInventory, AppError> {
    let mut cache = SourceCache::default();
    discover_root_with_cache(root, configured, is_candidate, &mut cache)
}

/// Discovers root-wide source while reusing content-validated retained bytes.
pub(crate) fn discover_root_with_cache(
    root: &Path,
    configured: &ConfiguredInventory,
    is_candidate: impl Fn(&Path) -> bool,
    cache: &mut SourceCache,
) -> Result<GenericSourceInventory, AppError> {
    let root = root
        .canonicalize()
        .map_err(|source| AppError::InvalidRoot {
            path: root.to_path_buf(),
            source,
        })?;
    cache.begin_refresh();
    let mut warnings = Vec::new();
    let mut owners = configured
        .projects
        .iter()
        .flat_map(|project| {
            project
                .packages
                .iter()
                .map(|package| PackageOwner::new(&project.root, package))
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    owners.sort_by(|left, right| {
        right
            .root
            .components()
            .count()
            .cmp(&left.root.components().count())
            .then_with(|| left.root.cmp(&right.root))
            .then_with(|| left.package.id.cmp(&right.package.id))
    });

    let mut records = BTreeMap::<PhysicalFileId, FileRecord>::new();
    let mut builder = WalkBuilder::new(&root);
    let filter_root = root.clone();
    builder
        .standard_filters(false)
        .hidden(false)
        .parents(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|left, right| left.cmp(right))
        .filter_entry(move |entry| !is_structural_directory(entry.path(), &filter_root));

    for entry in builder.build() {
        let entry = entry.map_err(|source| AppError::Discovery {
            root: root.clone(),
            source,
        })?;
        let path = entry.path();
        if entry.file_type().is_some_and(|kind| kind.is_dir())
            || !path.is_file()
            || (!is_rust_source(path) && !is_candidate(path))
        {
            continue;
        }
        let canonical = match path.canonicalize() {
            Ok(canonical) if canonical.starts_with(&root) => canonical,
            Ok(canonical) => {
                warnings.push(skipped_warning(
                    "source-outside-root",
                    path,
                    format!(
                        "canonical target `{}` is outside the Root",
                        canonical.display()
                    ),
                ));
                continue;
            }
            Err(error) => {
                warnings.push(skipped_warning(
                    "source-identity",
                    path,
                    format!("could not canonicalize it: {error}"),
                ));
                continue;
            }
        };
        let identity = match physical_identity(path, &canonical) {
            Ok(identity) => identity,
            Err(error) => {
                warnings.push(skipped_warning(
                    "source-identity",
                    path,
                    format!("could not establish physical identity: {error}"),
                ));
                continue;
            }
        };
        let containing_packages = owners
            .iter()
            .filter(|owner| path.starts_with(&owner.root))
            .map(|owner| owner.package.id.clone())
            .collect::<BTreeSet<_>>();
        if let Some(record) = records.get_mut(&identity) {
            record.aliases.insert(path.to_path_buf());
            record.containing_packages.extend(containing_packages);
            if path < record.representative_path.as_path() {
                record.representative_path = path.to_path_buf();
            }
            continue;
        }
        match cache.load(path, &canonical) {
            Ok(bytes) => {
                records.insert(
                    identity.clone(),
                    FileRecord {
                        identity,
                        representative_path: path.to_path_buf(),
                        aliases: BTreeSet::from([path.to_path_buf()]),
                        containing_packages,
                        bytes,
                        disposition: FileDisposition::Pending,
                    },
                );
            }
            Err(error) => warnings.push(skipped_warning(
                "source-unreadable",
                path,
                format!("could not read it: {error}"),
            )),
        }
    }

    let mut files = records.into_values().collect::<Vec<_>>();
    files.sort_by(|left, right| left.representative_path.cmp(&right.representative_path));
    let packages = package_projection(&owners, &files);

    warnings.sort();
    cache.finish_refresh();
    Ok(GenericSourceInventory {
        root: RootSourceInventory { root, files },
        packages,
        warnings,
    })
}

struct PackageOwner<'a> {
    project_root: &'a Path,
    package: &'a ConfiguredPackage,
    root: PathBuf,
}

impl<'a> PackageOwner<'a> {
    fn new(project_root: &'a Path, package: &'a ConfiguredPackage) -> Result<Self, AppError> {
        let root = package
            .manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .canonicalize()
            .map_err(|source| AppError::CanonicalPath {
                path: package.manifest_path.clone(),
                source,
            })?;
        Ok(Self {
            project_root,
            package,
            root,
        })
    }
}

fn package_projection(
    owners: &[PackageOwner<'_>],
    records: &[FileRecord],
) -> Vec<GenericPackageSources> {
    let mut files = vec![Vec::new(); owners.len()];
    for record in records
        .iter()
        .filter(|record| !is_rust_source(&record.representative_path))
    {
        let Some(owner_index) = owners
            .iter()
            .position(|owner| record.containing_packages.contains(&owner.package.id))
        else {
            continue;
        };
        if has_nested_package_boundary(&record.representative_path, &owners[owner_index].root) {
            continue;
        }
        files[owner_index].push(GenericSource {
            path: record.representative_path.clone(),
            bytes: Arc::clone(&record.bytes),
        });
    }
    owners
        .iter()
        .zip(files)
        .filter(|(_, files)| !files.is_empty())
        .map(|(owner, files)| GenericPackageSources {
            project_root: owner.project_root.to_path_buf(),
            id: owner.package.id.clone(),
            name: owner.package.name.clone(),
            manifest_path: owner.package.manifest_path.clone(),
            files,
        })
        .collect()
}

fn has_nested_package_boundary(path: &Path, owner_root: &Path) -> bool {
    path.parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|ancestor| *ancestor != owner_root)
        .any(|ancestor| ancestor.join("Cargo.toml").is_file())
}

fn is_structural_directory(path: &Path, root: &Path) -> bool {
    path != root
        && path.is_dir()
        && matches!(
            path.file_name().and_then(OsStr::to_str),
            Some(".cargo-sloc" | ".git" | ".hg" | ".svn")
        )
}

#[cfg(unix)]
fn physical_identity(path: &Path, _canonical: &Path) -> Result<PhysicalFileId, std::io::Error> {
    use std::os::unix::fs::MetadataExt;

    let metadata = path.metadata()?;
    Ok(PhysicalFileId::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

/// Resolves the invocation-wide physical identity for an existing file.
pub(crate) fn physical_identity_for_path(path: &Path) -> Result<PhysicalFileId, std::io::Error> {
    let canonical = path.canonicalize()?;
    physical_identity(path, &canonical)
}

#[cfg(not(unix))]
fn physical_identity(_path: &Path, canonical: &Path) -> Result<PhysicalFileId, std::io::Error> {
    Ok(PhysicalFileId::Canonical(canonical.to_path_buf()))
}

fn is_rust_source(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("rs"))
}

fn skipped_warning(code: &str, path: &Path, reason: String) -> Warning {
    Warning {
        code: code.to_owned(),
        message: format!(
            "skipped generic source candidate `{}` because {reason}",
            path.display()
        ),
    }
}
