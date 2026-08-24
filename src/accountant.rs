//! Language Accountant boundary.

use std::fmt;
use std::path::PathBuf;

use crate::generic_source::PhysicalFileId;
use crate::model::{Counts, PackageId, SourceIdentity};

/// A stable supported-language identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LanguageId {
    key: &'static str,
    display_name: &'static str,
}

impl LanguageId {
    /// Rust source handled by cargo-loc's configuration-aware Accountant.
    pub const RUST: Self = Self::new("rust", "Rust");

    /// Rust source without a selected Cargo build context.
    pub const RUST_UNCONFIGURED: Self = Self::new("rust-unconfigured", "Rust (unconfigured)");

    /// Creates a stable language identity from catalog-owned static strings.
    pub const fn new(key: &'static str, display_name: &'static str) -> Self {
        Self { key, display_name }
    }

    /// Returns the stable machine-readable language key.
    pub const fn key(self) -> &'static str {
        self.key
    }

    /// Returns the stable user-facing language name.
    pub const fn display_name(self) -> &'static str {
        self.display_name
    }
}

impl fmt::Display for LanguageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.display_name)
    }
}

/// The implementation used to produce an accounting row.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AccountingEngine {
    /// cargo-loc's Rust syntax and cfg analysis.
    Rust,
    /// Tokei's generic lexical scanner.
    Tokei,
}

impl AccountingEngine {
    /// Returns the stable JSON spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Tokei => "tokei",
        }
    }
}

/// Precision of the semantics represented by an accounting row.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AccountingPrecision {
    /// Source was projected through exact selected build contexts.
    ConfigurationAware,
    /// Rust syntax was classified without Cargo build-context filtering.
    Unconfigured,
    /// Source was classified lexically without build-context provenance.
    Lexical,
}

impl AccountingPrecision {
    /// Returns the stable JSON spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfigurationAware => "configuration-aware",
            Self::Unconfigured => "unconfigured",
            Self::Lexical => "lexical",
        }
    }
}

/// Stable owner of one physical-file contribution.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ScopeId {
    /// A selected Cargo Package.
    Package {
        /// Cargo's opaque Package ID.
        id: String,
        /// Cargo Package name.
        name: String,
        /// Absolute Package manifest path.
        manifest_path: PathBuf,
        /// Absolute owning Project root.
        project_root: PathBuf,
    },
    /// The requested Root for files without one Package owner.
    Root {
        /// Canonical absolute requested Root.
        path: PathBuf,
    },
}

/// Mutually exclusive accounting path selected for one physical file.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AccountingRoute {
    /// Rust evaluated through selected Cargo semantic contexts.
    ConfiguredRust,
    /// Rust classified without selected Cargo semantic contexts.
    UnconfiguredRust,
    /// A non-Rust language classified by Tokei.
    Tokei(LanguageId),
}

/// One checked per-file contribution before Scope/language aggregation.
#[derive(Clone, Debug)]
pub struct FileContribution {
    /// Invocation-wide physical-file identity.
    pub identity: PhysicalFileId,
    /// Exactly one resolved report owner.
    pub scope: ScopeId,
    /// Exactly one resolved accounting path.
    pub route: AccountingRoute,
    /// Stable accounted language.
    pub language: LanguageId,
    /// Implementation that produced the contribution.
    pub engine: AccountingEngine,
    /// Semantic precision of the contribution.
    pub precision: AccountingPrecision,
    /// Counts for exactly one physical file.
    pub counts: Counts,
}

/// Language-neutral Package/language counts supplied to report aggregation.
#[derive(Clone, Debug)]
pub struct AccountingRow {
    /// Cargo's opaque Package ID.
    pub package_id: String,
    /// Cargo Package name.
    pub package_name: String,
    /// Absolute Package manifest path.
    pub manifest_path: PathBuf,
    /// Stable accounted language.
    pub language: LanguageId,
    /// Implementation that produced the counts.
    pub engine: AccountingEngine,
    /// Semantic precision of the counts.
    pub precision: AccountingPrecision,
    /// Common report measures.
    pub counts: Counts,
}

/// Language-neutral input supplied to an Accountant.
#[derive(Clone, Debug)]
pub struct AccountantInput {
    /// Owning Cargo Package.
    pub package: PackageId,
    /// Candidate source identities selected by shared infrastructure.
    pub sources: Vec<SourceIdentity>,
}

/// Language-neutral result returned by an Accountant.
#[derive(Clone, Debug)]
pub struct AccountantOutput {
    /// Accounted language.
    pub language: LanguageId,
    /// Implementation that produced the counts.
    pub engine: AccountingEngine,
    /// Semantic precision of the counts.
    pub precision: AccountingPrecision,
    /// Common report measures.
    pub counts: Counts,
}

/// Failure produced by language-specific analysis.
#[derive(Debug)]
pub struct AccountantError {
    message: String,
}

impl AccountantError {
    /// Creates an analysis failure with user-facing context.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AccountantError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AccountantError {}

/// A language-specific source discovery and classification component.
pub trait Accountant: Send + Sync {
    /// Returns the language handled by this Accountant.
    fn language(&self) -> LanguageId;

    /// Accounts the selected source for one Package.
    fn account(&self, input: &AccountantInput) -> Result<AccountantOutput, AccountantError>;
}

/// Invocation-local Accountant registry.
#[derive(Default)]
pub struct AccountantRegistry {
    accountants: Vec<Box<dyn Accountant>>,
}

impl AccountantRegistry {
    /// Registers one Accountant.
    pub fn register(&mut self, accountant: impl Accountant + 'static) {
        self.accountants.push(Box::new(accountant));
        self.accountants.sort_by_key(|item| item.language());
    }

    /// Iterates in stable language order.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Accountant> {
        self.accountants.iter().map(Box::as_ref)
    }
}
