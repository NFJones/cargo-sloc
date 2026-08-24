//! Context-sensitive first-party Rust source graph discovery.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use ra_ap_syntax::{Edition, ast};

use crate::configuration::{BuildContext, ConfiguredInventory, ConfiguredPackage};
use crate::error::AppError;
use crate::model::{CfgOption, ContextKind};
use crate::report::Warning;
use crate::rust_analysis::{EvaluatedFile, FileAnalysis};

/// Context-reachable Rust source for all selected Packages.
#[derive(Clone, Debug, Default)]
pub struct SourceInventory {
    /// Package source graphs in deterministic Project/Package order.
    pub packages: Vec<PackageSources>,
    /// Nonfatal source-identity diagnostics.
    pub warnings: Vec<Warning>,
}

/// Context-reachable Rust files owned by one selected Package.
#[derive(Clone, Debug)]
pub struct PackageSources {
    /// Canonical root of the owning Cargo Project.
    pub project_root: PathBuf,
    /// Cargo's opaque Package ID.
    pub id: String,
    /// Cargo Package name.
    pub name: String,
    /// Absolute Package manifest path.
    pub manifest_path: PathBuf,
    /// Interned source-semantic contexts referenced by reachable files.
    pub semantic_contexts: Vec<SemanticContextKey>,
    /// Unique reachable physical files.
    pub files: Vec<ReachableSource>,
}

impl PackageSources {
    /// Resolves one package-local semantic context identifier.
    #[must_use]
    pub fn semantic_context(&self, id: SemanticContextId) -> Option<&SemanticContextKey> {
        self.semantic_contexts.get(id.0)
    }
}

/// Package-local identifier for one interned source-semantic context.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SemanticContextId(pub(crate) usize);

/// Inputs that can change Rust source reachability, diagnostics, or accounting.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SemanticContextKey {
    pub(crate) edition: String,
    pub(crate) cfg_options: BTreeSet<CfgOption>,
    pub(crate) recognized_cfg_names: BTreeSet<String>,
    pub(crate) recognized_features: BTreeSet<String>,
    pub(crate) provenance: ContextKind,
    pub(crate) harness: bool,
}

impl SemanticContextKey {
    fn new(context: &BuildContext, edition: &str) -> Self {
        Self {
            edition: edition.to_owned(),
            cfg_options: context.cfg_options.clone(),
            recognized_cfg_names: context.recognized_cfg_names.clone(),
            recognized_features: context.recognized_features.clone(),
            provenance: context.provenance,
            harness: context.harness,
        }
    }

    /// Returns whether this context contributes production or test-only code.
    #[must_use]
    pub fn provenance(&self) -> ContextKind {
        self.provenance
    }

    /// Returns whether Cargo enables a generated harness in this context.
    #[must_use]
    pub fn harness(&self) -> bool {
        self.harness
    }
}

/// One unique physical source file and its applicable Build Contexts.
#[derive(Clone, Debug)]
pub struct ReachableSource {
    /// Canonical identity, or a normalized absolute fallback.
    pub path: PathBuf,
    /// Interned semantic contexts in which this file is reachable.
    pub contexts: BTreeSet<SemanticContextId>,
    /// Owned analysis keyed by Rust edition.
    pub(crate) analyses: BTreeMap<String, Arc<FileAnalysis>>,
    /// Context-specific projections evaluated from the owned analysis.
    pub(crate) evaluations: BTreeMap<SemanticContextId, Arc<EvaluatedFile>>,
}

/// Discovers Rust source reachable from all configured target roots.
pub fn discover(configured: &ConfiguredInventory) -> Result<SourceInventory, AppError> {
    let mut cache = SourceCache::default();
    discover_with_cache(configured, &mut cache)
}

