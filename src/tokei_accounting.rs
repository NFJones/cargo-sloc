//! Narrow in-memory Tokei adapter for non-Rust lexical accounting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokei::{Config, LanguageType};

use crate::accountant::{AccountingEngine, AccountingPrecision, AccountingRow, LanguageId};
use crate::error::AppError;
use crate::generic_source::GenericSourceInventory;
use crate::model::{Counts, TestCount};

/// Version of cargo-sloc's Tokei recognition and conversion behavior.
pub const ADAPTER_VERSION: u32 = 2;

/// Pinned Tokei catalog compatibility version.
pub const CATALOG_VERSION: &str = "tokei-14.0.0";

#[derive(Clone)]
struct CachedAccounting {
    bytes: Arc<[u8]>,
    result: Option<(LanguageType, Counts)>,
}

/// Resident cache for per-file generic lexical accounting.
#[derive(Default)]
pub(crate) struct AccountingCache {
    entries: BTreeMap<PathBuf, CachedAccounting>,
    touched: std::collections::BTreeSet<PathBuf>,
}

impl AccountingCache {
    pub(crate) fn begin_refresh(&mut self) {
        self.touched.clear();
    }

    pub(crate) fn finish_refresh(&mut self) {
        self.entries.retain(|path, _| self.touched.contains(path));
    }
}

/// Accounts one retained non-Rust file while reusing content-validated results.
pub(crate) fn account_file_with_cache(
    path: &Path,
    bytes: &Arc<[u8]>,
    cache: &mut AccountingCache,
) -> Result<Option<(LanguageId, Counts)>, AppError> {
    cache.touched.insert(path.to_path_buf());
    let result = if let Some(cached) = cache
        .entries
        .get(path)
        .filter(|cached| cached.bytes.as_ref() == bytes.as_ref())
    {
        crate::metrics::record_cache(
            crate::metrics::Cache::GenericAccounting,
            crate::metrics::CacheOutcome::Hit,
            "physical-source-content",
        );
        cached.result
    } else {
        crate::metrics::record_cache(
            crate::metrics::Cache::GenericAccounting,
            crate::metrics::CacheOutcome::Miss,
            "physical-source-content",
        );
        let result = account_source(path, bytes, &Config::default())?;
        cache.entries.insert(
            path.to_path_buf(),
            CachedAccounting {
                bytes: Arc::clone(bytes),
                result,
            },
        );
        result
    };
    Ok(
        result
            .map(|(language, counts)| (LanguageId::new(language.name(), language.name()), counts)),
    )
}

/// Returns whether a path can be recognized without reading it, or may be an
/// extensionless script requiring retained-byte shebang inspection.
pub fn is_candidate_path(path: &Path) -> bool {
    if is_rust(path) {
        return false;
    }
    special_filename(path).is_some()
        || path
            .extension()
            .and_then(|extension| extension.to_str())
            .and_then(LanguageType::from_file_extension)
            .is_some()
        || path.extension().is_none()
}

/// Returns whether recognizing this candidate requires a bounded shebang probe.
pub fn requires_shebang_probe(path: &Path) -> bool {
    !is_rust(path) && special_filename(path).is_none() && path.extension().is_none()
}

/// Recognizes a non-Rust language using path metadata and retained bytes.
pub fn recognize(path: &Path, bytes: &[u8]) -> Option<LanguageType> {
    if is_rust(path) {
        return None;
    }
    special_filename(path)
        .or_else(|| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .and_then(LanguageType::from_file_extension)
        })
        .or_else(|| {
            path.extension()
                .is_none()
                .then(|| shebang_language(bytes))
                .flatten()
        })
        .filter(|language| *language != LanguageType::Rust)
}

/// Accounts retained generic-source bytes into deterministic Package/language
/// rows without invoking Tokei's filesystem walker or formatter.
pub fn account(inventory: &GenericSourceInventory) -> Result<Vec<AccountingRow>, AppError> {
    let mut cache = AccountingCache::default();
    account_with_cache(inventory, &mut cache)
}

