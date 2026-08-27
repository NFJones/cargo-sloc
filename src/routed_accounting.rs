//! Central physical-file ownership, route selection, and checked aggregation.
//!
//! This module is the only boundary allowed to turn root-ledger records into
//! accounting contributions. It enforces one physical identity, one Scope,
//! one accounting route, and one contribution before report aggregation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use crate::accountant::{
    AccountingEngine, AccountingPrecision, AccountingRoute, FileContribution, LanguageId, ScopeId,
};
use crate::configuration::{ConfiguredInventory, ConfiguredPackage};
use crate::error::AppError;
use crate::generic_source::{
    FileDisposition, FileRecord, GenericSourceInventory, PhysicalFileId, physical_identity_for_path,
};
use crate::model::RootFilePolicy;
use crate::rust_source::{PackageSources, ReachableSource, SourceInventory};

/// Fully validated per-file accounting for one invocation.
#[derive(Clone, Debug, Default)]
pub struct RoutedAccounting {
    /// Exactly one contribution for every accounted ledger identity.
    pub contributions: Vec<FileContribution>,
}

#[derive(Clone, Copy)]
struct RustClaim<'a> {
    package: &'a PackageSources,
    source: &'a ReachableSource,
}

#[derive(Clone)]
struct PackageInfo<'a> {
    project_root: &'a Path,
    package: &'a ConfiguredPackage,
    root: PathBuf,
}

enum RustWork<'a> {
    Configured {
        record: &'a crate::generic_source::FileRecord,
        scope: ScopeId,
        claims: Vec<RustClaim<'a>>,
    },
    Unconfigured {
        record: &'a crate::generic_source::FileRecord,
        scope: ScopeId,
        path: &'a Path,
    },
}

/// Resolves ownership and accounts every eligible root-ledger record once.
pub(crate) fn resolve(
    root_files: RootFilePolicy,
    configured: &ConfiguredInventory,
    sources: &SourceInventory,
    inventory: &GenericSourceInventory,
    tokei_cache: &mut crate::tokei_accounting::AccountingCache,
) -> Result<RoutedAccounting, AppError> {
    let packages = package_info(configured)?;
    let claims = rust_claims(sources)?;
    let records = reconcile_rust_claims(inventory, &claims, &packages);
    let mut eligible_records = Vec::new();
    let mut excluded_ledger_ids = BTreeSet::new();
    for record in records {
        let file_claims = claims
            .get(&record.identity)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let scope = resolve_scope(
            &inventory.root.root,
            &record.containing_packages,
            file_claims,
            &packages,
        )?;
        if matches!(
            scope,
            ScopeId::Package { ref id, .. }
                if packages
                    .iter()
                    .any(|package| package.package.id == *id && package.package.targets.is_empty())
        ) {
            excluded_ledger_ids.insert(record.identity.clone());
            continue;
        }
        if root_files == RootFilePolicy::Exclude && matches!(scope, ScopeId::Root { .. }) {
            excluded_ledger_ids.insert(record.identity.clone());
            continue;
        }
        eligible_records.push((record, scope));
    }
    let ledger_ids = eligible_records
        .iter()
        .map(|(record, _)| record.identity.clone())
        .collect::<BTreeSet<_>>();
    for identity in claims.keys() {
        if !ledger_ids.contains(identity) && !excluded_ledger_ids.contains(identity) {
            return Err(AppError::ReportInvariant(format!(
                "reachable Rust identity {identity:?} is absent from the Root source inventory"
            )));
        }
    }

    let mut rust_work = Vec::new();
    let mut generic_work = Vec::new();
    for (record, scope) in &eligible_records {
        let file_claims = claims
            .get(&record.identity)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if !file_claims.is_empty() {
            rust_work.push(RustWork::Configured {
                record,
                scope: scope.clone(),
                claims: file_claims.to_vec(),
            });
        } else if let Some(path) = record.aliases.iter().find(|path| is_rust(path)) {
            rust_work.push(RustWork::Unconfigured {
                record,
                scope: scope.clone(),
                path,
            });
        } else if let Some(path) = record
            .aliases
            .iter()
            .find(|path| crate::tokei_accounting::recognize(path, &record.bytes).is_some())
        {
            generic_work.push((record, scope.clone(), path));
        }
    }

    let worker_count = rust_work.len().div_ceil(8).clamp(1, 8);
    crate::metrics::record_accounting_workers(worker_count);
    let mut contributions = account_rust_work(&rust_work, worker_count)?;

    crate::tokei_accounting::AccountingCache::begin_refresh(tokei_cache);
    let generic_result = generic_work
        .into_iter()
        .map(|(record, scope, path)| {
            let result =
                crate::tokei_accounting::account_file_with_cache(path, &record.bytes, tokei_cache)?;
            Ok(result.map(|(language, counts)| FileContribution {
                identity: record.identity.clone(),
                scope,
                route: AccountingRoute::Tokei(language),
                language,
                engine: AccountingEngine::Tokei,
                precision: AccountingPrecision::Lexical,
                counts,
            }))
        })
        .collect::<Result<Vec<_>, AppError>>();
    crate::tokei_accounting::AccountingCache::finish_refresh(tokei_cache);
    contributions.extend(generic_result?.into_iter().flatten());
    contributions.sort_by(|left, right| left.identity.cmp(&right.identity));
    validate_partition(&ledger_ids, &contributions)?;
    Ok(RoutedAccounting { contributions })
}

