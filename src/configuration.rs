//! Project-local Cargo feature, target, toolchain, and cfg resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_config2::{Config, PathAndArgs, ResolveOptions};
use cargo_toml::{Manifest, Resolver};
use guppy::graph::cargo::{BuildPlatform, CargoOptions, CargoResolverVersion};
use guppy::graph::feature::{FeatureId, StandardFeatures, feature_id_filter};
use guppy::platform::{Platform, TargetFeatures};
use guppy::{CargoMetadata, PackageId};
use serde::{Deserialize, Serialize};

use crate::discovery::{
    Inventory, ProjectInventory, TargetContext, TargetInventory, TargetKind,
    target_selector_matches,
};
use crate::error::AppError;
use crate::model::{BuildRole, CfgOption, ContextKind, Selection};
use crate::report::Warning;

/// Inventory augmented with immutable Build Contexts.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ConfiguredInventory {
    /// Configured Projects in deterministic root order.
    pub projects: Vec<ConfiguredProject>,
    /// In-Root workspace Package roots excluded by the current selection.
    pub unselected_package_roots: Vec<PathBuf>,
    /// Configuration warnings.
    pub warnings: Vec<Warning>,
}

/// Project-local toolchain and target configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConfiguredProject {
    /// Canonical Project root.
    pub root: PathBuf,
    /// Host target reported by the selected toolchain.
    pub host_target: String,
    /// Effective target names after Cargo precedence.
    pub targets: Vec<String>,
    /// Selected Packages and their Build Contexts.
    pub packages: Vec<ConfiguredPackage>,
}

/// One selected Package with all applicable Build Contexts.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConfiguredPackage {
    /// Cargo's opaque package ID.
    pub id: String,
    /// Cargo package name.
    pub name: String,
    /// Absolute Package manifest path.
    pub manifest_path: PathBuf,
    /// Selected target roots from discovery.
    pub targets: Vec<TargetInventory>,
    /// Deduplicated Build Contexts.
    pub contexts: Vec<BuildContext>,
}

/// One exact source-selection context.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BuildContext {
    /// Cargo Target name.
    pub target_name: String,
    /// Cargo Target kind.
    pub target_kind: TargetKind,
    /// Host- or target-built role.
    pub role: BuildRole,
    /// Report provenance.
    pub provenance: ContextKind,
    /// Target triple or custom-target identity used by rustc.
    pub compilation_target: String,
    /// Crate type used for built-in cfg probing.
    pub crate_type: String,
    /// Context-specific enabled Package features.
    pub features: BTreeSet<String>,
    /// Active conditional-compilation options.
    pub cfg_options: BTreeSet<CfgOption>,
    /// Recognized built-in and toolchain cfg names, including inactive names.
    pub recognized_cfg_names: BTreeSet<String>,
    /// Recognized built-in and toolchain cfg name/value pairs, including inactive values.
    pub recognized_cfg_options: BTreeSet<CfgOption>,
    /// Recognized feature cfg values, including disabled declarations.
    pub recognized_features: BTreeSet<String>,
    /// Whether Cargo selects a generated harness.
    pub harness: bool,
}

/// Resolves Build Contexts for every selected Project.
pub fn resolve(
    selection: &Selection,
    inventory: &Inventory,
) -> Result<ConfiguredInventory, AppError> {
    let mut projects = Vec::new();
    let mut warnings = Vec::new();
    let mut matched_features = BTreeSet::new();

    for project in &inventory.projects {
        projects.push(resolve_project(
            selection,
            project,
            &mut warnings,
            &mut matched_features,
        )?);
    }
    for feature in &selection.features {
        if !matched_features.contains(feature) {
            return Err(AppError::UnmatchedFeature(feature.clone()));
        }
    }
    warnings.sort();
    Ok(ConfiguredInventory {
        projects,
        unselected_package_roots: inventory.unselected_package_roots.clone(),
        warnings,
    })
}