/// Accounts generic files while reusing content-validated per-file results.
pub(crate) fn account_with_cache(
    inventory: &GenericSourceInventory,
    cache: &mut AccountingCache,
) -> Result<Vec<AccountingRow>, AppError> {
    let config = Config::default();
    let mut rows = Vec::new();
    cache.begin_refresh();

    for package in &inventory.packages {
        let mut by_language = BTreeMap::<LanguageType, Counts>::new();
        for source in &package.files {
            cache.touched.insert(source.path.clone());
            let result = if let Some(cached) = cache
                .entries
                .get(&source.path)
                .filter(|cached| cached.bytes.as_ref() == source.bytes.as_ref())
            {
                crate::metrics::record_cache(
                    crate::metrics::Cache::GenericAccounting,
                    crate::metrics::CacheOutcome::Hit,
                    "physical-source-content",
                );
                cached.result
            } else {
                crate::metrics::record_cache(
                    crate::metrics::Cache::GenericAccounting,
                    crate::metrics::CacheOutcome::Miss,
                    "physical-source-content",
                );
                let result = account_source(&source.path, &source.bytes, &config)?;
                cache.entries.insert(
                    source.path.clone(),
                    CachedAccounting {
                        bytes: Arc::clone(&source.bytes),
                        result,
                    },
                );
                result
            };
            let Some((language, counts)) = result else {
                continue;
            };
            let total = by_language.entry(language).or_insert(Counts {
                test: TestCount::Unavailable,
                ..Counts::default()
            });
            *total = total.checked_add(counts)?;
        }

        rows.extend(
            by_language
                .into_iter()
                .map(|(language, counts)| AccountingRow {
                    package_id: package.id.clone(),
                    package_name: package.name.clone(),
                    manifest_path: package.manifest_path.clone(),
                    language: LanguageId::new(language.name(), language.name()),
                    engine: AccountingEngine::Tokei,
                    precision: AccountingPrecision::Lexical,
                    counts,
                }),
        );
    }
    rows.sort_by(|left, right| {
        left.package_name
            .cmp(&right.package_name)
            .then_with(|| left.language.cmp(&right.language))
            .then_with(|| left.package_id.cmp(&right.package_id))
    });
    cache.finish_refresh();
    Ok(rows)
}

fn account_source(
    path: &Path,
    bytes: &Arc<[u8]>,
    config: &Config,
) -> Result<Option<(LanguageType, Counts)>, AppError> {
    if bytes.contains(&0) {
        return Ok(None);
    }
    let Some(language) = recognize(path, bytes) else {
        return Ok(None);
    };
    let stats = language.parse_from_slice(bytes, config).summarise();
    let (blanks, comments, code) =
        physical_line_counts(bytes, stats.blanks, stats.comments, stats.code);
    Ok(Some((
        language,
        Counts {
            files: 1,
            lines: checked_usize(physical_line_count(bytes), "counting Tokei source lines")?,
            blanks: checked_usize(blanks, "counting Tokei blank lines")?,
            comments: checked_usize(comments, "counting Tokei comment lines")?,
            code: checked_usize(code, "counting Tokei code lines")?,
            test: TestCount::Unavailable,
        },
    )))
}

/// Collapses recursively reported embedded-language statistics to one category
/// per physical line in the host file.
fn physical_line_counts(
    bytes: &[u8],
    blanks: usize,
    comments: usize,
    code: usize,
) -> (usize, usize, usize) {
    let lines = physical_line_count(bytes);
    let blanks = blanks.min(lines);
    let remaining = lines - blanks;
    let mut comments = comments.min(remaining);
    let mut code = code.min(remaining);
    let excess = comments.saturating_add(code).saturating_sub(remaining);
    let removed_code = code.min(excess);
    code -= removed_code;
    comments -= excess - removed_code;
    code += remaining - comments - code;
    (blanks, comments, code)
}

fn physical_line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    bytes.iter().filter(|byte| **byte == b'\n').count() + usize::from(!bytes.ends_with(b"\n"))
}

fn checked_usize(value: usize, operation: &'static str) -> Result<u64, AppError> {
    u64::try_from(value).map_err(|_| AppError::CountOverflow(operation))
}

fn is_rust(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
}

