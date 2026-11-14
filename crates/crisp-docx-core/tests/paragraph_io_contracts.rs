//! Controls for lossless no-op and bounded paragraph replacement.
use crisp_docx_core::{
    extract_paragraph_runs, extract_paragraph_texts, replace_paragraph_runs,
    replace_paragraph_texts, Package,
};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn package(body: &str) -> Package {
    let mut p = Package::new();
    p.set_part(
        "[Content_Types].xml",
        br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#
            .to_vec(),
    );
    p.set_part("word/document.xml", format!(r#"<w:document xmlns:w="{W}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}</w:body></w:document>"#).into_bytes());
    p.set_part("word/media/preserved.bin", vec![0, 1, 2, 3]);
    p
}

const RICH: &[&str] = &[
    r#"<w:p><w:hyperlink r:id="link"><w:r><w:t>link</w:t></w:r></w:hyperlink></w:p>"#,
    r#"<w:p><w:r><w:drawing><w:txbxContent><w:p><w:r><w:t>box</w:t></w:r></w:p></w:txbxContent></w:drawing></w:r><w:r><w:t>tail</w:t></w:r></w:p>"#,
    r#"<w:p><w:ins w:id="4" w:author="Editor"><w:r><w:t>inserted</w:t></w:r></w:ins><w:del w:id="5" w:author="Editor"><w:r><w:delText>deleted</w:delText></w:r></w:del></w:p>"#,
    r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/><w:instrText>DATE</w:instrText><w:fldChar w:fldCharType="separate"/><w:t>cached</w:t><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    r#"<w:p><w:r><w:t>before</w:t></w:r><w:bookmarkStart w:id="7" w:name="mid"/><w:r><w:t>inside</w:t></w:r><w:bookmarkEnd w:id="7"/><w:r><w:t>after</w:t></w:r></w:p>"#,
    r#"<w:p><w:sdt><w:sdtContent><w:r><w:t>control</w:t></w:r></w:sdtContent></w:sdt></w:p>"#,
    r#"<w:p><w:r><w:t>page</w:t><w:br w:type="page"/><w:t>after</w:t></w:r></w:p>"#,
];

#[test]
fn unchanged_text_preserves_every_part_byte_for_byte() {
    for body in RICH {
        let mut p = package(body);
        let before = p.clone();
        let texts = extract_paragraph_texts(&p).unwrap();
        replace_paragraph_texts(&mut p, &texts).unwrap();
        assert_eq!(p.as_btree(), before.as_btree(), "{body}");
    }
}

#[test]
fn unchanged_runs_preserve_every_part_byte_for_byte() {
    for body in RICH {
        let mut p = package(body);
        let before = p.clone();
        let runs = extract_paragraph_runs(&p).unwrap();
        replace_paragraph_runs(&mut p, &runs).unwrap();
        assert_eq!(p.as_btree(), before.as_btree(), "{body}");
    }
}

#[test]
fn changed_rich_text_is_rejected_atomically() {
    for body in RICH {
        let mut p = package(&format!("<w:p><w:r><w:t>plain</w:t></w:r></w:p>{body}"));
        let before = p.clone();
        let mut texts = extract_paragraph_texts(&p).unwrap();
        texts[0] = "first change".into();
        texts[1] = "cannot flatten this".into();
        let err = replace_paragraph_texts(&mut p, &texts).unwrap_err();
        assert!(err.to_string().contains("cannot preserve"), "{err}");
        assert_eq!(p.as_btree(), before.as_btree(), "{body}");
    }
}

#[test]
fn changed_rich_runs_are_rejected_atomically() {
    for body in RICH {
        let mut p = package(body);
        let before = p.clone();
        let mut runs = extract_paragraph_runs(&p).unwrap();
        runs[0].runs[0].text = "change".into();
        let err = replace_paragraph_runs(&mut p, &runs).unwrap_err();
        assert!(err.to_string().contains("cannot preserve"), "{err}");
        assert_eq!(p.as_btree(), before.as_btree(), "{body}");
    }
}

#[test]
fn empty_paragraphs_have_stable_indices() {
    let mut p = package("<w:p/><w:p></w:p><w:tbl><w:tr><w:tc><w:p/><w:p><w:r><w:t>tail</w:t></w:r></w:p></w:tc></w:tr></w:tbl>");
    assert_eq!(extract_paragraph_texts(&p).unwrap(), ["", "", "", "tail"]);
    assert_eq!(extract_paragraph_runs(&p).unwrap().len(), 4);
    replace_paragraph_texts(&mut p, &["one".into(), "two".into(), "three".into()]).unwrap();
    assert_eq!(
        extract_paragraph_texts(&p).unwrap(),
        ["one", "two", "three", "tail"]
    );
}

#[test]
fn nested_textboxes_do_not_terminate_the_outer_paragraph() {
    let p = package(&format!("{}<w:p><w:r><w:t>next</w:t></w:r></w:p>", RICH[1]));
    assert_eq!(extract_paragraph_texts(&p).unwrap(), ["boxtail", "next"]);
    let runs = extract_paragraph_runs(&p).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].full_text(), "boxtail");
    assert_eq!(runs[1].full_text(), "next");
}