fn resolve_project(
    selection: &Selection,
    project: &ProjectInventory,
    warnings: &mut Vec<Warning>,
    matched_features: &mut BTreeSet<String>,
) -> Result<ConfiguredProject, AppError> {
    let initial_rustc = default_rustc();
    let initial_host = probe_host_target(&initial_rustc, &project.root)?;
    let mut config = Config::load_with_options(
        &project.root,
        ResolveOptions::default().host_triple(initial_host.clone()),
    )
    .map_err(|error| AppError::CargoConfiguration {
        project: project.root.clone(),
        message: error.to_string(),
    })?;
    let host_target = if config.rustc() == &initial_rustc {
        initial_host.clone()
    } else {
        probe_host_target(config.rustc(), &project.root)?
    };
    if host_target != initial_host {
        config = Config::load_with_options(
            &project.root,
            ResolveOptions::default().host_triple(host_target.clone()),
        )
        .map_err(|error| AppError::CargoConfiguration {
            project: project.root.clone(),
            message: error.to_string(),
        })?;
    }
    let requested: Vec<_> = selection.requested_targets.iter().collect();
    let targets = config.build_target_for_config(requested).map_err(|error| {
        AppError::CargoConfiguration {
            project: project.root.clone(),
            message: error.to_string(),
        }
    })?;

    let graph = CargoMetadata::parse_json(project.metadata.document())
        .and_then(CargoMetadata::build_graph)
        .map_err(|error| AppError::FeatureResolution {
            project: project.root.clone(),
            message: error.to_string(),
        })?;
    let resolver = resolver_version(project)?;
    let selected_ids: Vec<_> = project
        .packages
        .iter()
        .map(|package| PackageId::new(package.id.clone()))
        .collect();
    let package_set =
        graph
            .resolve_ids(&selected_ids)
            .map_err(|error| AppError::FeatureResolution {
                project: project.root.clone(),
                message: error.to_string(),
            })?;
    let feature_ids =
        requested_feature_ids(selection, project, &graph, &selected_ids, matched_features)?;
    let standard = if selection.all_features {
        StandardFeatures::All
    } else if selection.no_default_features {
        StandardFeatures::None
    } else {
        StandardFeatures::Default
    };
    let initials = package_set.to_feature_set(feature_id_filter(standard, feature_ids));
    let host_platform =
        Platform::new(host_target.clone(), TargetFeatures::Unknown).map_err(|error| {
            AppError::FeatureResolution {
                project: project.root.clone(),
                message: error.to_string(),
            }
        })?;

    let needs_dev = project.packages.iter().any(|package| {
        package.targets.iter().any(|target| {
            target
                .contexts
                .iter()
                .any(|context| *context != TargetContext::Production)
        })
    });
    let mut contexts_by_package = BTreeMap::<String, BTreeSet<BuildContext>>::new();
    let mut missing_by_named_target = BTreeMap::<(String, String), BTreeSet<String>>::new();
    let mut cfg_cache = BTreeMap::<(OsString, String), BTreeSet<CfgOption>>::new();

    for target in &targets {
        let target_platform = Platform::new(target.triple().to_owned(), TargetFeatures::Unknown)
            .map_err(|error| AppError::FeatureResolution {
                project: project.root.clone(),
                message: format!("target `{}`: {error}", target.triple()),
            })?;
        for include_dev in [false, true]
            .into_iter()
            .filter(|value| !*value || needs_dev)
        {
            let mut options = CargoOptions::new();
            options
                .set_resolver(resolver)
                .set_include_dev(include_dev)
                .set_host_platform(host_platform.clone())
                .set_target_platform(target_platform.clone());
            let cargo_set = initials.clone().into_cargo_set(&options).map_err(|error| {
                AppError::FeatureResolution {
                    project: project.root.clone(),
                    message: error.to_string(),
                }
            })?;

            for package in &project.packages {
                let guppy_id = PackageId::new(package.id.clone());
                let target_features = features_for(
                    cargo_set.platform_features(BuildPlatform::Target),
                    &guppy_id,
                    &project.root,
                )?;
                let host_features = features_for(
                    cargo_set.platform_features(BuildPlatform::Host),
                    &guppy_id,
                    &project.root,
                )?;
                for cargo_target in &package.targets {
                    for source_context in cargo_target
                        .contexts
                        .iter()
                        .filter(|context| (**context != TargetContext::Production) == include_dev)
                    {
                        let mut roles = Vec::new();
                        let proc_macro = cargo_target
                            .crate_types
                            .iter()
                            .any(|kind| kind == "proc-macro");
                        if cargo_target.kind == TargetKind::BuildScript {
                            if let Some(features) = &target_features {
                                roles.push((BuildRole::Host, features));
                            }
                        } else if proc_macro {
                            if let Some(features) = &host_features {
                                roles.push((BuildRole::Host, features));
                            }
                        } else {
                            if let Some(features) = &target_features {
                                roles.push((BuildRole::Target, features));
                            }
                            if cargo_target.kind == TargetKind::Lib
                                && *source_context == TargetContext::Production
                                && let Some(features) = &host_features
                            {
                                roles.push((BuildRole::Host, features));
                            }
                        }
                        for (role, features) in roles {
                            let missing: BTreeSet<_> = cargo_target
                                .required_features
                                .iter()
                                .filter(|feature| !features.contains(*feature))
                                .cloned()
                                .collect();
                            if !missing.is_empty() {
                                let selector = format!(
                                    "{}:{}",
                                    cargo_target.kind.selector_name(),
                                    cargo_target.name
                                );
                                if selection.target_includes.iter().any(|include| {
                                    target_selector_matches(
                                        include,
                                        cargo_target.kind,
                                        &cargo_target.name,
                                    )
                                }) {
                                    missing_by_named_target
                                        .entry((package.id.clone(), selector))
                                        .or_default()
                                        .extend(missing);
                                }
                                continue;
                            }
                            let (context_target, rustc_target) = match role {
                                BuildRole::Host => {
                                    (host_target.clone(), OsString::from(&host_target))
                                }
                                BuildRole::Target => (
                                    target.triple().to_owned(),
                                    target.spec_path().map_or_else(
                                        || OsString::from(target.triple()),
                                        |path| path.as_os_str().to_owned(),
                                    ),
                                ),
                            };
                            let crate_type = crate_type(cargo_target);
                            let key = (rustc_target.clone(), crate_type.clone());
                            let active_cfg = if let Some(cached) = cfg_cache.get(&key) {
                                crate::metrics::record_cache(
                                    crate::metrics::Cache::Cfg,
                                    crate::metrics::CacheOutcome::Hit,
                                    "target-and-crate-type",
                                );
                                cached.clone()
                            } else {
                                crate::metrics::record_cache(
                                    crate::metrics::Cache::Cfg,
                                    crate::metrics::CacheOutcome::Miss,
                                    "target-and-crate-type",
                                );
                                let probed =
                                    probe_cfg(&config, &project.root, &rustc_target, &crate_type)?;
                                cfg_cache.insert(key, probed.clone());
                                probed
                            };
                            let recognized_cfg_names = recognized_cfg_names(&active_cfg);
                            let recognized_cfg_options = recognized_cfg_options(&active_cfg);
                            let mut cfg_options = active_cfg;
                            for feature in features {
                                cfg_options.insert(CfgOption::KeyValue {
                                    name: "feature".to_owned(),
                                    value: feature.clone(),
                                });
                            }
                            if *source_context != TargetContext::Production && cargo_target.harness
                            {
                                cfg_options.insert(CfgOption::Name("test".to_owned()));
                            }
                            contexts_by_package
                                .entry(package.id.clone())
                                .or_default()
                                .insert(BuildContext {
                                    target_name: cargo_target.name.clone(),
                                    target_kind: cargo_target.kind,
                                    role,
                                    provenance: if *source_context == TargetContext::Production {
                                        ContextKind::Production
                                    } else {
                                        ContextKind::Test
                                    },
                                    compilation_target: context_target,
                                    crate_type,
                                    features: features.clone(),
                                    cfg_options,
                                    recognized_cfg_names,
                                    recognized_cfg_options,
                                    recognized_features: package.declared_features.clone(),
                                    harness: cargo_target.harness,
                                });
                        }
                    }
                }
            }
        }
    }

    let has_rustflags = has_unmodeled_rustflags(&config, &project.root);
    if has_rustflags {
        warnings.push(Warning {
            code: "unmodeled-rustflags".to_owned(),
            message: format!(
                "Project `{}` configures rustflags that may alter cfg values",
                project.root.display()
            ),
        });
    }
    let mut packages = Vec::new();
    for package in &project.packages {
        let contexts: Vec<_> = contexts_by_package
            .remove(&package.id)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let targets: Vec<_> = package
            .targets
            .iter()
            .filter(|target| {
                contexts.iter().any(|context| {
                    context.target_kind == target.kind && context.target_name == target.name
                })
            })
            .cloned()
            .collect();
        for target in &package.targets {
            let selector = format!("{}:{}", target.kind.selector_name(), target.name);
            if selection
                .target_includes
                .iter()
                .any(|include| target_selector_matches(include, target.kind, &target.name))
                && !targets.iter().any(|configured| {
                    configured.kind == target.kind && configured.name == target.name
                })
                && let Some(missing) =
                    missing_by_named_target.get(&(package.id.clone(), selector.clone()))
            {
                return Err(AppError::IneligibleNamedTarget {
                    target: selector,
                    features: missing.iter().cloned().collect::<Vec<_>>().join(", "),
                });
            }
        }
        packages.push(ConfiguredPackage {
            id: package.id.clone(),
            name: package.name.clone(),
            manifest_path: package.manifest_path.clone(),
            targets,
            contexts,
        });
    }
    Ok(ConfiguredProject {
        root: project.root.clone(),
        host_target,
        targets: targets
            .iter()
            .map(|target| target.triple().to_owned())
            .collect(),
        packages,
    })
}