fn special_filename(path: &Path) -> Option<LanguageType> {
    let filename = path.file_name()?.to_str()?.to_ascii_lowercase();
    let name = match filename.as_str() {
        "build" | "workspace" | "module" => "Bazel",
        "cmakelists.txt" => "CMake",
        "dockerfile" => "Dockerfile",
        "justfile" => "Just",
        "gnumakefile" | "makefile" => "Makefile",
        "meson.build" | "meson_options.txt" => "Meson",
        "nuget.config" | "packages.config" | "nugetdefaults.config" => "NuGet Config",
        "pkgbuild" => "Pacman Makepkg",
        "rakefile" => "Rakefile",
        "sconstruct" | "sconscript" => "SCons",
        "snakefile" => "Snakemake",
        _ => return None,
    };
    LanguageType::from_name(name)
}

fn shebang_language(bytes: &[u8]) -> Option<LanguageType> {
    let prefix = &bytes[..bytes.len().min(128)];
    let first_line = prefix.split(|byte| *byte == b'\n').next()?;
    let first_line = std::str::from_utf8(first_line).ok()?.trim_end_matches('\r');
    let mut words = first_line.split_whitespace();
    let executable = words.next()?;
    let language = match executable {
        "#!/bin/awk" if words.next() == Some("-f") => "AWK",
        "#!/bin/bash" => "BASH",
        "#!/usr/bin/crystal" => "Crystal",
        "#!/bin/csh" => "C Shell",
        "#!/bin/fish" => "Fish",
        "#!/usr/bin/env" if words.clone().next() == Some("just") => "Just",
        "#!/bin/ksh" => "Korn Shell",
        "#!/usr/bin/perl" => "Perl",
        "#!/usr/bin/raku" | "#!/usr/bin/perl6" => "Raku",
        "#!/bin/sh" => "Shell",
        "#!/bin/zsh" => "Zsh",
        "#!/usr/bin/env" => return env_language(words.next()?),
        _ => return None,
    };
    LanguageType::from_name(language)
}

