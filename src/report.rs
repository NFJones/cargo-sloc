//! Language-neutral report records and deterministic renderers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use comfy_table::{Cell, CellAlignment, Row, Table, presets::UTF8_NO_BORDERS};
use serde::{Deserialize, Serialize};

use crate::accountant::{
    AccountingEngine, AccountingPrecision, AccountingRow, FileContribution, LanguageId, ScopeId,
};
use crate::configuration::ConfiguredInventory;
use crate::error::AppError;
use crate::model::{BuildRole, ContextKind, Counts, RootFilePolicy, Selection, TestCount};
use crate::rust_accounting::AccountingInventory;

/// Current public JSON report schema version.
pub(crate) const JSON_SCHEMA_VERSION: u8 = 3;

/// One stable nonfatal diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Warning {
    /// Stable machine-readable warning code.
    pub code: String,
    /// Human-readable warning message.
    pub message: String,
}

/// One Scope/language aggregation row.
#[derive(Clone, Debug)]
pub struct ScopeRow {
    /// Stable display label.
    pub label: String,
    /// Stable Package or Root identity.
    pub scope: ScopeId,
    /// Accounted language.
    pub language: LanguageId,
    /// Implementation that produced the row.
    pub engine: AccountingEngine,
    /// Semantic precision of the row.
    pub precision: AccountingPrecision,
    /// Common measures.
    pub counts: Counts,
}