fn resolver_version(project: &ProjectInventory) -> Result<CargoResolverVersion, AppError> {
    let manifest =
        Manifest::from_path(&project.manifest_path).map_err(|source| AppError::CargoManifest {
            manifest: project.manifest_path.clone(),
            source,
        })?;
    let resolver = manifest
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.resolver);
    Ok(match resolver {
        Some(Resolver::V2) => CargoResolverVersion::V2,
        Some(Resolver::V3) => CargoResolverVersion::V3,
        Some(Resolver::V1) => CargoResolverVersion::V1,
        None => match project.root_package_edition.as_deref() {
            Some("2024") => CargoResolverVersion::V3,
            Some("2021") => CargoResolverVersion::V2,
            _ => CargoResolverVersion::V1,
        },
    })
}

fn requested_feature_ids<'g>(
    selection: &'g Selection,
    project: &ProjectInventory,
    graph: &'g guppy::graph::PackageGraph,
    selected_ids: &'g [PackageId],
    matched: &mut BTreeSet<String>,
) -> Result<Vec<FeatureId<'g>>, AppError> {
    let mut ids = Vec::new();
    for requested in &selection.features {
        let (package_name, feature_name) = requested
            .split_once('/')
            .map_or((None, requested.as_str()), |(package, feature)| {
                (Some(package), feature)
            });
        let mut found = false;
        for (inventory_package, package_id) in project.packages.iter().zip(selected_ids) {
            if package_name.is_some_and(|name| name != inventory_package.name) {
                continue;
            }
            let metadata =
                graph
                    .metadata(package_id)
                    .map_err(|error| AppError::FeatureResolution {
                        project: project.root.clone(),
                        message: error.to_string(),
                    })?;
            if metadata.named_features().any(|name| name == feature_name) {
                ids.push(FeatureId::named(package_id, feature_name));
                found = true;
            }
        }
        if found {
            matched.insert(requested.clone());
        }
    }
    Ok(ids)
}

