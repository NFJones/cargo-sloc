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
    /// Test-only code lines.
    pub test: u64,
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
            test: self
                .test
                .checked_add(other.test)
                .ok_or(AppError::CountOverflow("adding test counts"))?,
        })
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
