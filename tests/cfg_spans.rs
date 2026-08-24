//! Lossless conditional-attribute span feasibility tests.

use std::fs;
use std::process::Command;

use ra_ap_syntax::{AstNode, Edition, SyntaxKind, ast};

const FIXTURE: &str = "tests/fixtures/cfg-spans/all_positions.rs";

#[test]
fn selected_parser_accepts_the_cfg_position_fixture_losslessly() {
    let source = fixture_source();
    let parse = ast::SourceFile::parse(&source, Edition::Edition2024);

    assert!(
        parse.errors().is_empty(),
        "parse errors: {:#?}",
        parse.errors()
    );
    assert_eq!(parse.syntax_node().to_string(), source);
}

#[test]
fn conditional_attributes_have_precise_governed_nodes() {
    let source = fixture_source();
    let parse = ast::SourceFile::parse(&source, Edition::Edition2024);
    let root = parse.tree().syntax().clone();
    let owners: Vec<_> = root
        .descendants()
        .filter_map(ast::Attr::cast)
        .filter(|attribute| {
            attribute
                .simple_name()
                .is_some_and(|name| name == "cfg" || name == "cfg_attr")
        })
        .map(|attribute| attribute.syntax().parent().expect("attribute owner").kind())
        .collect();

    assert_eq!(
        owners,
        [
            SyntaxKind::SOURCE_FILE,
            SyntaxKind::MODULE,
            SyntaxKind::CONST,
            SyntaxKind::RECORD_FIELD,
            SyntaxKind::VARIANT,
            SyntaxKind::TYPE_PARAM,
            SyntaxKind::LET_STMT,
            SyntaxKind::BLOCK_EXPR,
            SyntaxKind::MATCH_ARM,
            SyntaxKind::MACRO_EXPR,
            SyntaxKind::CONST,
            SyntaxKind::CONST,
        ]
    );
}

#[test]
fn governed_ranges_preserve_independent_comments_and_same_line_source() {
    let source = fixture_source();
    let parse = ast::SourceFile::parse(&source, Edition::Edition2024);
    let root = parse.tree().syntax().clone();
    let owner = root
        .descendants()
        .filter_map(ast::Attr::cast)
        .find(|attribute| {
            attribute.simple_name().is_some_and(|name| name == "cfg")
                && attribute.syntax().parent().is_some_and(|parent| {
                    parent.kind() == SyntaxKind::CONST
                        && parent.text().to_string().contains("same_line_marker")
                })
        })
        .and_then(|attribute| attribute.syntax().parent())
        .expect("same-line cfg owner");
    let range = owner.text_range();
    let start: usize = range.start().into();
    let end: usize = range.end().into();

    assert!(source[..start].ends_with("let keep_before = 1; "));
    assert!(source[start..end].contains("same_line_marker"));
    assert!(source[end..].starts_with(" let keep_after = 2;"));

    let standalone_comment = source.find("// standalone_before").expect("comment marker");
    let item_start = source
        .find("#[cfg(all())]\nconst item_marker")
        .expect("item marker");
    assert!(standalone_comment < item_start);
}

#[test]
fn attribute_like_tokens_inside_macros_are_not_parsed_as_attributes() {
    let source = fixture_source();
    let parse = ast::SourceFile::parse(&source, Edition::Edition2024);
    let root = parse.tree().syntax().clone();
    let macro_call = root
        .descendants()
        .find(|node| {
            node.kind() == SyntaxKind::MACRO_CALL
                && node.text().to_string().contains("token_tree_marker")
        })
        .expect("token-tree macro call");

    assert!(
        macro_call
            .descendants()
            .filter_map(ast::Attr::cast)
            .next()
            .is_none(),
        "attribute-like macro input must remain unexpanded token-tree source"
    );
}

#[test]
fn selected_toolchain_accepts_the_cfg_position_fixture() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    let output_dir = tempfile::tempdir().expect("create rustc probe output directory");
    let output = Command::new("rustc")
        .args([
            "--edition",
            "2024",
            "--crate-type",
            "lib",
            "--emit",
            "metadata",
        ])
        .arg(&fixture)
        .arg("-o")
        .arg(output_dir.path().join("cfg-span-fixture.rmeta"))
        .output()
        .expect("run rustc fixture probe");

    assert!(
        output.status.success(),
        "rustc rejected cfg-position fixture:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture_source() -> String {
    fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE))
        .expect("read cfg-span fixture")
}