fn features_for(
    set: &guppy::graph::feature::FeatureSet<'_>,
    package_id: &PackageId,
    project: &Path,
) -> Result<Option<BTreeSet<String>>, AppError> {
    set.features_for(package_id)
        .map(|features| {
            features.map(|features| features.named_features().map(str::to_owned).collect())
        })
        .map_err(|error| AppError::FeatureResolution {
            project: project.to_path_buf(),
            message: error.to_string(),
        })
}

fn crate_type(target: &TargetInventory) -> String {
    if target.kind == TargetKind::BuildScript {
        "bin".to_owned()
    } else {
        target
            .crate_types
            .first()
            .cloned()
            .unwrap_or_else(|| "lib".to_owned())
    }
}

fn default_rustc() -> PathAndArgs {
    let path = std::env::var_os("RUSTC").map_or_else(
        || {
            let cargo = std::env::var_os("CARGO").map(PathBuf::from);
            cargo
                .and_then(|mut path| {
                    path.set_file_name(format!("rustc{}", std::env::consts::EXE_SUFFIX));
                    path.is_file().then_some(path)
                })
                .unwrap_or_else(|| PathBuf::from(format!("rustc{}", std::env::consts::EXE_SUFFIX)))
        },
        PathBuf::from,
    );
    PathAndArgs::new(path)
}