/// Discovers Rust source while reusing content-validated owned file analyses.
pub(crate) fn discover_with_cache(
    configured: &ConfiguredInventory,
    cache: &mut SourceCache,
) -> Result<SourceInventory, AppError> {
    cache.begin_refresh();
    let mut warnings = Vec::new();
    let mut packages = Vec::new();

    for project in &configured.projects {
        for package in &project.packages {
            packages.push(discover_package(
                &project.root,
                package,
                cache,
                &mut warnings,
            )?);
        }
    }
    warn_about_unknown_cfgs(&packages, &mut warnings)?;
    warn_about_shared_sources(&packages, &mut warnings);
    warnings.sort();
    cache.finish_refresh();
    Ok(SourceInventory { packages, warnings })
}

fn warn_about_unknown_cfgs(
    packages: &[PackageSources],
    warnings: &mut Vec<Warning>,
) -> Result<(), AppError> {
    for package in packages {
        let mut unknown = BTreeSet::new();
        for source in &package.files {
            for evaluation in source.evaluations.values() {
                unknown.extend(evaluation.unknown_cfgs.iter().cloned());
            }
        }
        warnings.extend(unknown.into_iter().map(|predicate| Warning {
            code: "unknown-cfg".to_owned(),
            message: format!(
                "cfg predicate `{predicate}` in Package `{}` may depend on an unmodeled build script or compiler flag",
                package.name
            ),
        }));
    }
    Ok(())
}

fn warn_about_shared_sources(packages: &[PackageSources], warnings: &mut Vec<Warning>) {
    let mut owners = BTreeMap::<&Path, BTreeMap<(&Path, &str), (&str, &Path)>>::new();
    for package in packages {
        for source in &package.files {
            owners.entry(&source.path).or_default().insert(
                (package.project_root.as_path(), package.id.as_str()),
                (package.name.as_str(), package.manifest_path.as_path()),
            );
        }
    }
    for (path, packages) in owners {
        if packages.len() > 1 {
            let labels = packages
                .into_values()
                .map(|(name, manifest)| format!("`{name}` ({})", manifest.display()))
                .collect::<Vec<_>>()
                .join(", ");
            warnings.push(Warning {
                code: "source-shared-between-packages".to_owned(),
                message: format!(
                    "source `{}` is reachable from selected Packages {}",
                    path.display(),
                    labels
                ),
            });
        }
    }
}

