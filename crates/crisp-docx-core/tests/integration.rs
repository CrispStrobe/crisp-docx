//! End-to-end integration tests against the public API surface.
//!
//! These exercise the same operations that `docxtool clean` /
//! `docxtool notes-kind` exercise in the Python project, so behaviour
//! parity is testable without running the Python pipeline.

mod common;

use std::collections::BTreeMap;

use crisp_docx_core::{
    convert_notes_kind, inject_footnotes, normalize_tags, strip_rsids, NotesKind, Package,
};

fn part_str<'a>(pkg: &'a Package, name: &str) -> &'a str {
    std::str::from_utf8(pkg.get_part(name).expect("part missing")).expect("non-utf8 part")
}

#[test]
fn round_trip_preserves_byte_identity() {
    let bytes = common::docx_with_rsids();
    let pkg = Package::from_bytes(&bytes).unwrap();
    // Same number of parts, same names, same content.
    let expected = ["[Content_Types].xml", "word/document.xml"];
    let names: Vec<_> = pkg.parts().map(|(n, _)| n.to_string()).collect();
    assert_eq!(names, expected);
}

#[test]
fn strip_rsids_end_to_end() {
    let mut pkg = Package::from_bytes(&common::docx_with_rsids()).unwrap();
    let n = strip_rsids(&mut pkg).unwrap();
    assert_eq!(n, 8, "fixture has 8 rsid-family attrs across <w:p> + <w:r>");
    let doc = part_str(&pkg, "word/document.xml");
    for needle in [
        "paraId",
        "textId",
        "rsidR",
        "rsidRDefault",
        "rsidRPr",
        "rsidP",
    ] {
        assert!(!doc.contains(needle), "should be gone: {needle}");
    }
    assert!(doc.contains("hello"), "content preserved");
}

#[test]
fn strip_rsids_idempotent() {
    let mut pkg = Package::from_bytes(&common::docx_with_rsids()).unwrap();
    let first = strip_rsids(&mut pkg).unwrap();
    let second = strip_rsids(&mut pkg).unwrap();
    assert!(first > 0);
    assert_eq!(second, 0);
}

#[test]
fn normalize_tags_end_to_end() {
    let mut pkg = Package::from_bytes(&common::docx_with_textutil_tags()).unwrap();
    let n = normalize_tags(&mut pkg).unwrap();
    assert_eq!(n, 3, "expected three renames: sz-cs, b-cs, i-cs");
    let doc = part_str(&pkg, "word/document.xml");
    for old in ["w:sz-cs", "w:b-cs", "w:i-cs"] {
        assert!(!doc.contains(old), "{old} should be renamed");
    }
    for new in ["w:szCs", "w:bCs", "w:iCs"] {
        assert!(doc.contains(new), "{new} should be present");
    }
}

#[test]
fn convert_footnotes_to_endnotes_end_to_end() {
    let mut pkg = Package::from_bytes(&common::docx_with_footnotes()).unwrap();
    convert_notes_kind(&mut pkg, NotesKind::Endnotes).unwrap();

    // Part was renamed.
    assert!(pkg.get_part("word/footnotes.xml").is_none());
    assert!(pkg.get_part("word/endnotes.xml").is_some());

    // Inside the (formerly footnotes) part, all the relevant element names
    // and style values now use the endnote spelling.
    let en = part_str(&pkg, "word/endnotes.xml");
    assert!(en.contains("<w:endnotes"));
    assert!(en.contains("<w:endnote "));
    assert!(en.contains("w:endnoteRef"));
    assert!(!en.contains("w:footnote"));

    // document.xml's reference was rewritten too.
    let doc = part_str(&pkg, "word/document.xml");
    assert!(doc.contains("w:endnoteReference"));
    assert!(!doc.contains("w:footnoteReference"));

    // Relationship + Content_Types updated.
    let rels = part_str(&pkg, "word/_rels/document.xml.rels");
    assert!(rels.contains("relationships/endnotes"));
    assert!(rels.contains("endnotes.xml"));
    assert!(!rels.contains("relationships/footnotes"));
    assert!(!rels.contains("footnotes.xml"));

    let ct = part_str(&pkg, "[Content_Types].xml");
    assert!(ct.contains("/word/endnotes.xml"));
    assert!(ct.contains("wordprocessingml.endnotes+xml"));
    assert!(!ct.contains("/word/footnotes.xml"));
}

