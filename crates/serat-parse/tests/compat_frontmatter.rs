//! Block-style frontmatter lists against the compat fixtures (task #57; FR-STR-01,
//! FR-NAV-01, NFR-CMP-01; ADR-0003).
//!
//! Fixtures live in `bench/compat/corpus/frontmatter/` (see its README):
//! `accepted/` shapes must become one `List` field with the expected items, and
//! `refused/` shapes must leave no field under the list's key, no panic, and the
//! keys around them (`title`, `status`) intact.

use std::fs;
use std::path::{Path, PathBuf};

use serat_core::{Field, FieldValue, NoteParser, VaultPath};
use serat_parse::CommonMarkParser;

fn dir(kind: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bench/compat/corpus/frontmatter")
        .join(kind)
}

fn fields_of(kind: &str, name: &str, transform: impl Fn(String) -> String) -> (String, Vec<Field>) {
    let path = dir(kind).join(format!("{name}.md"));
    let text = transform(fs::read_to_string(&path).expect("fixture"));
    let parsed = CommonMarkParser::new()
        .parse(&VaultPath::new("note.md").expect("path"), text.as_bytes())
        .expect("a fixture never fails the note");
    (text, parsed.fields)
}

fn text(s: &str) -> FieldValue {
    FieldValue::Text(s.to_owned())
}

fn items_of(fields: &[Field], key: &str) -> Vec<FieldValue> {
    match fields.iter().find(|f| f.key == key).map(|f| &f.value) {
        Some(FieldValue::List(items)) => items.clone(),
        other => panic!("{key}: expected a list, found {other:?}"),
    }
}

fn expected() -> Vec<(&'static str, &'static str, Vec<FieldValue>)> {
    vec![
        (
            "aliases-plain",
            "aliases",
            vec![text("alpha"), text("beta gamma")],
        ),
        (
            "aliases-quoted",
            "aliases",
            vec![
                text("double: with colon"),
                text("single # not a comment"),
                text("Ada (Lovelace)"),
            ],
        ),
        ("tags-column-zero", "tags", vec![text("one"), text("two")]),
        (
            "tags-four-space",
            "tags",
            vec![text("one"), text("two"), text("three")],
        ),
        (
            "comments-and-blanks",
            "aliases",
            vec![text("alpha"), text("beta")],
        ),
        (
            "null-item-dropped",
            "aliases",
            vec![text("alpha"), text("beta")],
        ),
        (
            "typed-plain-items",
            "aliases",
            vec![
                FieldValue::Num(2026.0),
                text("2026"),
                FieldValue::Bool(true),
            ],
        ),
        (
            "wikilinks-quoted",
            "attendees",
            vec![text("[[Ada Lovelace]]"), text("[[Alan Turing]]")],
        ),
        (
            "flow-mixed",
            "tags",
            vec![
                FieldValue::Num(2026.0),
                text("2026"),
                FieldValue::Bool(true),
                text("a, b"),
                text("[[X]]"),
                text("plain one"),
            ],
        ),
        (
            "block-mixed",
            "tags",
            vec![
                FieldValue::Num(2026.0),
                text("2026"),
                FieldValue::Bool(true),
                text("a, b"),
                text("[[X]]"),
                text("plain one"),
            ],
        ),
        (
            "non-ascii",
            "aliases",
            vec![text("\u{e9}t\u{e9}"), text("\u{3bf}\u{3b4}\u{3cc}\u{3c2}")],
        ),
    ]
}

#[test]
fn every_accepted_shape_is_one_list_field_with_its_items() {
    for (name, key, items) in expected() {
        let (_, fields) = fields_of("accepted", name, |t| t);
        assert_eq!(items_of(&fields, key), items, "{name}");
        assert_eq!(
            fields.iter().filter(|f| f.key == key).count(),
            1,
            "{name}: one field per list"
        );
        for survivor in ["title", "status"] {
            assert!(
                fields.iter().any(|f| f.key == survivor),
                "{name}: {survivor}"
            );
        }
    }
}

#[test]
fn the_accepted_shapes_parse_the_same_with_crlf_line_endings() {
    for (name, key, items) in expected() {
        let (_, fields) = fields_of("accepted", name, |t| t.replace('\n', "\r\n"));
        assert_eq!(items_of(&fields, key), items, "{name} (CRLF)");
    }
}

#[test]
fn a_list_field_ranges_cover_the_first_marker_to_the_last_item_byte() {
    let (src, fields) = fields_of("accepted", "aliases-quoted", |t| t);
    let field = fields.iter().find(|f| f.key == "aliases").expect("aliases");
    let value = field.value_range.slice(src.as_bytes()).expect("slice");
    assert_eq!(
        std::str::from_utf8(value).expect("utf8"),
        "- \"double: with colon\"\n  - 'single # not a comment'\n  - \"Ada (Lovelace)\""
    );
    let whole = field.range.slice(src.as_bytes()).expect("slice");
    assert!(
        std::str::from_utf8(whole)
            .expect("utf8")
            .starts_with("aliases:\n")
    );
}

#[test]
fn every_refused_shape_leaves_no_field_for_its_key_and_keeps_its_neighbours() {
    let mut seen = 0;
    for entry in fs::read_dir(dir("refused")).expect("refused dir") {
        let path = entry.expect("entry").path();
        let name = path
            .file_stem()
            .expect("stem")
            .to_string_lossy()
            .into_owned();
        let (src, fields) = fields_of("refused", &name, |t| t);
        let key = src
            .lines()
            .nth(2)
            .and_then(|l| l.split(':').next())
            .expect("the fixture's key is on line 3");
        assert!(
            !fields.iter().any(|f| f.key == key),
            "{name}: {key} must be refused whole, found {fields:?}"
        );
        // Nothing nested leaks out as a top-level field either.
        for leaked in ["name", "type", "inner"] {
            assert!(
                !fields.iter().any(|f| f.key == leaked),
                "{name}: {leaked} leaked"
            );
        }
        for survivor in ["title", "status"] {
            assert!(
                fields.iter().any(|f| f.key == survivor),
                "{name}: {survivor} was lost"
            );
        }
        seen += 1;
    }
    assert!(seen >= 14, "the refused corpus shrank to {seen} cases");
}

#[test]
fn the_flow_and_block_forms_of_the_same_items_give_the_same_values() {
    let (_, flow) = fields_of("accepted", "flow-mixed", |t| t);
    let (_, block) = fields_of("accepted", "block-mixed", |t| t);
    assert_eq!(items_of(&flow, "tags"), items_of(&block, "tags"));
}
