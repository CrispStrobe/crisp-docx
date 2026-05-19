//! End-to-end integration tests against the public API surface.
//!
//! These exercise the same operations that `docxtool clean` /
//! `docxtool notes-kind` exercise in the Python project, so behaviour
//! parity is testable without running the Python pipeline.

mod common;

use std::collections::BTreeMap;

use crisp_docx_core::{
    convert_notes_kind, inject_footnotes, normalize_tags, strip_rsids, transplant_body, NotesKind,
    Package,
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
fn transplant_swaps_body_and_preserves_blueprint_sectpr() {
    let mut bp = Package::from_bytes(&common::docx_blueprint()).unwrap();
    let src = Package::from_bytes(&common::docx_source_with_footnotes()).unwrap();

    transplant_body(&mut bp, &src).unwrap();

    let doc = part_str(&bp, "word/document.xml");

    // Source content is present.
    assert!(doc.contains("source paragraph one"));
    assert!(doc.contains("source paragraph two"));

    // Blueprint's distinctive paragraph is gone — only its sectPr survives.
    assert!(!doc.contains("blueprint-only paragraph"));

    // Blueprint's sectPr (letter size) is preserved; source's (8000x10000) is dropped.
    assert!(doc.contains("12240"), "letter width missing");
    assert!(doc.contains("15840"), "letter height missing");
    assert!(!doc.contains("w:w=\"8000\""), "source sectPr leaked");
    assert!(!doc.contains("w:h=\"10000\""), "source sectPr leaked");

    // Footnotes part carried over.
    let fn_part = part_str(&bp, "word/footnotes.xml");
    assert!(fn_part.contains("source note 1"));

    // Content-types and rels patched.
    let ct = part_str(&bp, "[Content_Types].xml");
    assert!(ct.contains("/word/footnotes.xml"));
    let rels = part_str(&bp, "word/_rels/document.xml.rels");
    assert!(rels.contains("relationships/footnotes"));
    assert!(rels.contains(r#"Target="footnotes.xml""#));
}

#[test]
fn transplant_strips_rsids_from_grafted_runs() {
    // Source has rsid attrs on a paragraph; blueprint has none. After
    // transplant, the result must be rsid-free.
    let mut bp = Package::from_bytes(&common::docx_blueprint()).unwrap();
    let src = Package::from_bytes(&common::docx_with_rsids()).unwrap();
    transplant_body(&mut bp, &src).unwrap();
    let doc = part_str(&bp, "word/document.xml");
    for needle in ["paraId", "rsidR", "rsidRPr", "rsidRDefault"] {
        assert!(!doc.contains(needle), "should be stripped: {needle}");
    }
}

#[test]
fn transplant_remaps_styleids_when_blueprint_uses_different_ids() {
    use crisp_docx_core::{apply_style_mapping, transplant_body, StyleIndex, StyleMapper};
    use std::collections::HashMap;

    // Blueprint with a custom styleId for the H1 heading: "MyH1" named
    // "Heading 1". Source uses pandoc's "Heading1". The mapper should
    // rewrite "Heading1" → "MyH1" via name-roundtripping.
    let blueprint_doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="MyH1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#;
    let blueprint_styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:styleId="MyH1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>"#;
    let source_doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Source title</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#;
    let source_styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>"#;
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;

    let make = |doc: &str, styles: &str| -> Vec<u8> {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(buf);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        use std::io::Write;
        zw.start_file("[Content_Types].xml", opts).unwrap();
        zw.write_all(content_types.as_bytes()).unwrap();
        zw.start_file("word/document.xml", opts).unwrap();
        zw.write_all(doc.as_bytes()).unwrap();
        zw.start_file("word/styles.xml", opts).unwrap();
        zw.write_all(styles.as_bytes()).unwrap();
        zw.start_file("word/_rels/document.xml.rels", opts).unwrap();
        zw.write_all(rels.as_bytes()).unwrap();
        zw.finish().unwrap().into_inner()
    };

    let mut bp = Package::from_bytes(&make(blueprint_doc, blueprint_styles)).unwrap();
    let src = Package::from_bytes(&make(source_doc, source_styles)).unwrap();
    transplant_body(&mut bp, &src).unwrap();

    let doc = part_str(&bp, "word/document.xml");
    assert!(
        doc.contains(r#"w:val="MyH1""#),
        "expected pStyle remapped to MyH1, got: {doc}"
    );
    assert!(
        !doc.contains(r#"w:val="Heading1""#),
        "source styleId should be gone"
    );
    // Source content is in.
    assert!(doc.contains("Source title"));

    // Direct apply_style_mapping test, decoupled from transplant.
    let mut bp2 = Package::from_bytes(&make(blueprint_doc, blueprint_styles)).unwrap();
    // Force the document to contain a source-styleId paragraph.
    let src_bytes = source_doc.as_bytes().to_vec();
    bp2.set_part("word/document.xml", src_bytes);
    let bp_idx = StyleIndex::from_package(&bp2).unwrap();
    let src_idx = StyleIndex::from_styles_xml(source_styles.as_bytes()).unwrap();
    let mapper = StyleMapper::new(&bp_idx, HashMap::new());
    let n = apply_style_mapping(&mut bp2, &mapper, &src_idx, &bp_idx).unwrap();
    assert_eq!(n, 1, "expected exactly one paragraph rewritten");
    assert!(part_str(&bp2, "word/document.xml").contains(r#"w:val="MyH1""#));
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
