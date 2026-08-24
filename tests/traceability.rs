//! Mechanical completeness checks for specification traceability.

use std::collections::BTreeSet;

#[test]
fn normative_paragraphs_have_entries() {
    let normative_paragraphs = include_str!("../SPEC.md")
        .split("\n\n")
        .filter(|paragraph| paragraph.contains("MUST"))
        .count();
    let identifiers: Vec<_> = include_str!("../docs/TRACEABILITY.md")
        .lines()
        .filter_map(traceability_identifier)
        .collect();
    let unique: BTreeSet<_> = identifiers.iter().copied().collect();

    assert_eq!(identifiers.len(), normative_paragraphs);
    assert_eq!(
        unique.len(),
        normative_paragraphs,
        "duplicate traceability ID"
    );
    assert_eq!(
        identifiers,
        (1..=normative_paragraphs).collect::<Vec<_>>(),
        "traceability IDs must be consecutive and follow specification order"
    );
    assert!(
        !include_str!("../docs/TRACEABILITY.md").contains("| Planned |"),
        "release traceability must not contain unverified planned rows"
    );
}

fn traceability_identifier(line: &str) -> Option<usize> {
    let value = line.strip_prefix("| SPEC-")?.split_once(" |")?.0;
    Some(value.parse().expect("numeric traceability identifier"))
}