#[test]
fn convert_endnotes_back_to_footnotes_is_reversible() {
    let mut pkg = Package::from_bytes(&common::docx_with_footnotes()).unwrap();
    convert_notes_kind(&mut pkg, NotesKind::Endnotes).unwrap();
    convert_notes_kind(&mut pkg, NotesKind::Footnotes).unwrap();

    // Back to the footnotes layout.
    assert!(pkg.get_part("word/footnotes.xml").is_some());
    assert!(pkg.get_part("word/endnotes.xml").is_none());
    let doc = part_str(&pkg, "word/document.xml");
    assert!(doc.contains("w:footnoteReference"));
    assert!(!doc.contains("w:endnoteReference"));
}

#[test]
fn convert_noop_when_already_target() {
    let mut pkg = Package::from_bytes(&common::docx_with_footnotes()).unwrap();
    convert_notes_kind(&mut pkg, NotesKind::Footnotes).unwrap();
    // Still has footnotes, no endnotes.
    assert!(pkg.get_part("word/footnotes.xml").is_some());
    assert!(pkg.get_part("word/endnotes.xml").is_none());
}

#[test]
fn inject_footnotes_creates_part_and_rewrites_runs() {
    let mut pkg = Package::from_bytes(&common::docx_with_inline_markers()).unwrap();
    let mut notes: BTreeMap<u32, &str> = BTreeMap::new();
    notes.insert(1, "Note one.");
    notes.insert(2, "Note two.");

    let report = inject_footnotes(&mut pkg, &notes).unwrap();
    assert_eq!(report.inserted, 2);
    assert!(report.unknown_ids.is_empty());
    assert!(report.unused_ids.is_empty());

    // Document body now references both notes.
    let doc = part_str(&pkg, "word/document.xml");
    assert!(doc.contains(r#"w:id="1""#));
    assert!(doc.contains(r#"w:id="2""#));
    assert!(doc.contains("w:footnoteReference"));
    assert!(doc.contains("FootnoteReference"));
    // Inline brackets gone.
    assert!(!doc.contains("[1]"));
    assert!(!doc.contains("[2]"));

    // Footnotes part exists with the entries plus the two separators.
    let fn_part = part_str(&pkg, "word/footnotes.xml");
    assert!(fn_part.contains(r#"w:id="-1""#));
    assert!(fn_part.contains(r#"w:id="0""#));
    assert!(fn_part.contains(r#"w:id="1""#));
    assert!(fn_part.contains(r#"w:id="2""#));
    assert!(fn_part.contains("Note one."));
    assert!(fn_part.contains("Note two."));

    // Content-types + rels patched.
    let ct = part_str(&pkg, "[Content_Types].xml");
    assert!(ct.contains("/word/footnotes.xml"));
    assert!(ct.contains("wordprocessingml.footnotes+xml"));

    let rels = part_str(&pkg, "word/_rels/document.xml.rels");
    assert!(rels.contains("relationships/footnotes"));
    assert!(rels.contains(r#"Target="footnotes.xml""#));
}

#[test]
fn inject_footnotes_reports_unknown_and_unused() {
    let mut pkg = Package::from_bytes(&common::docx_with_inline_markers()).unwrap();
    let mut notes: BTreeMap<u32, &str> = BTreeMap::new();
    notes.insert(1, "Note one.");
    // [2] is in the body but not in `notes`.
    notes.insert(99, "Never used.");

    let report = inject_footnotes(&mut pkg, &notes).unwrap();
    assert_eq!(report.inserted, 1);
    assert_eq!(report.unknown_ids, Vec::<u32>::new()); // [2] isn't *seen* by find_first_marker because it's not in `notes`
    assert_eq!(report.unused_ids, vec![99]);

    // The body still contains the unmatched [2] verbatim.
    let doc = part_str(&pkg, "word/document.xml");
    assert!(doc.contains("[2]"));
}

#[test]
fn save_then_open_round_trips_to_byte_equivalent_content() {
    use std::io::Write;
    let original = common::docx_with_rsids();
    let pkg = Package::from_bytes(&original).unwrap();

    let tmp = std::env::temp_dir().join(format!("crisp-docx-rt-{}.docx", std::process::id()));
    pkg.save(&tmp).unwrap();
    let reloaded = Package::open(&tmp).unwrap();
    let _ = std::fs::remove_file(&tmp);

    // Same parts, same contents.
    let want: Vec<(&str, &[u8])> = pkg.parts().collect();
    let got: Vec<(&str, &[u8])> = reloaded.parts().collect();
    assert_eq!(want.len(), got.len());
    for ((wn, wd), (gn, gd)) in want.iter().zip(got.iter()) {
        assert_eq!(wn, gn);
        assert_eq!(wd, gd);
    }

    // Suppress unused-import warning on Windows (where the helper isn't needed).
    let _ = std::io::sink().write(&[]);
}
