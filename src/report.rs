//! Language-neutral report records and deterministic renderers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use comfy_table::{
    Cell, CellAlignment, Row, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL_CONDENSED,
};
use serde::{Deserialize, Serialize};

use crate::accountant::Language;
use crate::configuration::ConfiguredInventory;
use crate::error::AppError;
use crate::model::{BuildRole, ContextKind, Counts, Selection};
use crate::rust_accounting::AccountingInventory;

/// One stable nonfatal diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Warning {
    /// Stable machine-readable warning code.
    pub code: String,
    /// Human-readable warning message.
    pub message: String,
}

/// One Package/language aggregation row.
#[derive(Clone, Debug)]
pub struct PackageRow {
    /// Stable display label.
    pub label: String,
    /// Cargo Package name.
    pub name: String,
    /// Cargo's opaque Package ID.
    pub package_id: String,
    /// Absolute owning Project root.
    pub project_root: PathBuf,
    /// Absolute Package manifest path.
    pub manifest_path: PathBuf,
    /// Accounted language.
    pub language: Language,
    /// Common measures.
    pub counts: Counts,
}

/// Complete report model shared by the terminal table and JSON.
#[derive(Clone, Debug)]
pub struct Report {
    /// Normalized invocation selection.
    pub selection: Selection,
    /// Deterministically ordered Package rows.
    pub packages: Vec<PackageRow>,
    /// Arithmetic sum of Package rows.
    pub total: Counts,
    /// Deterministically ordered warnings.
    pub warnings: Vec<Warning>,
    /// Project-local host and effective compilation targets.
    project_targets: Vec<ProjectTargets>,
    /// Owning Project root keyed by Cargo Package ID.
    package_projects: BTreeMap<String, PathBuf>,
    /// Context-specific effective feature provenance.
    feature_contexts: Vec<FeatureContext>,
}

impl Report {
    /// Creates the successful empty report used before discovery is implemented.
    pub fn empty(selection: Selection) -> Self {
        Self {
            selection,
            packages: Vec::new(),
            total: Counts::default(),
            warnings: Vec::new(),
            project_targets: Vec::new(),
            package_projects: BTreeMap::new(),
            feature_contexts: Vec::new(),
        }
    }

    /// Adds resolved Project target provenance to the report model.
    pub fn apply_configuration(
        &mut self,
        configured: &ConfiguredInventory,
    ) -> Result<(), AppError> {
        self.package_projects.clear();
        self.feature_contexts.clear();
        self.project_targets = configured
            .projects
            .iter()
            .map(|project| ProjectTargets {
                project_root: project.root.clone(),
                host_target: project.host_target.clone(),
                targets: project
                    .targets
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            })
            .collect();
        for project in &configured.projects {
            for package in &project.packages {
                if let Some(previous) = self
                    .package_projects
                    .insert(package.id.clone(), project.root.clone())
                    && previous != project.root
                {
                    return Err(AppError::ReportInvariant(format!(
                        "Package `{}` belongs to both `{}` and `{}`",
                        package.id,
                        previous.display(),
                        project.root.display()
                    )));
                }
                for context in &package.contexts {
                    self.feature_contexts.push(FeatureContext {
                        project_root: project.root.clone(),
                        package_id: package.id.clone(),
                        package_name: package.name.clone(),
                        target_name: context.target_name.clone(),
                        target_kind: context.target_kind.selector_name().to_owned(),
                        build_role: match context.role {
                            BuildRole::Host => "host",
                            BuildRole::Target => "target",
                        },
                        provenance: match context.provenance {
                            ContextKind::Production => "production",
                            ContextKind::Test => "test",
                        },
                        compilation_target: context.compilation_target.clone(),
                        crate_type: context.crate_type.clone(),
                        features: context.features.iter().cloned().collect(),
                    });
                }
            }
        }
        self.project_targets
            .sort_by(|left, right| left.project_root.cmp(&right.project_root));
        self.feature_contexts.sort();
        Ok(())
    }