fn env_language(interpreter: &str) -> Option<LanguageType> {
    let name = if interpreter.starts_with("bash") {
        "BASH"
    } else if interpreter.starts_with("crystal") {
        "Crystal"
    } else if interpreter.starts_with("csh") {
        "C Shell"
    } else if interpreter.starts_with("cython") {
        "Cython"
    } else if interpreter.starts_with("elvish") {
        "Elvish"
    } else if interpreter.starts_with("fish") {
        "Fish"
    } else if interpreter.starts_with("groovy") {
        "Groovy"
    } else if interpreter.starts_with("just") {
        "Just"
    } else if interpreter.starts_with("ksh") {
        "Korn Shell"
    } else if interpreter.starts_with("python") {
        "Python"
    } else if interpreter.starts_with("racket") {
        "Racket"
    } else if interpreter.starts_with("raku") || interpreter.starts_with("perl6") {
        "Raku"
    } else if interpreter.starts_with("ruby") {
        "Ruby"
    } else if interpreter.starts_with("sh") {
        "Shell"
    } else {
        return None;
    };
    LanguageType::from_name(name)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use crate::generic_source::{GenericPackageSources, GenericSource, GenericSourceInventory};
    use crate::model::TestCount;

    use super::{account, is_candidate_path, recognize};

    #[test]
    fn recognizes_extensions_special_filenames_and_retained_shebangs_but_not_rust() {
        assert_eq!(
            recognize(&PathBuf::from("app.ts"), b"").map(|language| language.name()),
            Some("TypeScript")
        );
        assert_eq!(
            recognize(&PathBuf::from("Dockerfile"), b"").map(|language| language.name()),
            Some("Dockerfile")
        );
        assert_eq!(
            recognize(
                &PathBuf::from("tool"),
                b"#!/usr/bin/env python3\nprint('ok')\n"
            )
            .map(|language| language.name()),
            Some("Python")
        );
        assert!(recognize(&PathBuf::from("lib.rs"), b"fn main() {}\n").is_none());
        assert!(is_candidate_path(&PathBuf::from("script")));
        assert!(!is_candidate_path(&PathBuf::from("lib.rs")));
    }

    #[test]
    fn accounts_retained_bytes_by_language_with_unavailable_test_counts() {
        let inventory = GenericSourceInventory {
            root: Default::default(),
            packages: vec![GenericPackageSources {
                project_root: PathBuf::from("/project"),
                id: "package-id".to_owned(),
                name: "app".to_owned(),
                manifest_path: PathBuf::from("/project/Cargo.toml"),
                files: vec![
                    source(
                        "/project/app.js",
                        b"// note\nconst text = '/* code */';\n\n",
                    ),
                    source("/project/tool", b"#!/usr/bin/env python3\nprint('ok')"),
                ],
            }],
            warnings: Vec::new(),
        };

        let rows = account(&inventory).expect("account lexical sources");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].language.display_name(), "JavaScript");
        assert_eq!(rows[0].counts.files, 1);
        assert_eq!(rows[0].counts.lines, 3);
        assert_eq!(rows[0].counts.blanks, 1);
        assert_eq!(rows[0].counts.comments, 1);
        assert_eq!(rows[0].counts.code, 1);
        assert_eq!(rows[0].counts.test, TestCount::Unavailable);
        assert_eq!(rows[1].language.display_name(), "Python");
        assert_eq!(rows[1].counts.files, 1);
        assert_eq!(rows[1].counts.lines, 2);
        assert_eq!(rows[1].counts.test, TestCount::Unavailable);
    }

    #[test]
    fn embedded_languages_are_summarized_into_one_host_file_row() {
        let inventory = GenericSourceInventory {
            root: Default::default(),
            packages: vec![GenericPackageSources {
                project_root: PathBuf::from("/project"),
                id: "package-id".to_owned(),
                name: "app".to_owned(),
                manifest_path: PathBuf::from("/project/Cargo.toml"),
                files: vec![source(
                    "/project/index.html",
                    b"<script>\nconst value = 1;\n</script>\n",
                )],
            }],
            warnings: Vec::new(),
        };

        let rows = account(&inventory).expect("account embedded source");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].language.display_name(), "HTML");
        assert_eq!(rows[0].counts.files, 1);
        assert_eq!(rows[0].counts.lines, 3);
        assert_eq!(rows[0].counts.test, TestCount::Unavailable);
    }

    #[test]
    fn embedded_inline_script_counts_its_single_physical_line_once() {
        let inventory = GenericSourceInventory {
            root: Default::default(),
            packages: vec![GenericPackageSources {
                project_root: PathBuf::from("/project"),
                id: "package-id".to_owned(),
                name: "app".to_owned(),
                manifest_path: PathBuf::from("/project/Cargo.toml"),
                files: vec![source(
                    "/project/index.html",
                    b"<script>const value = 1;</script>",
                )],
            }],
            warnings: Vec::new(),
        };

        let rows = account(&inventory).expect("account inline embedded source");

        assert_eq!(rows[0].counts.lines, 1);
        assert_eq!(rows[0].counts.blanks, 0);
        assert_eq!(rows[0].counts.comments, 0);
        assert_eq!(rows[0].counts.code, 1);
    }

    #[test]
    fn accepts_non_utf8_source_bytes_but_skips_nul_bearing_binary_candidates() {
        let inventory = GenericSourceInventory {
            root: Default::default(),
            packages: vec![GenericPackageSources {
                project_root: PathBuf::from("/project"),
                id: "package-id".to_owned(),
                name: "app".to_owned(),
                manifest_path: PathBuf::from("/project/Cargo.toml"),
                files: vec![
                    source("/project/non-utf8.js", b"const value = '\xff';\n"),
                    source("/project/binary.js", b"const\0value = 1;\n"),
                ],
            }],
            warnings: Vec::new(),
        };

        let rows = account(&inventory).expect("account byte-oriented source");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].counts.files, 1);
        assert_eq!(rows[0].counts.lines, 1);
    }

    #[test]
    fn pinned_ambiguity_rules_are_stable() {
        for (path, expected) in [
            ("header.h", "C Header"),
            ("objective.m", "Objective-C"),
            ("bits.inc", "Bitbake"),
            ("query.scm", "Scheme"),
            ("lower.s", "GNU Style Assembly"),
        ] {
            assert_eq!(
                recognize(&PathBuf::from(path), b"").map(|language| language.name()),
                Some(expected)
            );
        }
        assert!(recognize(&PathBuf::from("upper.S"), b"").is_none());
    }

    fn source(path: &str, bytes: &[u8]) -> GenericSource {
        GenericSource {
            path: PathBuf::from(path),
            bytes: Arc::from(bytes),
        }
    }
}