#[test]
fn paragraph_tab_stops_are_not_visible_text() {
    let p = package(
        r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr><w:r><w:t>A</w:t><w:tab/><w:t>B</w:t></w:r></w:p>"#,
    );
    assert_eq!(extract_paragraph_texts(&p).unwrap(), ["A\tB"]);
    assert_eq!(extract_paragraph_runs(&p).unwrap()[0].full_text(), "A\tB");
}

#[test]
fn changed_text_keeps_the_first_run_format() {
    let mut p = package(
        r#"<w:p><w:r><w:rPr><w:b/><w:color w:val="123456"/></w:rPr><w:t>old</w:t></w:r></w:p>"#,
    );
    replace_paragraph_texts(&mut p, &["new".into()]).unwrap();
    let runs = extract_paragraph_runs(&p).unwrap();
    assert_eq!(runs[0].full_text(), "new");
    let rpr = std::str::from_utf8(runs[0].runs[0].rpr_xml.as_ref().unwrap()).unwrap();
    assert!(rpr.contains("123456") && rpr.contains("w:b"));
}

#[test]
fn changed_plain_paragraph_leaves_adjacent_rich_xml_untouched() {
    let body = format!("<w:p><w:r><w:t>old</w:t></w:r></w:p>{}", RICH[1]);
    let mut p = package(&body);
    replace_paragraph_texts(&mut p, &["new".into()]).unwrap();
    let xml = std::str::from_utf8(p.get_part("word/document.xml").unwrap()).unwrap();
    assert!(xml.contains(RICH[1]));
    assert_eq!(
        p.get_part("word/media/preserved.bin"),
        Some([0, 1, 2, 3].as_slice())
    );
}

#[test]
fn endnote_references_are_preserved_when_plain_text_changes() {
    let mut p = package(r#"<w:p><w:r><w:t>old</w:t><w:endnoteReference w:id="7"/></w:r></w:p>"#);
    replace_paragraph_texts(&mut p, &["new".into()]).unwrap();
    let xml = std::str::from_utf8(p.get_part("word/document.xml").unwrap()).unwrap();
    assert!(xml.contains("w:endnoteReference") && xml.contains("w:id=\"7\""));
}

#[test]
fn replacement_controls_are_ooxml_elements() {
    let mut p = package("<w:p><w:r><w:t>old</w:t></w:r></w:p>");
    replace_paragraph_texts(&mut p, &["A\tB\nC".into()]).unwrap();
    let xml = std::str::from_utf8(p.get_part("word/document.xml").unwrap()).unwrap();
    assert!(xml.contains("<w:tab/>") && xml.contains("<w:br/>"));
    assert_eq!(extract_paragraph_texts(&p).unwrap(), ["A\tB\nC"]);
}

#[test]
fn invalid_text_entities_are_errors_in_both_extractors() {
    let p = package("<w:p><w:r><w:t>bad &notDefined;</w:t></w:r></w:p>");
    assert!(extract_paragraph_texts(&p).is_err());
    assert!(extract_paragraph_runs(&p).is_err());
}

#[test]
fn invalid_xml_characters_reject_without_mutation() {
    let mut p = package("<w:p><w:r><w:t>old</w:t></w:r></w:p>");
    let before = p.clone();
    assert!(replace_paragraph_texts(&mut p, &["bad\0text".into()]).is_err());
    assert_eq!(p.as_btree(), before.as_btree());
}

#[test]
fn alternate_word_prefix_can_be_extracted_and_updated() {
    let mut p = package("<w:p><w:r><w:t>old</w:t></w:r></w:p>");
    let xml = std::str::from_utf8(p.get_part("word/document.xml").unwrap())
        .unwrap()
        .replace("xmlns:w=", "xmlns:q=")
        .replace("w:", "q:");
    p.set_part("word/document.xml", xml.into_bytes());
    assert_eq!(extract_paragraph_texts(&p).unwrap(), ["old"]);
    replace_paragraph_texts(&mut p, &["new".into()]).unwrap();
    assert_eq!(extract_paragraph_texts(&p).unwrap(), ["new"]);
    let xml = std::str::from_utf8(p.get_part("word/document.xml").unwrap()).unwrap();
    assert!(xml.contains("<q:r>") && !xml.contains("<w:r>"));
}

#[test]
fn nested_previous_run_properties_are_preserved_whole() {
    let p = package(
        r#"<w:p><w:r><w:rPr><w:b/><w:rPrChange w:id="1" w:author="Editor"><w:rPr><w:i/></w:rPr></w:rPrChange></w:rPr><w:t>x</w:t></w:r></w:p>"#,
    );
    let runs = extract_paragraph_runs(&p).unwrap();
    let xml = std::str::from_utf8(runs[0].runs[0].rpr_xml.as_ref().unwrap()).unwrap();
    assert!(xml.ends_with("</w:rPrChange></w:rPr>"));
}