fn reconcile_rust_claims(
    inventory: &GenericSourceInventory,
    claims: &BTreeMap<PhysicalFileId, Vec<RustClaim<'_>>>,
    packages: &[PackageInfo<'_>],
) -> Vec<FileRecord> {
    let mut records = inventory
        .root
        .files
        .iter()
        .cloned()
        .map(|record| (record.identity.clone(), record))
        .collect::<BTreeMap<_, _>>();
    for (identity, file_claims) in claims {
        for claim in file_claims {
            let path = &claim.source.path;
            if !path.starts_with(&inventory.root.root) {
                continue;
            }
            let containing_packages = packages
                .iter()
                .filter(|package| path.starts_with(&package.root))
                .map(|package| package.package.id.clone())
                .collect::<BTreeSet<_>>();
            if let Some(record) = records.get_mut(identity) {
                record.merge_alias(path.clone(), containing_packages);
            } else {
                records.insert(
                    identity.clone(),
                    FileRecord {
                        identity: identity.clone(),
                        representative_path: path.clone(),
                        aliases: BTreeSet::from([path.clone()]),
                        containing_packages,
                        bytes: Arc::clone(&claim.source.bytes),
                        disposition: FileDisposition::Pending,
                    },
                );
            }
        }
    }
    let mut records = records.into_values().collect::<Vec<_>>();
    records.sort_by(|left, right| left.representative_path.cmp(&right.representative_path));
    records
}

fn account_rust_work(
    work: &[RustWork<'_>],
    worker_count: usize,
) -> Result<Vec<FileContribution>, AppError> {
    if worker_count <= 1 {
        return work.iter().map(account_rust_file).collect();
    }
    let chunk_size = work.len().div_ceil(worker_count);
    let partials = thread::scope(|scope| {
        work.chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .map(account_rust_file)
                        .collect::<Result<Vec<_>, _>>()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|worker| {
                worker.join().map_err(|_| {
                    AppError::ReportInvariant("routed accounting worker panicked".to_owned())
                })?
            })
            .collect::<Result<Vec<_>, AppError>>()
    })?;
    Ok(partials.into_iter().flatten().collect())
}

fn account_rust_file(work: &RustWork<'_>) -> Result<FileContribution, AppError> {
    match work {
        RustWork::Configured {
            record,
            scope,
            claims,
        } => {
            let pairs = claims
                .iter()
                .map(|claim| (claim.package, claim.source))
                .collect::<Vec<_>>();
            let mut counts = crate::rust_accounting::account_claims(&pairs)?;
            counts.files = 1;
            Ok(FileContribution {
                identity: record.identity.clone(),
                scope: scope.clone(),
                route: AccountingRoute::ConfiguredRust,
                language: LanguageId::RUST,
                engine: AccountingEngine::Rust,
                precision: AccountingPrecision::ConfigurationAware,
                counts,
            })
        }
        RustWork::Unconfigured {
            record,
            scope,
            path,
        } => Ok(FileContribution {
            identity: record.identity.clone(),
            scope: scope.clone(),
            route: AccountingRoute::UnconfiguredRust,
            language: LanguageId::RUST_UNCONFIGURED,
            engine: AccountingEngine::Rust,
            precision: AccountingPrecision::Unconfigured,
            counts: crate::rust_accounting::account_unconfigured(path, &record.bytes)?,
        }),
    }
}

fn package_info(configured: &ConfiguredInventory) -> Result<Vec<PackageInfo<'_>>, AppError> {
    configured
        .projects
        .iter()
        .flat_map(|project| {
            project.packages.iter().map(|package| {
                let root = package
                    .manifest_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .canonicalize()
                    .map_err(|source| AppError::CanonicalPath {
                        path: package.manifest_path.clone(),
                        source,
                    })?;
                Ok(PackageInfo {
                    project_root: &project.root,
                    package,
                    root,
                })
            })
        })
        .collect()
}

