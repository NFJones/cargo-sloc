//! Integration coverage for cfg-aware Rust physical-line accounting.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use cargo_sloc::cli::ParseOutcome;
use cargo_sloc::configuration::resolve;
use cargo_sloc::discovery::discover;
use cargo_sloc::error::AppError;
use cargo_sloc::model::{Counts, Selection, TestCount};
use cargo_sloc::rust_accounting::{AccountingInventory, account};
use cargo_sloc::rust_source::discover as discover_sources;
use tempfile::TempDir;

#[test]
fn cfg_projection_separates_production_and_test_code() {
    let root = package(
        "cfg-counts",
        "src/lib.rs",
        r#"// top
pub fn production() {}
#[cfg(any())]
fn inactive() {}
#[cfg(test)]
fn test_only() {}
#[test]
fn built_in_test() {}
#[cfg_attr(test, test)]
fn conditional_harness() {}
fn same_line() { let before = 1; #[cfg(any())] const hidden: u8 = 0; let after = 2; }
fn cfg_macro() { if cfg!(any()) { unreachable!(); } }
macro_rules! tokens { ($($t:tt)*) => {}; }
tokens! { #[cfg(any())] const inside_macro: u8 = 0; }
"#,
    );

    assert_eq!(
        counts(root.path(), []),
        Counts {
            files: 1,
            lines: 12,
            blanks: 0,
            comments: 1,
            code: 7,
            test: TestCount::Known(4),
        }
    );
}

#[test]
fn cfg_projection_supports_statement_and_tail_macro_expressions() {
    let root = package(
        "cfg-macro-expressions",
        "src/lib.rs",
        r#"pub fn statement_macros() {
    #[cfg(any())]
    println!("inactive statement");
    #[cfg(all())]
    println!("active statement");
    #[cfg_attr(all(), cfg(any()))]
    println!("nested inactive");
}
pub fn tail_macro() {
    #[cfg(any())]
    println!("inactive tail")
}
"#,
    );

    assert_eq!(
        counts(root.path(), []),
        Counts {
            files: 1,
            lines: 6,
            blanks: 0,
            comments: 0,
            code: 6,
            test: TestCount::Known(0),
        }
    );
}

#[test]
fn inactive_cfg_constructs_remove_their_owned_separator_lines() {
    let root = package(
        "cfg-separators",
        "src/lib.rs",
        r#"pub struct Record {
    #[cfg(any())]
    hidden: u8
    ,
    pub active: u8,
}
pub enum Choice {
    #[cfg(any())]
    Hidden
    ,
    Active,
}
pub fn generic<
    #[cfg(any())]
    Hidden
    ,
    Active
>() {}
pub fn matched(value: u8) {
    match value {
        #[cfg(any())]
        0 => ()
        ,
        _ => (),
    }
}
"#,
    );

    assert_eq!(
        counts(root.path(), []),
        Counts {
            files: 1,
            lines: 14,
            blanks: 0,
            comments: 0,
            code: 14,
            test: TestCount::Known(0),
        }
    );
}

#[test]
fn rust_lexing_handles_comments_literals_shebang_and_blank_lines() {
    let root = package(
        "lexical-counts",
        "src/main.rs",
        "#!/usr/bin/env rustx\n\
// comment\n\
const A: &str = \"/* not comment */\"; // mixed\n\
const B: &str = r#\"// not comment\"#;\n\
const C: &str = \"first\n\
\n\
last\";\n\
/* block\n\
\n\
end */\n\
   \n\
fn main() {}\n",
    );

    assert_eq!(
        counts(root.path(), []),
        Counts {
            files: 1,
            lines: 12,
            blanks: 1,
            comments: 5,
            code: 7,
            test: TestCount::Known(0),
        }
    );
}

#[test]
fn empty_and_inactive_only_files_still_count_as_files() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"file-counts\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    write(
        root.path().join("src/lib.rs"),
        "mod empty;\nmod inactive;\n",
    );
    write(root.path().join("src/empty.rs"), "");
    write(
        root.path().join("src/inactive.rs"),
        "#![cfg(any())]\npub fn hidden() {}\n",
    );

    assert_eq!(
        counts(root.path(), []),
        Counts {
            files: 3,
            lines: 2,
            blanks: 0,
            comments: 0,
            code: 2,
            test: TestCount::Known(0),
        }
    );
}

#[test]
fn harness_free_test_and_bench_sources_are_test_without_cfg_test() {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"manual-harnesses\"\nversion = \"0.1.0\"\nedition = \"2024\"\nautobins = false\n\n[[test]]\nname = \"manual-test\"\npath = \"tests/manual.rs\"\nharness = false\n\n[[bench]]\nname = \"manual-bench\"\npath = \"benches/manual.rs\"\nharness = false\n",
    );
    let source = "fn main() {}\n#[cfg(test)]\nfn incorrectly_included() {}\n";
    write(root.path().join("tests/manual.rs"), source);
    write(root.path().join("benches/manual.rs"), source);

    assert_eq!(
        counts(root.path(), []),
        Counts {
            files: 2,
            lines: 2,
            blanks: 0,
            comments: 0,
            code: 0,
            test: TestCount::Known(2),
        }
    );
}

fn counts<const N: usize>(root: &Path, arguments: [&str; N]) -> Counts {
    let accounting = accounting(root, arguments).expect("account fixture");
    assert_eq!(accounting.packages.len(), 1);
    accounting.packages[0].counts
}

fn accounting<const N: usize>(
    root: &Path,
    arguments: [&str; N],
) -> Result<AccountingInventory, AppError> {
    let selection = selection(root, arguments)?;
    let inventory = discover(&selection)?;
    let configured = resolve(&selection, &inventory)?;
    let sources = discover_sources(&configured)?;
    account(&sources)
}

fn selection<const N: usize>(root: &Path, arguments: [&str; N]) -> Result<Selection, AppError> {
    let mut arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
    arguments.push(root.as_os_str().to_owned());
    match cargo_sloc::cli::parse(arguments, Path::new(env!("CARGO_MANIFEST_DIR")))? {
        ParseOutcome::Selection(selection) => Ok(selection),
        ParseOutcome::EarlyExit { .. } => panic!("unexpected early CLI exit"),
    }
}

fn package(name: &str, source_path: &str, source: &str) -> TempDir {
    let root = TempDir::new().expect("create Root");
    write(
        root.path().join("Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    );
    write(root.path().join(source_path), source);
    root
}

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture directory");
    fs::write(path, contents).expect("write fixture file");
}