    /// Adds Package-level language counts produced by Accountants.
    pub fn apply_accounting(&mut self, accounting: &AccountingInventory) -> Result<(), AppError> {
        let duplicate_names = accounting
            .packages
            .iter()
            .filter(|package| package.counts.files > 0)
            .fold(BTreeMap::<&str, usize>::new(), |mut names, package| {
                *names.entry(&package.name).or_default() += 1;
                names
            });
        self.packages = accounting
            .packages
            .iter()
            .filter(|package| package.counts.files > 0)
            .map(|package| {
                let project_root = self.package_projects.get(&package.id).ok_or_else(|| {
                    AppError::ReportInvariant(format!(
                        "Package `{}` has counts but no owning Project",
                        package.id
                    ))
                })?;
                Ok(PackageRow {
                    label: package_label(
                        &self.selection,
                        package,
                        duplicate_names
                            .get(package.name.as_str())
                            .copied()
                            .unwrap_or(0)
                            > 1,
                    ),
                    name: package.name.clone(),
                    package_id: package.id.clone(),
                    project_root: project_root.clone(),
                    manifest_path: package.manifest_path.clone(),
                    language: Language::Rust,
                    counts: package.counts,
                })
            })
            .collect::<Result<_, AppError>>()?;
        self.packages.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then_with(|| left.language.cmp(&right.language))
                .then_with(|| left.package_id.cmp(&right.package_id))
        });
        self.total = self
            .packages
            .iter()
            .try_fold(Counts::default(), |total, row| {
                total.checked_add(row.counts)
            })?;
        Ok(())
    }

    /// Renders the selected output format into a complete byte buffer.
    pub fn render(&self) -> Result<Vec<u8>, AppError> {
        if self.selection.json {
            self.render_json()
        } else {
            Ok(self.render_table().into_bytes())
        }
    }

    fn render_table(&self) -> String {
        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL_CONDENSED)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_header(Row::from(vec![
                text_cell("Package"),
                text_cell("Language"),
                numeric_cell("Files"),
                numeric_cell("Lines"),
                numeric_cell("Blanks"),
                numeric_cell("Comments"),
                numeric_cell("Code"),
                numeric_cell("Test"),
            ]));
        for row in &self.packages {
            table.add_row(report_table_row(
                &row.label,
                &row.language.to_string(),
                row.counts,
            ));
        }
        table.add_row(report_table_row("Total", "All", self.total));

        let mut output = table.to_string();
        while output.ends_with('\n') {
            output.pop();
        }
        output.push('\n');
        output
    }

    fn render_json(&self) -> Result<Vec<u8>, AppError> {
        let host_targets = self
            .project_targets
            .iter()
            .map(|project| project.host_target.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let targets = self
            .project_targets
            .iter()
            .flat_map(|project| project.targets.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let project_targets = self
            .project_targets
            .iter()
            .map(ProjectTargets::to_json)
            .collect::<Result<Vec<_>, _>>()?;
        let feature_contexts = self
            .feature_contexts
            .iter()
            .map(FeatureContext::to_json)
            .collect::<Result<Vec<_>, _>>()?;
        let packages = self
            .packages
            .iter()
            .map(PackageRow::to_json)
            .collect::<Result<Vec<_>, _>>()?;
        let configuration = JsonConfiguration {
            package_selectors: self.selection.package_selectors.iter().collect(),
            workspace: self.selection.workspace,
            package_exclude_selectors: self.selection.package_exclude_selectors.iter().collect(),
            host_targets,
            targets,
            project_targets,
            feature_contexts,
            all_features: self.selection.all_features,
            no_default_features: self.selection.no_default_features,
            features: self.selection.features.iter().collect(),
            target_includes: self.selection.target_includes.iter().collect(),
            target_excludes: self.selection.target_excludes.iter().collect(),
            requested_targets: self.selection.requested_targets.iter().collect(),
        };
        let document = JsonReport {
            schema_version: 1,
            root: self.selection.root.json_string()?,
            configuration,
            packages,
            total: self.total,
            warnings: &self.warnings,
        };
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

fn package_label(
    selection: &Selection,
    package: &crate::rust_accounting::PackageAccounting,
    duplicate: bool,
) -> String {
    if !duplicate {
        return package.name.clone();
    }
    let qualifier = package
        .manifest_path
        .parent()
        .and_then(|path| path.strip_prefix(selection.root.as_path()).ok())
        .filter(|path| !path.as_os_str().is_empty())
        .and_then(Path::to_str)
        .unwrap_or(&package.id);
    format!("{} ({qualifier})", package.name)
}

fn report_table_row(label: &str, language: &str, counts: Counts) -> Row {
    Row::from(vec![
        text_cell(sanitize_table_cell(label)),
        text_cell(language),
        numeric_cell(counts.files),
        numeric_cell(counts.lines),
        numeric_cell(counts.blanks),
        numeric_cell(counts.comments),
        numeric_cell(counts.code),
        numeric_cell(counts.test),
    ])
}

fn text_cell(value: impl ToString) -> Cell {
    Cell::new(value).set_alignment(CellAlignment::Left)
}

fn numeric_cell(value: impl ToString) -> Cell {
    Cell::new(value).set_alignment(CellAlignment::Right)
}

fn sanitize_table_cell(value: &str) -> String {
    use std::fmt::Write;

    let mut sanitized = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\r' => sanitized.push_str("\\r"),
            '\n' => sanitized.push_str("\\n"),
            '\t' => sanitized.push_str("\\t"),
            character if character.is_control() => {
                write!(sanitized, "\\u{{{:04X}}}", u32::from(character))
                    .expect("writing to a String cannot fail");
            }
            character => sanitized.push(character),
        }
    }
    sanitized
}

#[derive(Serialize)]
struct JsonReport<'a> {
    schema_version: u8,
    root: String,
    configuration: JsonConfiguration<'a>,
    packages: Vec<JsonPackageRow>,
    total: Counts,
    warnings: &'a [Warning],
}