fn rust_claims<'a>(
    sources: &'a SourceInventory,
) -> Result<BTreeMap<PhysicalFileId, Vec<RustClaim<'a>>>, AppError> {
    let mut claims = BTreeMap::<PhysicalFileId, Vec<RustClaim<'a>>>::new();
    for package in &sources.packages {
        for source in &package.files {
            let identity = physical_identity_for_path(&source.path).map_err(|error| {
                AppError::ReportInvariant(format!(
                    "could not establish physical identity for reachable Rust source `{}`: {error}",
                    source.path.display()
                ))
            })?;
            claims
                .entry(identity)
                .or_default()
                .push(RustClaim { package, source });
        }
    }
    Ok(claims)
}

fn resolve_scope(
    root: &Path,
    containing_packages: &BTreeSet<String>,
    claims: &[RustClaim<'_>],
    packages: &[PackageInfo<'_>],
) -> Result<ScopeId, AppError> {
    let containing = packages
        .iter()
        .filter(|info| containing_packages.contains(&info.package.id))
        .max_by(|left, right| {
            left.root
                .components()
                .count()
                .cmp(&right.root.components().count())
                .then_with(|| right.package.id.cmp(&left.package.id))
        });
    if let Some(package) = containing {
        return Ok(package_scope(package));
    }
    let claimants = claims
        .iter()
        .map(|claim| claim.package.id.as_str())
        .collect::<BTreeSet<_>>();
    if claimants.len() == 1 {
        let id = *claimants.first().expect("single claimant exists");
        let package = packages
            .iter()
            .find(|package| package.package.id == id)
            .ok_or_else(|| {
                AppError::ReportInvariant(format!(
                    "Rust claimant Package `{id}` is absent from configured inventory"
                ))
            })?;
        return Ok(package_scope(package));
    }
    Ok(ScopeId::Root {
        path: root.to_path_buf(),
    })
}

fn package_scope(package: &PackageInfo<'_>) -> ScopeId {
    ScopeId::Package {
        id: package.package.id.clone(),
        name: package.package.name.clone(),
        manifest_path: package.package.manifest_path.clone(),
        project_root: package.project_root.to_path_buf(),
    }
}

fn validate_partition(
    ledger_ids: &BTreeSet<PhysicalFileId>,
    contributions: &[FileContribution],
) -> Result<(), AppError> {
    let mut seen = BTreeSet::new();
    for contribution in contributions {
        if !ledger_ids.contains(&contribution.identity) {
            return Err(AppError::ReportInvariant(format!(
                "accounting contribution {:?} is absent from the Root source inventory",
                contribution.identity
            )));
        }
        if !seen.insert(contribution.identity.clone()) {
            return Err(AppError::ReportInvariant(format!(
                "physical identity {:?} produced multiple accounting contributions",
                contribution.identity
            )));
        }
        if contribution.counts.files != 1 {
            return Err(AppError::ReportInvariant(format!(
                "physical identity {:?} contributed {} files instead of one",
                contribution.identity, contribution.counts.files
            )));
        }
    }
    Ok(())
}

fn is_rust(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;
    use crate::accountant::{AccountingRoute, ScopeId};
    use crate::cli::ParseOutcome;
    use crate::generic_source::{GenericSourceInventory, PhysicalFileId};
    use crate::model::{Counts, TestCount};

    #[test]
    fn routes_standalone_rust_and_generic_files_to_root() {
        let root = TempDir::new().expect("create Root");
        write(
            root.path().join("standalone.rs"),
            "// note\nfn standalone() {}\n",
        );
        write(root.path().join("tool.py"), "print('root')\n");
        let configured = ConfiguredInventory::default();
        let sources = SourceInventory::default();
        let inventory = crate::generic_source::discover_root(
            root.path(),
            &configured,
            crate::tokei_accounting::is_candidate_path,
        )
        .expect("discover root ledger");
        let canonical_root = root.path().canonicalize().expect("canonical Root");
        let mut cache = crate::tokei_accounting::AccountingCache::default();

        let accounting = resolve(
            RootFilePolicy::Include,
            &configured,
            &sources,
            &inventory,
            &mut cache,
        )
        .expect("resolve root accounting");

        assert_eq!(accounting.contributions.len(), 2);
        assert!(accounting.contributions.iter().all(|contribution| {
            matches!(contribution.scope, ScopeId::Root { ref path } if path == &canonical_root)
        }));
        let rust = accounting
            .contributions
            .iter()
            .find(|contribution| contribution.language == LanguageId::RUST_UNCONFIGURED)
            .expect("unconfigured Rust contribution");
        assert_eq!(rust.route, AccountingRoute::UnconfiguredRust);
        assert_eq!(rust.precision, AccountingPrecision::Unconfigured);
        assert_eq!(rust.counts.files, 1);
        assert_eq!(rust.counts.lines, 2);
        assert_eq!(rust.counts.comments, 1);
        assert_eq!(rust.counts.code, 1);
        assert_eq!(rust.counts.test, TestCount::Unavailable);
        assert!(accounting.contributions.iter().any(|contribution| {
            matches!(contribution.route, AccountingRoute::Tokei(_))
                && contribution.language.display_name() == "Python"
        }));
    }

    #[test]
    fn deepest_package_owns_unreachable_rust_without_claiming_configuration() {
        let root = TempDir::new().expect("create Root");
        write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"outer\", \"outer/nested\"]\nresolver = \"3\"\n",
        );
        package(root.path().join("outer"), "outer", "pub fn outer() {}\n");
        package(
            root.path().join("outer/nested"),
            "nested",
            "pub fn nested() {}\n",
        );
        write(
            root.path().join("outer/nested/orphan.rs"),
            "fn orphan() {}\n",
        );

        let (configured, sources, inventory) = pipeline(root.path());
        let mut cache = crate::tokei_accounting::AccountingCache::default();
        let accounting = resolve(
            RootFilePolicy::Include,
            &configured,
            &sources,
            &inventory,
            &mut cache,
        )
        .expect("resolve nested package accounting");
        let orphan = accounting
            .contributions
            .iter()
            .find(|contribution| {
                contribution.language == LanguageId::RUST_UNCONFIGURED
                    && matches!(
                        contribution.scope,
                        ScopeId::Package { ref name, .. } if name == "nested"
                    )
            })
            .expect("nested orphan contribution");

        assert_eq!(orphan.route, AccountingRoute::UnconfiguredRust);
        assert_eq!(orphan.counts.test, TestCount::Unavailable);
    }

    #[test]
    fn reachable_ignored_rust_module_is_reconciled_into_root_inventory() {
        let root = TempDir::new().expect("create Root");
        package(
            root.path().to_path_buf(),
            "ignored-module",
            "mod helper;\npub fn library() {}\n",
        );
        write(
            root.path().join(".ignore"),
            "src/helper.rs\nsrc/unused.rs\n",
        );
        write(root.path().join("src/helper.rs"), "pub fn helper() {}\n");
        write(root.path().join("src/unused.rs"), "pub fn unused() {}\n");

        let (configured, sources, inventory) = pipeline(root.path());
        let identity = physical_identity_for_path(&root.path().join("src/helper.rs"))
            .expect("ignored helper physical identity");
        let unused_identity = physical_identity_for_path(&root.path().join("src/unused.rs"))
            .expect("ignored unused physical identity");
        assert!(
            inventory
                .root
                .files
                .iter()
                .all(|record| record.identity != identity && record.identity != unused_identity)
        );
        let mut cache = crate::tokei_accounting::AccountingCache::default();

        let accounting = resolve(
            RootFilePolicy::Include,
            &configured,
            &sources,
            &inventory,
            &mut cache,
        )
        .expect("resolve reachable ignored Rust module");
        let contributions = accounting
            .contributions
            .iter()
            .filter(|contribution| contribution.identity == identity)
            .collect::<Vec<_>>();

        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].route, AccountingRoute::ConfiguredRust);
        assert_eq!(contributions[0].counts.files, 1);
        assert_eq!(contributions[0].counts.lines, 1);
        assert!(
            accounting
                .contributions
                .iter()
                .all(|contribution| contribution.identity != unused_identity)
        );
    }

    #[test]
    fn multiply_claimed_out_of_package_rust_uses_one_root_contribution() {
        let root = TempDir::new().expect("create Root");
        write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"b\"]\nresolver = \"3\"\n",
        );
        for name in ["a", "b"] {
            package(
                root.path().join(name),
                name,
                "#[path = \"../../shared.rs\"]\nmod shared;\n",
            );
        }
        write(root.path().join("shared.rs"), "pub fn shared() {}\n");

        let (configured, sources, inventory) = pipeline(root.path());
        let mut cache = crate::tokei_accounting::AccountingCache::default();
        let accounting = resolve(
            RootFilePolicy::Include,
            &configured,
            &sources,
            &inventory,
            &mut cache,
        )
        .expect("resolve shared source accounting");
        let shared_identity =
            crate::generic_source::physical_identity_for_path(&root.path().join("shared.rs"))
                .expect("shared physical identity");
        let shared = accounting
            .contributions
            .iter()
            .filter(|contribution| contribution.identity == shared_identity)
            .collect::<Vec<_>>();

        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].route, AccountingRoute::ConfiguredRust);
        assert!(matches!(shared[0].scope, ScopeId::Root { .. }));
        assert_eq!(shared[0].counts.files, 1);
        assert_eq!(shared[0].counts.lines, 1);
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_scope_comes_from_the_representative_alias() {
        let root = TempDir::new().expect("create Root");
        write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"a\", \"z/nested\"]\nresolver = \"3\"\n",
        );
        package(root.path().join("a"), "a", "pub fn a() {}\n");
        package(
            root.path().join("z/nested"),
            "nested",
            "pub fn nested() {}\n",
        );
        write(root.path().join("a/source.js"), "const shared = true;\n");
        fs::hard_link(
            root.path().join("a/source.js"),
            root.path().join("z/nested/source.js"),
        )
        .expect("create cross-package hard link");

        let (configured, sources, inventory) = pipeline(root.path());
        let identity = physical_identity_for_path(&root.path().join("a/source.js"))
            .expect("shared physical identity");
        let record = inventory
            .root
            .files
            .iter()
            .find(|record| record.identity == identity)
            .expect("shared ledger record");
        assert_eq!(
            record.representative_path,
            root.path()
                .join("a/source.js")
                .canonicalize()
                .expect("canonical representative path")
        );
        let mut cache = crate::tokei_accounting::AccountingCache::default();

        let accounting = resolve(
            RootFilePolicy::Include,
            &configured,
            &sources,
            &inventory,
            &mut cache,
        )
        .expect("resolve cross-package hard link");
        let contribution = accounting
            .contributions
            .iter()
            .find(|contribution| contribution.identity == identity)
            .expect("shared contribution");

        assert!(matches!(
            contribution.scope,
            ScopeId::Package { ref name, .. } if name == "a"
        ));
        assert_eq!(contribution.counts.files, 1);
    }

    #[cfg(unix)]
    #[test]
    fn configured_rust_claim_overrides_non_rust_hard_link_representative() {
        let root = TempDir::new().expect("create Root");
        package(
            root.path().to_path_buf(),
            "hard-linked-rust",
            "pub fn library() {}\n",
        );
        fs::hard_link(root.path().join("src/lib.rs"), root.path().join("a.py"))
            .expect("create non-Rust hard-link alias");

        let (configured, sources, inventory) = pipeline(root.path());
        let identity = physical_identity_for_path(&root.path().join("src/lib.rs"))
            .expect("hard-linked Rust physical identity");
        let record = inventory
            .root
            .files
            .iter()
            .find(|record| record.identity == identity)
            .expect("hard-linked source ledger record");
        assert_eq!(
            record.representative_path,
            root.path()
                .join("a.py")
                .canonicalize()
                .expect("canonical representative path")
        );
        let mut cache = crate::tokei_accounting::AccountingCache::default();

        let accounting = resolve(
            RootFilePolicy::Include,
            &configured,
            &sources,
            &inventory,
            &mut cache,
        )
        .expect("resolve hard-linked Rust accounting");
        let contributions = accounting
            .contributions
            .iter()
            .filter(|contribution| contribution.identity == identity)
            .collect::<Vec<_>>();

        assert_eq!(contributions.len(), 1);
        assert_eq!(contributions[0].route, AccountingRoute::ConfiguredRust);
        assert_eq!(contributions[0].language, LanguageId::RUST);
        assert_eq!(contributions[0].counts.files, 1);
    }

    #[cfg(unix)]
    #[test]
    fn unconfigured_rust_hard_link_uses_a_nonrepresentative_rust_alias() {
        let root = TempDir::new().expect("create Root");
        write(root.path().join("a"), "pub fn rust() {}\n");
        fs::hard_link(root.path().join("a"), root.path().join("z.rs"))
            .expect("create Rust hard-link alias");

        let configured = ConfiguredInventory::default();
        let sources = SourceInventory::default();
        let inventory = crate::generic_source::discover_root(
            root.path(),
            &configured,
            crate::tokei_accounting::is_candidate_path,
        )
        .expect("discover hard-linked source");
        let mut cache = crate::tokei_accounting::AccountingCache::default();

        let accounting = resolve(
            RootFilePolicy::Include,
            &configured,
            &sources,
            &inventory,
            &mut cache,
        )
        .expect("resolve hard-linked Rust accounting");

        assert_eq!(accounting.contributions.len(), 1);
        assert_eq!(
            accounting.contributions[0].route,
            AccountingRoute::UnconfiguredRust
        );
    }

    #[cfg(unix)]
    #[test]
    fn generic_hard_link_uses_a_nonrepresentative_recognized_alias() {
        let root = TempDir::new().expect("create Root");
        write(root.path().join("a"), "print('python')\n");
        fs::hard_link(root.path().join("a"), root.path().join("z.py"))
            .expect("create Python hard-link alias");

        let configured = ConfiguredInventory::default();
        let sources = SourceInventory::default();
        let inventory = crate::generic_source::discover_root(
            root.path(),
            &configured,
            crate::tokei_accounting::is_candidate_path,
        )
        .expect("discover hard-linked source");
        let mut cache = crate::tokei_accounting::AccountingCache::default();

        let accounting = resolve(
            RootFilePolicy::Include,
            &configured,
            &sources,
            &inventory,
            &mut cache,
        )
        .expect("resolve hard-linked Python accounting");

        assert_eq!(accounting.contributions.len(), 1);
        assert!(matches!(
            accounting.contributions[0].route,
            AccountingRoute::Tokei(_)
        ));
        assert_eq!(
            accounting.contributions[0].language.display_name(),
            "Python"
        );
    }

    #[test]
    fn partition_rejects_duplicate_and_unknown_contributions() {
        let known = PhysicalFileId::Canonical(PathBuf::from("known"));
        let unknown = PhysicalFileId::Canonical(PathBuf::from("unknown"));
        let contribution = |identity| FileContribution {
            identity,
            scope: ScopeId::Root {
                path: PathBuf::from("/root"),
            },
            route: AccountingRoute::UnconfiguredRust,
            language: LanguageId::RUST_UNCONFIGURED,
            engine: AccountingEngine::Rust,
            precision: AccountingPrecision::Unconfigured,
            counts: Counts {
                files: 1,
                test: TestCount::Unavailable,
                ..Counts::default()
            },
        };

        let ledger = BTreeSet::from([known.clone()]);
        assert!(
            validate_partition(&ledger, &[contribution(known.clone()), contribution(known)])
                .is_err()
        );
        assert!(validate_partition(&ledger, &[contribution(unknown)]).is_err());
    }

    fn pipeline(root: &Path) -> (ConfiguredInventory, SourceInventory, GenericSourceInventory) {
        let selection = match crate::cli::parse(
            [OsString::from("--json"), root.as_os_str().to_owned()],
            root,
        )
        .expect("parse selection")
        {
            ParseOutcome::Selection(selection) => selection,
            ParseOutcome::EarlyExit { .. } => panic!("unexpected early exit"),
        };
        let discovered = crate::discovery::discover(&selection).expect("discover Cargo Projects");
        let configured = crate::configuration::resolve(&selection, &discovered)
            .expect("resolve Cargo configuration");
        let sources = crate::rust_source::discover(&configured).expect("discover Rust source");
        let inventory = crate::generic_source::discover_root(
            root,
            &configured,
            crate::tokei_accounting::is_candidate_path,
        )
        .expect("discover root ledger");
        (configured, sources, inventory)
    }

    fn package(root: PathBuf, name: &str, source: &str) {
        write(
            root.join("Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        );
        write(root.join("src/lib.rs"), source);
    }

    fn write(path: PathBuf, contents: &str) {
        fs::create_dir_all(path.parent().expect("fixture parent"))
            .expect("create fixture directory");
        fs::write(path, contents).expect("write fixture file");
    }
}
