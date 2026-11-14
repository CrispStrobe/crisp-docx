//! Independent package/lexical controls for note-kind conversion.
use crisp_docx_core::{convert_notes_kind, NotesKind, Package};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";

fn fixture() -> Package {
    let mut p = Package::new();
    p.set_part("[Content_Types].xml", br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/footnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml"/></Types>"#.to_vec());
    p.set_part("word/document.xml", format!(r#"<w:document xmlns:w="{W}"><w:body><w:p><w:r><w:t>FootnoteText / FootnoteReference / w:footnoteRef / café 日本語</w:t><w:footnoteReference w:id="1"/></w:r></w:p></w:body></w:document>"#).into_bytes());
    p.set_part("word/footnotes.xml", format!(r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"><w:p><w:pPr><w:pStyle w:val="FootnoteText"/></w:pPr><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/><w:t>FootnoteText / w:footnoteRef</w:t></w:r></w:p></w:footnote></w:footnotes>"#).into_bytes());
    p.set_part("word/_rels/document.xml.rels", format!(r#"<Relationships xmlns="{R}"><Relationship Id="notes" Type="{REL}" Target="footnotes.xml"/><Relationship Id="unrelated" Type="urn:unrelated" Target="footnotes.xml.backup"/><Relationship Id="external" Type="urn:external" Target="https://example.org/FootnoteText" TargetMode="External"/></Relationships>"#).into_bytes());
    p.set_part("word/footnotes.xml.backup", b"untouched backup".to_vec());
    p.set_part("docProps/core.xml", b"<unchanged/>".to_vec());
    p
}

fn xml<'a>(p: &'a Package, name: &str) -> &'a str {
    std::str::from_utf8(p.get_part(name).unwrap()).unwrap()
}

#[test]
fn body_text_is_not_a_substitution_target() {
    let mut p = fixture();
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(xml(&p, "word/document.xml").contains("FootnoteText / FootnoteReference / w:footnoteRef / café 日本語"));
    assert!(xml(&p, "word/document.xml").contains("<w:endnoteReference"));
}

#[test]
fn note_text_is_not_a_substitution_target() {
    let mut p = fixture();
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    let s = xml(&p, "word/endnotes.xml");
    assert!(s.contains("<w:t>FootnoteText / w:footnoteRef</w:t>"));
    assert!(s.contains(r#"w:val="EndnoteText""#));
    assert!(s.contains(r#"w:val="EndnoteReference""#));
}

#[test]
fn relationship_sidecar_moves_with_its_owner_and_round_trips() {
    let mut p = fixture();
    let rels = format!(r#"<Relationships xmlns="{R}"><Relationship Id="link" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.org/FootnoteText?q=1&amp;b=2" TargetMode="External"/></Relationships>"#).into_bytes();
    p.set_part("word/_rels/footnotes.xml.rels", rels.clone());
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(p.get_part("word/_rels/footnotes.xml.rels").is_none());
    assert_eq!(p.get_part("word/_rels/endnotes.xml.rels"), Some(rels.as_slice()));
    convert_notes_kind(&mut p, NotesKind::Footnotes).unwrap();
    assert_eq!(p.get_part("word/_rels/footnotes.xml.rels"), Some(rels.as_slice()));
    assert!(p.get_part("word/_rels/endnotes.xml.rels").is_none());
}

#[test]
fn unrelated_parts_remain_byte_identical() {
    let mut p = fixture();
    let before = p.get_part("docProps/core.xml").unwrap().to_vec();
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert_eq!(p.get_part("docProps/core.xml"), Some(before.as_slice()));
}

#[test]
fn unrelated_relationship_targets_are_not_prefix_rewritten() {
    let mut p = fixture();
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    let s = xml(&p, "word/_rels/document.xml.rels");
    assert!(s.contains(r#"Target="footnotes.xml.backup""#));
    assert!(s.contains("https://example.org/FootnoteText"));
}

#[test]
fn relationship_attributes_may_use_single_quotes() {
    let mut p = fixture();
    p.set_part("word/_rels/document.xml.rels", format!("<Relationships xmlns='{R}'><Relationship Id='notes' Type='{REL}' Target='footnotes.xml'/></Relationships>").into_bytes());
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(xml(&p, "word/_rels/document.xml.rels").contains("endnotes.xml"));
}

#[test]
fn content_type_attributes_may_use_single_quotes() {
    let mut p = fixture();
    let ct = xml(&p, "[Content_Types].xml").replace('"', "'");
    p.set_part("[Content_Types].xml", ct.into_bytes());
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(xml(&p, "[Content_Types].xml").contains("/word/endnotes.xml"));
}

#[test]
fn absolute_internal_note_target_is_rewritten() {
    let mut p = fixture();
    p.set_part("word/_rels/document.xml.rels", format!(r#"<Relationships xmlns="{R}"><Relationship Id="notes" Type="{REL}" Target="/word/footnotes.xml"/></Relationships>"#).into_bytes());
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(xml(&p, "word/_rels/document.xml.rels").contains("endnotes.xml"));
    assert!(!xml(&p, "word/_rels/document.xml.rels").contains("footnotes.xml"));
}

#[test]
fn dotted_internal_note_target_is_rewritten() {
    let mut p = fixture();
    p.set_part("word/_rels/document.xml.rels", format!(r#"<Relationships xmlns="{R}"><Relationship Id="notes" Type="{REL}" Target="./footnotes.xml"/></Relationships>"#).into_bytes());
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(xml(&p, "word/_rels/document.xml.rels").contains("endnotes.xml"));
}

#[test]
fn alternate_word_namespace_prefix_is_supported() {
    let mut p = fixture();
    for part in ["word/document.xml", "word/footnotes.xml"] {
        let s = xml(&p, part).replace("xmlns:w=", "xmlns:q=").replace("w:", "q:");
        p.set_part(part, s.into_bytes());
    }
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(xml(&p, "word/endnotes.xml").contains("<q:endnotes"));
    assert!(xml(&p, "word/document.xml").contains("<q:endnoteReference"));
}

#[test]
fn foreign_namespace_names_are_preserved() {
    let mut p = fixture();
    let notes = format!(r#"<w:footnotes xmlns:w="{W}" xmlns:x="urn:foreign"><w:footnote w:id="1"><w:p><x:footnoteRef x:val="FootnoteReference"/><w:r><w:t>note</w:t></w:r></w:p></w:footnote></w:footnotes>"#);
    p.set_part("word/footnotes.xml", notes.into_bytes());
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(xml(&p, "word/endnotes.xml").contains(r#"x:val="FootnoteReference""#));
}

#[test]
fn malformed_part_conversion_is_transactional() {
    let mut p = fixture();
    p.set_part("word/document.xml", b"<w:document><w:body></w:document>".to_vec());
    let before = p.clone();
    assert!(convert_notes_kind(&mut p, NotesKind::Endnotes).is_err());
    assert_eq!(p.as_btree(), before.as_btree());
}

#[test]
fn colliding_destination_relationship_part_is_rejected_transactionally() {
    let mut p = fixture();
    p.set_part("word/_rels/footnotes.xml.rels", b"<Relationships/>".to_vec());
    p.set_part("word/_rels/endnotes.xml.rels", b"<different/>".to_vec());
    let before = p.clone();
    assert!(convert_notes_kind(&mut p, NotesKind::Endnotes).is_err());
    assert_eq!(p.as_btree(), before.as_btree());
}

#[test]
fn explicit_empty_note_elements_are_renamed() {
    let mut p = fixture();
    p.set_part("word/footnotes.xml", format!(r#"<w:footnotes xmlns:w="{W}"><w:footnote w:id="1"/></w:footnotes>"#).into_bytes());
    convert_notes_kind(&mut p, NotesKind::Endnotes).unwrap();
    assert!(xml(&p, "word/endnotes.xml").contains("<w:endnote "));
}
