//! Language Accountant boundary.

use std::fmt;

use crate::model::{Counts, PackageId, SourceIdentity};

/// A stable supported-language identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Language {
    /// Rust source.
    Rust,
}

impl fmt::Display for Language {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rust => formatter.write_str("Rust"),
        }
    }
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
    pub language: Language,
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
    fn language(&self) -> Language;

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