#[derive(Serialize)]
struct JsonConfiguration<'a> {
    package_selectors: Vec<&'a String>,
    workspace: bool,
    package_exclude_selectors: Vec<&'a String>,
    host_targets: Vec<String>,
    targets: Vec<String>,
    project_targets: Vec<JsonProjectTargets>,
    feature_contexts: Vec<JsonFeatureContext>,
    all_features: bool,
    no_default_features: bool,
    features: Vec<&'a String>,
    target_includes: Vec<&'a String>,
    target_excludes: Vec<&'a String>,
    requested_targets: Vec<&'a String>,
}

#[derive(Clone, Debug)]
struct ProjectTargets {
    project_root: PathBuf,
    host_target: String,
    targets: Vec<String>,
}

impl ProjectTargets {
    fn to_json(&self) -> Result<JsonProjectTargets, AppError> {
        Ok(JsonProjectTargets {
            project_root: json_path(&self.project_root)?,
            host_target: self.host_target.clone(),
            targets: self.targets.clone(),
        })
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct FeatureContext {
    project_root: PathBuf,
    package_id: String,
    package_name: String,
    target_name: String,
    target_kind: String,
    build_role: &'static str,
    provenance: &'static str,
    compilation_target: String,
    crate_type: String,
    features: Vec<String>,
}

impl FeatureContext {
    fn to_json(&self) -> Result<JsonFeatureContext, AppError> {
        Ok(JsonFeatureContext {
            project_root: json_path(&self.project_root)?,
            package_id: self.package_id.clone(),
            package_name: self.package_name.clone(),
            target_name: self.target_name.clone(),
            target_kind: self.target_kind.clone(),
            build_role: self.build_role,
            provenance: self.provenance,
            compilation_target: self.compilation_target.clone(),
            crate_type: self.crate_type.clone(),
            features: self.features.clone(),
        })
    }
}

impl PackageRow {
    fn to_json(&self) -> Result<JsonPackageRow, AppError> {
        Ok(JsonPackageRow {
            name: self.name.clone(),
            package_id: self.package_id.clone(),
            project_root: json_path(&self.project_root)?,
            manifest_path: json_path(&self.manifest_path)?,
            language: self.language.to_string(),
            counts: self.counts,
        })
    }
}

fn json_path(path: &Path) -> Result<String, AppError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| AppError::NonUtf8JsonPath(path.to_path_buf()))
}

#[derive(Serialize)]
struct JsonPackageRow {
    name: String,
    package_id: String,
    project_root: String,
    manifest_path: String,
    language: String,
    #[serde(flatten)]
    counts: Counts,
}

#[derive(Serialize)]
struct JsonProjectTargets {
    project_root: String,
    host_target: String,
    targets: Vec<String>,
}

#[derive(Serialize)]
struct JsonFeatureContext {
    project_root: String,
    package_id: String,
    package_name: String,
    target_name: String,
    target_kind: String,
    build_role: &'static str,
    provenance: &'static str,
    compilation_target: String,
    crate_type: String,
    features: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_cell_sanitization_is_stable() {
        assert_eq!(sanitize_table_cell(r"a\b|c"), r"a\b|c");
        assert_eq!(
            sanitize_table_cell("\0\t\n\r\u{001b}\u{007f}\u{0085}λ"),
            r"\u{0000}\t\n\r\u{001B}\u{007F}\u{0085}λ"
        );

        let controls = (0..=0x1f)
            .chain(0x7f..=0x9f)
            .filter_map(char::from_u32)
            .collect::<String>();
        assert!(
            sanitize_table_cell(&controls)
                .chars()
                .all(|value| !value.is_control())
        );
    }
}