/// Complete report model shared by the terminal table and JSON.
#[derive(Clone, Debug)]
pub struct Report {
    /// Normalized invocation selection.
    pub selection: Selection,
    /// Deterministically ordered Scope rows.
    pub packages: Vec<ScopeRow>,
    /// Arithmetic sum of Scope rows.
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
        let rows = accounting
            .packages
            .iter()
            .filter(|package| package.counts.files > 0)
            .map(|package| AccountingRow {
                package_id: package.id.clone(),
                package_name: package.name.clone(),
                manifest_path: package.manifest_path.clone(),
                language: LanguageId::RUST,
                engine: AccountingEngine::Rust,
                precision: AccountingPrecision::ConfigurationAware,
                counts: package.counts,
            })
            .collect::<Vec<_>>();
        self.apply_rows(&rows)
    }

    /// Replaces report rows with validated per-file routed contributions.
    pub(crate) fn apply_contributions(
        &mut self,
        contributions: &[FileContribution],
    ) -> Result<(), AppError> {
        let mut aggregated =
            BTreeMap::<(ScopeId, LanguageId, AccountingEngine, AccountingPrecision), Counts>::new();
        for contribution in contributions {
            if self.selection.root_files == RootFilePolicy::Exclude
                && matches!(contribution.scope, ScopeId::Root { .. })
            {
                continue;
            }
            let counts = aggregated
                .entry((
                    contribution.scope.clone(),
                    contribution.language,
                    contribution.engine,
                    contribution.precision,
                ))
                .or_default();
            *counts = counts.checked_add(contribution.counts)?;
        }
        self.apply_scope_counts(aggregated)
    }

    /// Replaces report rows with language-neutral Accountant output.
    pub fn apply_rows(&mut self, rows: &[AccountingRow]) -> Result<(), AppError> {
        let mut aggregated =
            BTreeMap::<(ScopeId, LanguageId, AccountingEngine, AccountingPrecision), Counts>::new();
        for row in rows.iter().filter(|row| row.counts.files > 0) {
            let project_root = self.package_projects.get(&row.package_id).ok_or_else(|| {
                AppError::ReportInvariant(format!(
                    "Package `{}` has counts but no owning Project",
                    row.package_id
                ))
            })?;
            let scope = ScopeId::Package {
                id: row.package_id.clone(),
                name: row.package_name.clone(),
                manifest_path: row.manifest_path.clone(),
                project_root: project_root.clone(),
            };
            let counts = aggregated
                .entry((scope, row.language, row.engine, row.precision))
                .or_default();
            *counts = counts.checked_add(row.counts)?;
        }
        self.apply_scope_counts(aggregated)
    }

    fn apply_scope_counts(
        &mut self,
        aggregated: BTreeMap<(ScopeId, LanguageId, AccountingEngine, AccountingPrecision), Counts>,
    ) -> Result<(), AppError> {
        let duplicate_names = aggregated
            .keys()
            .filter_map(|(scope, ..)| match scope {
                ScopeId::Package { id, name, .. } => Some((name.clone(), id.clone())),
                ScopeId::Root { .. } => None,
            })
            .fold(
                BTreeMap::<String, BTreeSet<String>>::new(),
                |mut names, (name, id)| {
                    names.entry(name).or_default().insert(id);
                    names
                },
            );
        self.packages = aggregated
            .into_iter()
            .filter(|(_, counts)| counts.files > 0)
            .map(|((scope, language, engine, precision), counts)| ScopeRow {
                label: scope_label(&self.selection, &scope, &duplicate_names),
                scope,
                language,
                engine,
                precision,
                counts,
            })
            .collect();
        disambiguate_sanitized_scope_labels(&mut self.packages);
        let scope_totals =
            self.packages
                .iter()
                .try_fold(BTreeMap::<ScopeId, u64>::new(), |mut totals, row| {
                    let total = totals.entry(row.scope.clone()).or_default();
                    *total = total.checked_add(row.counts.lines).ok_or_else(|| {
                        AppError::ReportInvariant(format!(
                            "Scope `{}` line total overflowed while sorting report rows",
                            row.label
                        ))
                    })?;
                    Ok::<_, AppError>(totals)
                })?;
        self.packages.sort_by(|left, right| {
            scope_totals[&right.scope]
                .cmp(&scope_totals[&left.scope])
                .then_with(|| left.label.cmp(&right.label))
                .then_with(|| left.scope.cmp(&right.scope))
                .then_with(|| right.counts.lines.cmp(&left.counts.lines))
                .then_with(|| left.language.cmp(&right.language))
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
        } else if self.selection.totals {
            self.render_totals_table().map(String::into_bytes)
        } else {
            self.render_table().map(String::into_bytes)
        }
    }

    fn render_table(&self) -> Result<String, AppError> {
        let mut table = Table::new();
        table
            .load_preset(UTF8_NO_BORDERS)
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
        let mut previous_scope = None;
        for row in &self.packages {
            let package_label = if previous_scope.as_ref() == Some(&row.scope) {
                ""
            } else {
                &row.label
            };
            table.add_row(report_table_row(
                package_label,
                &row.language.to_string(),
                row.counts,
            ));
            previous_scope = Some(row.scope.clone());
        }
        table.add_row(report_table_row("Total", "All", self.total));

        let mut output = table.to_string();
        output = remove_intra_scope_dividers(output, &self.packages);
        while output.ends_with('\n') {
            output.pop();
        }
        output.push('\n');
        Ok(output)
    }

    fn render_totals_table(&self) -> Result<String, AppError> {
        let mut totals = BTreeMap::<LanguageId, Counts>::new();
        for row in &self.packages {
            let counts = totals.entry(row.language).or_default();
            *counts = counts.checked_add(row.counts)?;
        }
        let mut rows = totals.into_iter().collect::<Vec<_>>();
        rows.sort_by(
            |(left_language, left_counts), (right_language, right_counts)| {
                right_counts
                    .lines
                    .cmp(&left_counts.lines)
                    .then_with(|| left_language.cmp(right_language))
            },
        );

        let mut table = Table::new();
        table
            .load_preset(UTF8_NO_BORDERS)
            .set_header(Row::from(vec![
                text_cell("Language"),
                numeric_cell("Files"),
                numeric_cell("Lines"),
                numeric_cell("Blanks"),
                numeric_cell("Comments"),
                numeric_cell("Code"),
                numeric_cell("Test"),
            ]));
        for (language, counts) in rows {
            table.add_row(totals_table_row(&language.to_string(), counts));
        }
        table.add_row(totals_table_row("Total", self.total));

        let mut output = table.to_string();
        while output.ends_with('\n') {
            output.pop();
        }
        output.push('\n');
        Ok(output)
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
        let rows = self
            .packages
            .iter()
            .map(ScopeRow::to_json)
            .collect::<Result<Vec<_>, _>>()?;
        let configuration = JsonConfiguration {
            package_selectors: self.selection.package_selectors.iter().collect(),
            workspace: self.selection.workspace,
            package_exclude_selectors: self.selection.package_exclude_selectors.iter().collect(),
            root_files: self.selection.root_files.as_str(),
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
            schema_version: JSON_SCHEMA_VERSION,
            root: self.selection.root.json_string()?,
            configuration,
            rows,
            total: self.total,
            warnings: &self.warnings,
        };
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

fn remove_intra_scope_dividers(mut table: String, packages: &[ScopeRow]) -> String {
    let mut lines = table.lines();
    let mut output = String::new();
    if let Some(header) = lines.next() {
        output.push_str(header);
        output.push('\n');
    }
    if let Some(header_divider) = lines.next() {
        output.push_str(header_divider);
        output.push('\n');
    }

    for (index, _) in packages.iter().enumerate() {
        if let Some(row) = lines.next() {
            output.push_str(row);
            output.push('\n');
        }
        let divider = lines.next();
        if packages
            .get(index + 1)
            .is_none_or(|next| next.scope != packages[index].scope)
            && let Some(divider) = divider
        {
            output.push_str(divider);
            output.push('\n');
        }
    }

    for line in lines {
        output.push_str(line);
        output.push('\n');
    }
    table = output;
    table
}

fn scope_label(
    selection: &Selection,
    scope: &ScopeId,
    duplicate_names: &BTreeMap<String, BTreeSet<String>>,
) -> String {
    match scope {
        ScopeId::Root { .. } => "<root>".to_owned(),
        ScopeId::Package {
            id,
            name,
            manifest_path,
            ..
        } => {
            if duplicate_names
                .get(name.as_str())
                .is_none_or(|ids| ids.len() <= 1)
            {
                return name.clone();
            }
            let qualifier = manifest_path
                .parent()
                .and_then(|path| path.strip_prefix(selection.root.as_path()).ok())
                .filter(|path| !path.as_os_str().is_empty())
                .and_then(Path::to_str)
                .unwrap_or(id);
            format!("{name} ({qualifier})")
        }
    }
}

fn disambiguate_sanitized_scope_labels(rows: &mut [ScopeRow]) {
    let mut scopes_by_label = BTreeMap::<String, BTreeSet<ScopeId>>::new();
    for row in rows.iter() {
        scopes_by_label
            .entry(sanitize_table_cell(&row.label))
            .or_default()
            .insert(row.scope.clone());
    }
    for row in rows {
        let visible = sanitize_table_cell(&row.label);
        if scopes_by_label
            .get(&visible)
            .is_some_and(|scopes| scopes.len() > 1)
        {
            row.label = format!("{visible} [{}]", encoded_scope_identity(&row.scope));
        }
    }
}

fn encoded_scope_identity(scope: &ScopeId) -> String {
    match scope {
        ScopeId::Root { .. } => "scope:root".to_owned(),
        ScopeId::Package { id, .. } => {
            let encoded = id
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>();
            format!("scope:package:{encoded}")
        }
    }
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
        numeric_cell(match counts.test {
            TestCount::Known(value) => value.to_string(),
            TestCount::Unavailable => "n/a".to_owned(),
        }),
    ])
}