fn discover_package(
    project_root: &Path,
    package: &ConfiguredPackage,
    cache: &mut SourceCache,
    warnings: &mut Vec<Warning>,
) -> Result<PackageSources, AppError> {
    let mut queue = BTreeMap::<WorkKey, BTreeSet<SemanticContextId>>::new();
    let mut semantic_contexts = Vec::new();
    let mut semantic_context_ids = BTreeMap::new();
    for target in &package.targets {
        for context in package.contexts.iter().filter(|context| {
            context.target_name == target.name && context.target_kind == target.kind
        }) {
            let semantic_context = SemanticContextKey::new(context, &target.edition);
            let semantic_context_id = *semantic_context_ids
                .entry(semantic_context.clone())
                .or_insert_with(|| {
                    let id = SemanticContextId(semantic_contexts.len());
                    semantic_contexts.push(semantic_context);
                    id
                });
            let source_path = absolute_path(&target.source_path)?;
            let source_parent = source_path.parent().unwrap_or(Path::new(".")).to_path_buf();
            queue
                .entry(WorkKey {
                    path: source_path,
                    edition: target.edition.clone(),
                    default_module_base: source_parent,
                })
                .or_default()
                .insert(semantic_context_id);
        }
    }

    let mut visited = BTreeSet::new();
    let mut files = BTreeMap::<PathBuf, ReachableSource>::new();
    while let Some((work, context_ids)) = queue.pop_first() {
        let identity = source_identity(&work.path, warnings)?;
        let pending = context_ids
            .into_iter()
            .filter(|context| {
                visited.insert(VisitKey {
                    path: identity.clone(),
                    context: *context,
                    default_module_base: work.default_module_base.clone(),
                })
            })
            .collect::<Vec<_>>();
        if pending.is_empty() {
            continue;
        }

        let parsed = cache.parse(&work.path, &identity, &work.edition)?;
        let contexts = pending
            .iter()
            .map(|id| {
                semantic_contexts
                    .get(id.0)
                    .map(|context| (*id, context))
                    .ok_or_else(|| {
                        AppError::ReportInvariant(format!(
                            "work item references missing semantic context {}",
                            id.0
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let evaluated = parsed.analysis.evaluate_many(&contexts, &work.path)?;
        let reachable = files
            .entry(identity.clone())
            .or_insert_with(|| ReachableSource {
                path: identity.clone(),
                contexts: BTreeSet::new(),
                analyses: BTreeMap::new(),
                evaluations: BTreeMap::new(),
            });
        reachable
            .analyses
            .entry(work.edition.clone())
            .or_insert_with(|| Arc::clone(&parsed.analysis));
        for (context, evaluation) in evaluated {
            let evaluation = Arc::new(evaluation);
            reachable.contexts.insert(context);
            reachable
                .evaluations
                .insert(context, Arc::clone(&evaluation));
            enqueue_external_modules(&work, context, &evaluation, &mut queue, cache)?;
        }
    }

    Ok(PackageSources {
        project_root: project_root.to_path_buf(),
        id: package.id.clone(),
        name: package.name.clone(),
        manifest_path: package.manifest_path.clone(),
        semantic_contexts,
        files: files.into_values().collect(),
    })
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WorkKey {
    path: PathBuf,
    edition: String,
    default_module_base: PathBuf,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct VisitKey {
    path: PathBuf,
    context: SemanticContextId,
    default_module_base: PathBuf,
}

#[derive(Default)]
pub(crate) struct SourceCache {
    parsed: BTreeMap<(PathBuf, String), ParsedSource>,
    touched: BTreeSet<(PathBuf, String)>,
    validated_unchanged: BTreeSet<PathBuf>,
    validated_bytes: BTreeMap<PathBuf, Arc<[u8]>>,
    dependencies: BTreeSet<PathBuf>,
}

#[derive(Clone)]
struct ParsedSource {
    bytes: Arc<[u8]>,
    analysis: Arc<FileAnalysis>,
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
        self.parsed.retain(|key, _| self.touched.contains(key));
    }

    fn parse(
        &mut self,
        actual_path: &Path,
        identity: &Path,
        edition: &str,
    ) -> Result<ParsedSource, AppError> {
        let key = (identity.to_path_buf(), edition.to_owned());
        self.touched.insert(key.clone());
        self.dependencies.insert(actual_path.to_path_buf());
        if self.validated_unchanged.contains(actual_path)
            && let Some(parsed) = self.parsed.get(&key)
        {
            crate::metrics::record_cache(
                crate::metrics::Cache::Parse,
                crate::metrics::CacheOutcome::Hit,
                "validated-unchanged-source",
            );
            return Ok(parsed.clone());
        }
        let bytes = self.validated_bytes.remove(actual_path).map_or_else(
            || {
                fs::read(actual_path)
                    .map(Arc::<[u8]>::from)
                    .map_err(|source| AppError::SourceRead {
                        path: actual_path.to_path_buf(),
                        source,
                    })
            },
            Ok,
        )?;
        if let Some(parsed) = self
            .parsed
            .get(&key)
            .filter(|parsed| parsed.bytes.as_ref() == bytes.as_ref())
        {
            crate::metrics::record_cache(
                crate::metrics::Cache::Parse,
                crate::metrics::CacheOutcome::Hit,
                "physical-source-and-edition",
            );
            return Ok(parsed.clone());
        }
        crate::metrics::record_cache(
            crate::metrics::Cache::Parse,
            crate::metrics::CacheOutcome::Miss,
            "physical-source-and-edition",
        );
        let source = std::str::from_utf8(&bytes).map_err(|error| AppError::SourceEncoding {
            path: actual_path.to_path_buf(),
            message: error.to_string(),
        })?;
        let edition_value = Edition::from_str(edition).map_err(|error| AppError::SourceParse {
            path: actual_path.to_path_buf(),
            edition: edition.to_owned(),
            message: error.to_string(),
        })?;
        let parse_input = source.strip_prefix('\u{feff}').unwrap_or(source);
        let parse = ast::SourceFile::parse(parse_input, edition_value);
        if !parse.errors().is_empty() {
            return Err(AppError::SourceParse {
                path: actual_path.to_path_buf(),
                edition: edition.to_owned(),
                message: parse
                    .errors()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            });
        }
        let analysis = Arc::new(FileAnalysis::lower(&parse.tree(), source, actual_path)?);
        let parsed = ParsedSource { bytes, analysis };
        self.parsed.insert(key, parsed.clone());
        Ok(parsed)
    }
}

fn enqueue_external_modules(
    work: &WorkKey,
    context: SemanticContextId,
    evaluation: &EvaluatedFile,
    queue: &mut BTreeMap<WorkKey, BTreeSet<SemanticContextId>>,
    cache: &mut SourceCache,
) -> Result<(), AppError> {
    for module in &evaluation.modules {
        let mut default_base = work.default_module_base.clone();
        let mut explicit_base = if module.inline_components.is_empty() {
            work.path.parent().unwrap_or(Path::new(".")).to_path_buf()
        } else {
            work.default_module_base.clone()
        };
        for component in &module.inline_components {
            default_base.push(component);
            explicit_base.push(component);
        }
        let resolved = resolve_module_file(
            &work.path,
            &module.name,
            &default_base,
            &explicit_base,
            module.explicit_path.as_deref(),
            &mut cache.dependencies,
        )?;
        let child_base = child_module_base(&resolved);
        queue
            .entry(WorkKey {
                path: resolved,
                edition: work.edition.clone(),
                default_module_base: child_base,
            })
            .or_default()
            .insert(context);
    }
    Ok(())
}

fn resolve_module_file(
    source: &Path,
    module: &str,
    default_base: &Path,
    explicit_base: &Path,
    explicit: Option<&str>,
    dependencies: &mut BTreeSet<PathBuf>,
) -> Result<PathBuf, AppError> {
    if let Some(explicit) = explicit {
        let candidate = explicit_base.join(explicit);
        dependencies.insert(candidate.clone());
        if candidate.is_file() {
            return Ok(candidate);
        }
        return Err(AppError::ModuleNotFound {
            module: module.to_owned(),
            declaring_source: source.to_path_buf(),
            candidates: candidate.display().to_string(),
        });
    }
    let flat = default_base.join(format!("{module}.rs"));
    let nested = default_base.join(module).join("mod.rs");
    dependencies.insert(flat.clone());
    dependencies.insert(nested.clone());
    match (flat.is_file(), nested.is_file()) {
        (true, false) => Ok(flat),
        (false, true) => Ok(nested),
        (true, true) => Err(AppError::AmbiguousModule {
            module: module.to_owned(),
            declaring_source: source.to_path_buf(),
            first: flat,
            second: nested,
        }),
        (false, false) => Err(AppError::ModuleNotFound {
            module: module.to_owned(),
            declaring_source: source.to_path_buf(),
            candidates: format!("`{}` or `{}`", flat.display(), nested.display()),
        }),
    }
}

fn child_module_base(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("."));
    if path.file_name().is_some_and(|name| name == "mod.rs") {
        parent.to_path_buf()
    } else {
        path.file_stem()
            .map_or_else(|| parent.to_path_buf(), |stem| parent.join(stem))
    }
}

fn source_identity(path: &Path, warnings: &mut Vec<Warning>) -> Result<PathBuf, AppError> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(_) => {
            let path = absolute_path(path)?;
            warnings.push(Warning {
                code: "source-canonicalization-failed".to_owned(),
                message: format!(
                    "could not canonicalize readable source `{}`; using normalized absolute identity",
                    path.display()
                ),
            });
            Ok(path)
        }
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, AppError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(AppError::CurrentDirectory)?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}
