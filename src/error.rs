//! Typed operational and rendering errors.

use std::path::PathBuf;

use thiserror::Error;

/// An operational failure after command-line parsing has succeeded.
#[derive(Debug, Error)]
pub enum AppError {
    /// A semantic command-line usage error found after clap parsing.
    #[error("{0}")]
    Usage(String),
    /// The process working directory could not be read.
    #[error("failed to determine the current directory: {0}")]
    CurrentDirectory(#[source] std::io::Error),
    /// The requested Root does not exist or cannot be inspected.
    #[error("invalid Root `{path}`: {source}")]
    InvalidRoot {
        /// The user-provided or resolved path.
        path: PathBuf,
        /// The filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// The requested Root exists but is not a directory.
    #[error("invalid Root `{}`: path is not a directory", .0.display())]
    RootNotDirectory(PathBuf),
    /// A path required by the JSON schema is not valid UTF-8.
    #[error("cannot represent path `{}` losslessly in JSON", .0.display())]
    NonUtf8JsonPath(PathBuf),
    /// A Cargo-reported path could not be canonicalized.
    #[error("failed to canonicalize `{path}`: {source}")]
    CanonicalPath {
        /// Cargo-reported filesystem path.
        path: PathBuf,
        /// Canonicalization failure.
        #[source]
        source: std::io::Error,
    },
    /// Recursive manifest discovery failed.
    #[error("failed to discover Cargo manifests beneath `{root}`: {source}")]
    Discovery {
        /// Canonical discovery Root.
        root: PathBuf,
        /// Traversal failure.
        #[source]
        source: ignore::Error,
    },
    /// Cargo could not load a discovered candidate manifest.
    #[error("failed to load Cargo manifest `{manifest}`: {message}")]
    CargoMetadata {
        /// Discovered candidate manifest.
        manifest: PathBuf,
        /// Cargo metadata or bounded-process failure.
        message: String,
    },
    /// A Package manifest could not be interpreted for target settings.
    #[error("failed to interpret Cargo manifest `{manifest}`: {source}")]
    CargoManifest {
        /// Selected Package manifest.
        manifest: PathBuf,
        /// Manifest parsing or completion failure.
        #[source]
        source: cargo_toml::Error,
    },
    /// Project-local Cargo configuration could not be resolved.
    #[error("failed to resolve Cargo configuration for Project `{project}`: {message}")]
    CargoConfiguration {
        /// Project root whose configuration failed.
        project: PathBuf,
        /// Configuration diagnostic.
        message: String,
    },
    /// A non-compiling rustc query failed.
    #[error("rustc query failed for Project `{project}`: {message}")]
    RustcQuery {
        /// Project root used for toolchain selection.
        project: PathBuf,
        /// Query diagnostic.
        message: String,
    },
    /// Guppy could not reproduce the required Cargo feature context.
    #[error("failed to resolve Cargo feature contexts for Project `{project}`: {message}")]
    FeatureResolution {
        /// Project root whose feature graph failed.
        project: PathBuf,
        /// Resolver diagnostic.
        message: String,
    },
    /// A requested feature was not defined by any selected Package.
    #[error("feature `{0}` matched no selected Package")]
    UnmatchedFeature(String),
    /// A selected Rust source file could not be read.
    #[error("failed to read selected Rust source `{path}`: {source}")]
    SourceRead {
        /// Selected source path.
        path: PathBuf,
        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },
    /// A selected Rust source resolves outside the requested Root.
    #[error("selected Rust source `{path}` resolves outside Root `{root}`")]
    SourceOutsideRoot {
        /// Source path reported by Cargo or module resolution.
        path: PathBuf,
        /// Canonical requested Root.
        root: PathBuf,
    },
    /// A selected Rust source file was not valid UTF-8 input.
    #[error("selected Rust source `{path}` is not valid UTF-8: {message}")]
    SourceEncoding {
        /// Selected source path.
        path: PathBuf,
        /// Encoding diagnostic.
        message: String,
    },
    /// Lossless Rust parsing failed for selected source.
    #[error("failed to parse selected Rust source `{path}` using edition {edition}: {message}")]
    SourceParse {
        /// Selected source path.
        path: PathBuf,
        /// Cargo-reported Rust edition.
        edition: String,
        /// Parser diagnostic.
        message: String,
    },
    /// A cfg or cfg_attr used for module discovery was malformed.
    #[error("failed to evaluate module attributes in `{path}`: {message}")]
    ModuleAttribute {
        /// Source file containing the module declaration.
        path: PathBuf,
        /// Structural attribute diagnostic.
        message: String,
    },
    /// Rust module resolution found no source file.
    #[error(
        "cannot resolve module `{module}` declared in `{declaring_source}`; tried {candidates}"
    )]
    ModuleNotFound {
        /// Module name.
        module: String,
        /// Declaring source path.
        declaring_source: PathBuf,
        /// Candidate path summary.
        candidates: String,
    },
    /// Both accepted default module source paths exist.
    #[error(
        "module `{module}` declared in `{declaring_source}` is ambiguous between `{first}` and `{second}`"
    )]
    AmbiguousModule {
        /// Module name.
        module: String,
        /// Declaring source path.
        declaring_source: PathBuf,
        /// First existing candidate.
        first: PathBuf,
        /// Second existing candidate.
        second: PathBuf,
    },
    /// Cargo package-spec resolution could not be executed.
    #[error("failed to resolve package selector `{selector}` in Project `{project}`: {message}")]
    PackageSelector {
        /// Requested Cargo package specification.
        selector: String,
        /// Project root used for the query.
        project: PathBuf,
        /// Cargo diagnostic or process error.
        message: String,
    },
    /// A requested package selector matched no eligible Package.
    #[error("package selector `{0}` matched no eligible Package beneath the Root")]
    UnmatchedPackageSelector(String),
    /// A requested named target matched no selected Package.
    #[error("target selector `{0}` matched no target in the selected Packages")]
    UnmatchedTargetSelector(String),
    /// A requested named target is unavailable under the selected features.
    #[error("target `{target}` requires missing features: {features}")]
    IneligibleNamedTarget {
        /// Canonical target selector.
        target: String,
        /// Comma-separated missing features.
        features: String,
    },
    /// Checked report arithmetic overflowed.
    #[error("line-count overflow while {0}")]
    CountOverflow(&'static str),
    /// Report inputs violated an internal cross-phase identity invariant.
    #[error("cannot construct report: {0}")]
    ReportInvariant(String),
    /// JSON report serialization failed.
    #[error("failed to serialize JSON report: {0}")]
    Json(#[from] serde_json::Error),
}