fn totals_table_row(language: &str, counts: Counts) -> Row {
    Row::from(vec![
        text_cell(language),
        numeric_cell(counts.files),
        numeric_cell(counts.lines),
        numeric_cell(counts.blanks),
        numeric_cell(counts.comments),
        numeric_cell(counts.code),
        numeric_cell(match counts.test {
            TestCount::Known(value) => value.to_string(),
            TestCount::Unavailable => "n/a".to_owned(),
        }),
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
    rows: Vec<JsonScopeRow>,
    total: Counts,
    warnings: &'a [Warning],
}

#[derive(Serialize)]
struct JsonConfiguration<'a> {
    package_selectors: Vec<&'a String>,
    workspace: bool,
    package_exclude_selectors: Vec<&'a String>,
    root_files: &'static str,
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

impl ScopeRow {
    fn to_json(&self) -> Result<JsonScopeRow, AppError> {
        Ok(JsonScopeRow {
            scope: match &self.scope {
                ScopeId::Package {
                    id,
                    name,
                    project_root,
                    manifest_path,
                } => JsonScope::Package {
                    name: name.clone(),
                    package_id: id.clone(),
                    project_root: json_path(project_root)?,
                    manifest_path: json_path(manifest_path)?,
                },
                ScopeId::Root { .. } => JsonScope::Root { path: "." },
            },
            language: self.language.to_string(),
            accounting_engine: self.engine.as_str(),
            accounting_precision: self.precision.as_str(),
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
struct JsonScopeRow {
    scope: JsonScope,
    language: String,
    accounting_engine: &'static str,
    accounting_precision: &'static str,
    #[serde(flatten)]
    counts: Counts,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum JsonScope {
    Package {
        name: String,
        package_id: String,
        project_root: String,
        manifest_path: String,
    },
    Root {
        path: &'static str,
    },
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

    use crate::accountant::{AccountingEngine, AccountingPrecision, AccountingRow, LanguageId};
    use crate::model::TestCount;
    use tempfile::TempDir;

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

    #[test]
    fn mixed_rows_render_unavailable_test_without_duplicate_package_qualifier() {
        let root = TempDir::new().expect("create Root");
        let root_identity =
            crate::model::Root::resolve(root.path(), root.path()).expect("resolve temporary Root");
        let manifest_path = root.path().join("Cargo.toml");
        let mut report = Report::empty(Selection {
            root: root_identity,
            package_selectors: BTreeSet::new(),
            workspace: false,
            package_exclude_selectors: BTreeSet::new(),
            root_files: RootFilePolicy::Include,
            all_features: true,
            no_default_features: false,
            features: BTreeSet::new(),
            requested_targets: BTreeSet::new(),
            target_includes: BTreeSet::new(),
            target_excludes: BTreeSet::new(),
            json: false,
            totals: false,
        });
        report
            .package_projects
            .insert("package-id".to_owned(), root.path().to_path_buf());
        let rows = vec![
            AccountingRow {
                package_id: "package-id".to_owned(),
                package_name: "mixed".to_owned(),
                manifest_path: manifest_path.clone(),
                language: LanguageId::RUST,
                engine: AccountingEngine::Rust,
                precision: AccountingPrecision::ConfigurationAware,
                counts: Counts {
                    files: 1,
                    lines: 3,
                    code: 2,
                    test: TestCount::Known(1),
                    ..Counts::default()
                },
            },
            AccountingRow {
                package_id: "package-id".to_owned(),
                package_name: "mixed".to_owned(),
                manifest_path,
                language: LanguageId::new("typescript", "TypeScript"),
                engine: AccountingEngine::Tokei,
                precision: AccountingPrecision::Lexical,
                counts: Counts {
                    files: 1,
                    lines: 4,
                    comments: 1,
                    code: 3,
                    test: TestCount::Unavailable,
                    ..Counts::default()
                },
            },
        ];

        report.apply_rows(&rows).expect("apply mixed rows");
        let table = String::from_utf8(report.render().expect("render table")).expect("UTF-8 table");

        assert!(table.contains(" mixed   ┆ TypeScript ┆"));
        assert!(table.contains("         ┆ Rust       ┆"));
        assert!(!table.contains("mixed ("));
        assert!(
            table.contains(
                " Total   ┆ All        ┆     2 ┆     7 ┆      0 ┆        1 ┆    5 ┆  n/a "
            )
        );
        assert!(table.lines().filter(|line| line.ends_with(" n/a ")).count() == 2);
    }

    #[test]
    fn json_v3_uses_null_for_unavailable_test_and_structural_package_scope() {
        let root = TempDir::new().expect("create Root");
        let root_identity =
            crate::model::Root::resolve(root.path(), root.path()).expect("resolve temporary Root");
        let manifest_path = root.path().join("Cargo.toml");
        let mut report = Report::empty(Selection {
            root: root_identity,
            package_selectors: BTreeSet::new(),
            workspace: false,
            package_exclude_selectors: BTreeSet::new(),
            root_files: RootFilePolicy::Include,
            all_features: true,
            no_default_features: false,
            features: BTreeSet::new(),
            requested_targets: BTreeSet::new(),
            target_includes: BTreeSet::new(),
            target_excludes: BTreeSet::new(),
            json: true,
            totals: false,
        });
        report
            .package_projects
            .insert("package-id".to_owned(), root.path().to_path_buf());
        report
            .apply_rows(&[AccountingRow {
                package_id: "package-id".to_owned(),
                package_name: "mixed".to_owned(),
                manifest_path,
                language: LanguageId::new("typescript", "TypeScript"),
                engine: AccountingEngine::Tokei,
                precision: AccountingPrecision::Lexical,
                counts: Counts {
                    files: 1,
                    lines: 1,
                    code: 1,
                    test: TestCount::Unavailable,
                    ..Counts::default()
                },
            }])
            .expect("apply lexical row");

        let document: serde_json::Value =
            serde_json::from_slice(&report.render().expect("render JSON")).expect("parse JSON");
        assert_eq!(document["schema_version"], 3);
        assert_eq!(document["rows"][0]["scope"]["kind"], "package");
        assert_eq!(document["rows"][0]["scope"]["name"], "mixed");
        assert_eq!(document["rows"][0]["test"], serde_json::Value::Null);
        assert_eq!(document["rows"][0]["accounting_engine"], "tokei");
        assert_eq!(document["rows"][0]["accounting_precision"], "lexical");
        assert_eq!(document["total"]["test"], serde_json::Value::Null);
    }

    #[test]
    fn rows_sort_by_descending_package_total_then_language_total() {
        let root = TempDir::new().expect("create Root");
        let root_identity =
            crate::model::Root::resolve(root.path(), root.path()).expect("resolve temporary Root");
        let mut report = Report::empty(Selection {
            root: root_identity,
            package_selectors: BTreeSet::new(),
            workspace: false,
            package_exclude_selectors: BTreeSet::new(),
            root_files: RootFilePolicy::Include,
            all_features: true,
            no_default_features: false,
            features: BTreeSet::new(),
            requested_targets: BTreeSet::new(),
            target_includes: BTreeSet::new(),
            target_excludes: BTreeSet::new(),
            json: false,
            totals: false,
        });
        for id in ["alpha", "beta"] {
            report
                .package_projects
                .insert(id.to_owned(), root.path().to_path_buf());
        }
        let rows = [
            ("alpha", "alpha", LanguageId::RUST, 6),
            (
                "alpha",
                "alpha",
                LanguageId::new("typescript", "TypeScript"),
                4,
            ),
            ("beta", "beta", LanguageId::RUST, 8),
        ]
        .into_iter()
        .map(
            |(package_id, package_name, language, lines)| AccountingRow {
                package_id: package_id.to_owned(),
                package_name: package_name.to_owned(),
                manifest_path: root.path().join(format!("{package_id}/Cargo.toml")),
                language,
                engine: AccountingEngine::Rust,
                precision: AccountingPrecision::ConfigurationAware,
                counts: Counts {
                    files: 1,
                    lines,
                    code: lines,
                    test: TestCount::Known(0),
                    ..Counts::default()
                },
            },
        )
        .collect::<Vec<_>>();

        report.apply_rows(&rows).expect("apply rows");

        assert_eq!(
            report
                .packages
                .iter()
                .map(|row| (
                    row.label.as_str(),
                    row.language.to_string(),
                    row.counts.lines
                ))
                .collect::<Vec<_>>(),
            vec![
                ("alpha", "Rust".to_owned(), 6),
                ("alpha", "TypeScript".to_owned(), 4),
                ("beta", "Rust".to_owned(), 8),
            ]
        );
    }
}
