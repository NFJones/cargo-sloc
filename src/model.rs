//! Language-neutral request, identity, context, and count models.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Canonical absolute discovery root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Root(PathBuf);

impl Root {
    /// Resolves, validates, and canonicalizes a requested Root.
    pub fn resolve(path: &Path, current_directory: &Path) -> Result<Self, AppError> {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            current_directory.join(path)
        };
        let canonical = resolved
            .canonicalize()
            .map_err(|source| AppError::InvalidRoot {
                path: resolved.clone(),
                source,
            })?;
        if !canonical.is_dir() {
            return Err(AppError::RootNotDirectory(canonical));
        }
        Ok(Self(canonical))
    }

    /// Returns the canonical path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// Returns the lossless schema-v1 JSON representation.
    pub fn json_string(&self) -> Result<String, AppError> {
        self.0
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| AppError::NonUtf8JsonPath(self.0.clone()))
    }
}

/// Opaque Project identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProjectId(pub String);

/// Opaque Cargo Package identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PackageId(pub String);

/// Opaque Cargo Target identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TargetId(pub String);

/// Whether contributions owned by the requested Root are included.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum)]
pub enum RootFilePolicy {
    /// Include supported files that do not resolve to one selected Package.
    #[default]
    Include,
    /// Exclude contributions whose final owner is the Root.
    Exclude,
}

impl RootFilePolicy {
    /// Returns the stable CLI and JSON spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Include => "include",
            Self::Exclude => "exclude",
        }
    }
}

/// Production or test report provenance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ContextKind {
    /// A non-harness production context.
    Production,
    /// A harness, integration-test, or benchmark context.
    Test,
}

/// Whether Cargo builds a context for the host or selected target platform.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum BuildRole {
    /// A build script, procedural macro, or other host-built artifact.
    Host,
    /// An artifact built for the effective Compilation Target.
    Target,
}

/// One exact Rust conditional-compilation option.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum CfgOption {
    /// A bare option such as `unix`.
    Name(String),
    /// An exact name-value pair such as `target_os = "linux"`.
    KeyValue { name: String, value: String },
}

/// Stable source identity used by Accountants.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceIdentity(pub PathBuf);

/// Availability-aware count of test-only code lines.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TestCount {
    /// The Accountant can classify test-only provenance.
    Known(u64),
    /// The Accountant cannot classify test-only provenance.
    Unavailable,
}

impl Default for TestCount {
    fn default() -> Self {
        Self::Known(0)
    }
}

impl TestCount {
    /// Adds two Test counts, propagating unavailability and detecting overflow.
    pub fn checked_add(self, other: Self) -> Result<Self, AppError> {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left
                .checked_add(right)
                .map(Self::Known)
                .ok_or(AppError::CountOverflow("adding test counts")),
            (Self::Unavailable, _) | (_, Self::Unavailable) => Ok(Self::Unavailable),
        }
    }

    /// Increments a known Test count with overflow detection.
    pub fn checked_increment(self, operation: &'static str) -> Result<Self, AppError> {
        match self {
            Self::Known(value) => value
                .checked_add(1)
                .map(Self::Known)
                .ok_or(AppError::CountOverflow(operation)),
            Self::Unavailable => Ok(Self::Unavailable),
        }
    }
}

/// Common unsigned report measures.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Counts {
    /// Unique physical files.
    pub files: u64,
    /// Included physical lines.
    pub lines: u64,
    /// Included blank lines.
    pub blanks: u64,
    /// Included comment lines.
    pub comments: u64,
    /// Production code lines.
    pub code: u64,
    /// Test-only code lines, when the Accountant can determine provenance.
    pub test: TestCount,
}

impl Counts {
    /// Adds another set of counts with overflow detection.
    pub fn checked_add(self, other: Self) -> Result<Self, AppError> {
        Ok(Self {
            files: self
                .files
                .checked_add(other.files)
                .ok_or(AppError::CountOverflow("adding file counts"))?,
            lines: self
                .lines
                .checked_add(other.lines)
                .ok_or(AppError::CountOverflow("adding line counts"))?,
            blanks: self
                .blanks
                .checked_add(other.blanks)
                .ok_or(AppError::CountOverflow("adding blank counts"))?,
            comments: self
                .comments
                .checked_add(other.comments)
                .ok_or(AppError::CountOverflow("adding comment counts"))?,
            code: self
                .code
                .checked_add(other.code)
                .ok_or(AppError::CountOverflow("adding code counts"))?,
            test: self.test.checked_add(other.test)?,
        })
    }
}

#[cfg(test)]
mod count_tests {
    use super::{Counts, TestCount};

    #[test]
    fn unavailable_test_counts_propagate_through_checked_addition() {
        let known = Counts {
            test: TestCount::Known(7),
            ..Counts::default()
        };
        let unavailable = Counts {
            test: TestCount::Unavailable,
            ..Counts::default()
        };

        assert_eq!(
            known
                .checked_add(unavailable)
                .expect("add unavailable Test count")
                .test,
            TestCount::Unavailable
        );
    }

    #[test]
    fn known_test_count_overflow_is_rejected() {
        let maximum = Counts {
            test: TestCount::Known(u64::MAX),
            ..Counts::default()
        };
        let one = Counts {
            test: TestCount::Known(1),
            ..Counts::default()
        };

        assert!(maximum.checked_add(one).is_err());
    }
}

/// Normalized command selection retained in reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    /// Canonical discovery root.
    pub root: Root,
    /// Requested Cargo package specifications.
    pub package_selectors: BTreeSet<String>,
    /// Whether workspace-wide selection was explicit.
    pub workspace: bool,
    /// Requested workspace package exclusions.
    pub package_exclude_selectors: BTreeSet<String>,
    /// Whether Root-owned source contributions are included.
    pub root_files: RootFilePolicy,
    /// Whether all features are active.
    pub all_features: bool,
    /// Whether default features are disabled.
    pub no_default_features: bool,
    /// Explicitly requested features.
    pub features: BTreeSet<String>,
    /// Explicit compilation-target requests.
    pub requested_targets: BTreeSet<String>,
    /// Canonical package-target inclusion selectors.
    pub target_includes: BTreeSet<String>,
    /// Canonical package-target exclusion selectors.
    pub target_excludes: BTreeSet<String>,
    /// Whether JSON output was requested.
    pub json: bool,
}