fn probe_host_target(rustc: &PathAndArgs, project: &Path) -> Result<String, AppError> {
    crate::metrics::record_query(crate::metrics::Query::RustcHost);
    let mut command = Command::new(&rustc.path);
    command.args(&rustc.args).arg("-vV").current_dir(project);
    let output = crate::process::run(
        &mut command,
        format!("rustc host query for `{}`", project.display()),
    )
    .map_err(|error| AppError::RustcQuery {
        project: project.to_path_buf(),
        message: error.to_string(),
    })?;
    if !output.status.success() {
        return Err(AppError::RustcQuery {
            project: project.to_path_buf(),
            message: format!(
                "rustc exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|error| AppError::RustcQuery {
        project: project.to_path_buf(),
        message: error.to_string(),
    })?;
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| AppError::RustcQuery {
            project: project.to_path_buf(),
            message: "rustc -vV output contained no host triple".to_owned(),
        })
}

fn has_unmodeled_rustflags(config: &Config, project: &Path) -> bool {
    if config
        .build
        .rustflags
        .as_ref()
        .is_some_and(|flags| !flags.flags.is_empty())
        || std::env::vars_os().any(|(name, value)| {
            let name = name.to_string_lossy();
            !value.is_empty()
                && (name == "RUSTFLAGS"
                    || name == "CARGO_ENCODED_RUSTFLAGS"
                    || name == "CARGO_BUILD_RUSTFLAGS"
                    || (name.starts_with("CARGO_TARGET_") && name.ends_with("_RUSTFLAGS")))
        })
    {
        return true;
    }

    project.ancestors().any(|ancestor| {
        [
            ancestor.join(".cargo/config.toml"),
            ancestor.join(".cargo/config"),
        ]
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .any(|contents| {
            contents.lines().any(|line| {
                line.split('#')
                    .next()
                    .and_then(|line| line.split_once('='))
                    .is_some_and(|(key, _)| key.trim().trim_matches(['\'', '"']) == "rustflags")
            })
        })
    })
}

fn probe_cfg(
    config: &Config,
    project: &Path,
    target: &OsString,
    crate_type: &str,
) -> Result<BTreeSet<CfgOption>, AppError> {
    crate::metrics::record_query(crate::metrics::Query::RustcCfg);
    let rustc = config.rustc();
    let mut command = Command::new(&rustc.path);
    command
        .args(&rustc.args)
        .args(["--print", "cfg", "--crate-type", crate_type, "--target"])
        .arg(target)
        .current_dir(project);
    let output = crate::process::run(
        &mut command,
        format!("rustc cfg query for `{}`", project.display()),
    )
    .map_err(|error| AppError::RustcQuery {
        project: project.to_path_buf(),
        message: error.to_string(),
    })?;
    if !output.status.success() {
        return Err(AppError::RustcQuery {
            project: project.to_path_buf(),
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    let stdout = std::str::from_utf8(&output.stdout).map_err(|error| AppError::RustcQuery {
        project: project.to_path_buf(),
        message: error.to_string(),
    })?;
    stdout.lines().map(parse_cfg_option).collect()
}

fn parse_cfg_option(line: &str) -> Result<CfgOption, AppError> {
    if let Some((name, value)) = line.split_once('=') {
        let value: String = serde_json::from_str(value).map_err(AppError::Json)?;
        Ok(CfgOption::KeyValue {
            name: name.to_owned(),
            value,
        })
    } else {
        Ok(CfgOption::Name(line.to_owned()))
    }
}

fn recognized_cfg_names(active: &BTreeSet<CfgOption>) -> BTreeSet<String> {
    let mut names = BTreeSet::from([
        "debug_assertions".to_owned(),
        "doc".to_owned(),
        "doctest".to_owned(),
        "feature".to_owned(),
        "overflow_checks".to_owned(),
        "panic".to_owned(),
        "proc_macro".to_owned(),
        "relocation_model".to_owned(),
        "sanitize".to_owned(),
        "target_abi".to_owned(),
        "target_arch".to_owned(),
        "target_endian".to_owned(),
        "target_env".to_owned(),
        "target_family".to_owned(),
        "target_feature".to_owned(),
        "target_has_atomic".to_owned(),
        "target_has_atomic_equal_alignment".to_owned(),
        "target_has_atomic_load_store".to_owned(),
        "target_os".to_owned(),
        "target_pointer_width".to_owned(),
        "target_thread_local".to_owned(),
        "target_vendor".to_owned(),
        "test".to_owned(),
        "ub_checks".to_owned(),
        "unix".to_owned(),
        "windows".to_owned(),
    ]);
    names.extend(active.iter().map(|option| match option {
        CfgOption::Name(name) | CfgOption::KeyValue { name, .. } => name.clone(),
    }));
    names
}

fn recognized_cfg_options(active: &BTreeSet<CfgOption>) -> BTreeSet<CfgOption> {
    let mut options = active
        .iter()
        .filter_map(|option| match option {
            CfgOption::Name(_) => None,
            CfgOption::KeyValue { .. } => Some(option.clone()),
        })
        .collect::<BTreeSet<_>>();
    options.extend(
        [
            "aix",
            "android",
            "bitrig",
            "darwin",
            "dragonfly",
            "emscripten",
            "espidf",
            "freebsd",
            "fuchsia",
            "haiku",
            "hermit",
            "horizon",
            "hurd",
            "illumos",
            "ios",
            "l4re",
            "linux",
            "macos",
            "netbsd",
            "none",
            "nto",
            "openbsd",
            "psp",
            "redox",
            "solaris",
            "solid_asp3",
            "teeos",
            "trusty",
            "tvos",
            "uefi",
            "unknown",
            "visionos",
            "vxworks",
            "wasi",
            "watchos",
            "windows",
            "xous",
        ]
        .into_iter()
        .map(|value| CfgOption::KeyValue {
            name: "target_os".to_owned(),
            value: value.to_owned(),
        }),
    );
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root_package_edition: Option<&str>, resolver: Option<&str>) -> ProjectInventory {
        let directory = tempfile::tempdir().expect("create project directory");
        let root = directory.keep();
        let package = root_package_edition.map_or(String::new(), |edition| {
            format!("[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"{edition}\"\n\n")
        });
        let workspace = resolver.map_or_else(
            || "[workspace]\nmembers = []\n".to_owned(),
            |resolver| format!("[workspace]\nmembers = []\nresolver = \"{resolver}\"\n"),
        );
        std::fs::write(root.join("Cargo.toml"), format!("{package}{workspace}"))
            .expect("write project manifest");
        ProjectInventory {
            root: root.clone(),
            manifest_path: root.join("Cargo.toml"),
            metadata: crate::discovery::ProjectMetadataSnapshot::default(),
            root_package_edition: root_package_edition.map(str::to_owned),
            packages: Vec::new(),
        }
    }

    #[test]
    fn implicit_resolver_follows_workspace_root_package_edition() {
        assert_eq!(
            resolver_version(&project(Some("2018"), None)).expect("resolver"),
            CargoResolverVersion::V1
        );
        assert_eq!(
            resolver_version(&project(Some("2021"), None)).expect("resolver"),
            CargoResolverVersion::V2
        );
        assert_eq!(
            resolver_version(&project(Some("2024"), None)).expect("resolver"),
            CargoResolverVersion::V3
        );
        assert_eq!(
            resolver_version(&project(None, None)).expect("resolver"),
            CargoResolverVersion::V1
        );
    }

    #[test]
    fn explicit_workspace_resolver_overrides_root_package_edition() {
        assert_eq!(
            resolver_version(&project(Some("2024"), Some("1"))).expect("resolver"),
            CargoResolverVersion::V1
        );
        assert_eq!(
            resolver_version(&project(Some("2018"), Some("2"))).expect("resolver"),
            CargoResolverVersion::V2
        );
        assert_eq!(
            resolver_version(&project(Some("2018"), Some("3"))).expect("resolver"),
            CargoResolverVersion::V3
        );
    }
}
