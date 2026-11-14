//! Text-only paragraph access with the same source indices and preservation
//! rules as the run API. Unchanged paragraphs retain exact original XML.
//! Changed simple paragraphs collapse to the first run's formatting; rich
//! paragraphs that cannot be represented by this operation fail atomically.

use crate::error::Result;
use crate::package::Package;
use crate::paragraph_runs::{extract_paragraphs, replace_paragraphs, Run};

/// Extract visible text from outer paragraphs, including empty paragraphs.
/// Formatting tab stops are excluded; run tabs and breaks are included.
pub fn extract_paragraph_texts(pkg: &Package) -> Result<Vec<String>> {
    Ok(extract_paragraphs(pkg)?
        .iter()
        .map(|p| p.full_text())
        .collect())
}

/// Replace simple paragraph text at matching indices. Preserve first-run
/// formatting, paragraph properties and note references. Unchanged paragraphs
/// and trailing indices retain their exact original XML, including rich data.
pub fn replace_paragraph_texts(pkg: &mut Package, new_texts: &[String]) -> Result<()> {
    let mut paragraphs = extract_paragraphs(pkg)?;
    for (p, text) in paragraphs.iter_mut().zip(new_texts) {
        if p.full_text() == *text {
            continue;
        }
        let first_rpr = p.runs.first().and_then(|r| r.rpr_xml.clone());
        let references = p
            .runs
            .iter()
            .flat_map(|r| r.footnote_refs.iter().cloned())
            .collect();
        p.runs = vec![Run {
            text: text.clone(),
            rpr_xml: first_rpr,
            footnote_refs: references,
        }];
    }
    replace_paragraphs(pkg, &paragraphs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Package;
    use std::io::{Cursor, Write};

    fn make_doc(body: &str) -> Vec<u8> {
        let buf = Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(buf);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("[Content_Types].xml", opts).unwrap();
        zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#)
            .unwrap();
        zw.start_file("word/document.xml", opts).unwrap();
        let doc = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{body}</w:document>"#
        );
        zw.write_all(doc.as_bytes()).unwrap();
        zw.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_single_paragraph_text() {
        let body = r#"<w:body><w:p><w:r><w:t>Hello.</w:t></w:r></w:p></w:body>"#;
        let pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let texts = extract_paragraph_texts(&pkg).unwrap();
        assert_eq!(texts, vec!["Hello.".to_string()]);
    }

    #[test]
    fn extracts_multiple_paragraphs_in_order() {
        let body = r#"<w:body><w:p><w:r><w:t>First.</w:t></w:r></w:p><w:p><w:r><w:t>Second.</w:t></w:r></w:p></w:body>"#;
        let pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let texts = extract_paragraph_texts(&pkg).unwrap();
        assert_eq!(texts, vec!["First.", "Second."]);
    }

    #[test]
    fn concatenates_split_runs_into_single_text() {
        let body = r#"<w:body><w:p><w:r><w:t>Hello </w:t></w:r><w:r><w:t>world.</w:t></w:r></w:p></w:body>"#;
        let pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let texts = extract_paragraph_texts(&pkg).unwrap();
        assert_eq!(texts, vec!["Hello world."]);
    }

    #[test]
    fn tabs_and_breaks_become_special_chars() {
        let body = r#"<w:body><w:p><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t></w:r></w:p></w:body>"#;
        let pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let texts = extract_paragraph_texts(&pkg).unwrap();
        assert_eq!(texts, vec!["a\tb\nc"]);
    }

    #[test]
    fn replace_swaps_paragraph_text() {
        let body = r#"<w:body><w:p><w:r><w:t>Hello.</w:t></w:r></w:p><w:p><w:r><w:t>World.</w:t></w:r></w:p></w:body>"#;
        let mut pkg = Package::from_bytes(&make_doc(body)).unwrap();
        replace_paragraph_texts(&mut pkg, &["Hallo.".into(), "Welt.".into()]).unwrap();
        let texts = extract_paragraph_texts(&pkg).unwrap();
        assert_eq!(texts, vec!["Hallo.", "Welt."]);
    }

    #[test]
    fn replace_preserves_paragraph_style_via_ppr() {
        let body = r#"<w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Old.</w:t></w:r></w:p></w:body>"#;
        let mut pkg = Package::from_bytes(&make_doc(body)).unwrap();
        replace_paragraph_texts(&mut pkg, &["New.".into()]).unwrap();
        let xml = pkg
            .get_part("word/document.xml")
            .map(String::from_utf8_lossy)
            .unwrap()
            .into_owned();
        assert!(xml.contains("Heading1"), "lost pStyle: {xml}");
        assert!(xml.contains("New."), "lost new text: {xml}");
    }

    #[test]
    fn replace_preserves_footnote_references() {
        let body = r#"<w:body><w:p><w:r><w:t>Cite</w:t><w:footnoteReference w:id="1"/></w:r></w:p></w:body>"#;
        let mut pkg = Package::from_bytes(&make_doc(body)).unwrap();
        replace_paragraph_texts(&mut pkg, &["Zitat".into()]).unwrap();
        let xml = pkg
            .get_part("word/document.xml")
            .map(String::from_utf8_lossy)
            .unwrap()
            .into_owned();
        assert!(xml.contains("Zitat"), "missing text: {xml}");
        assert!(
            xml.contains("w:footnoteReference") && xml.contains(r#"w:id="1""#),
            "lost footnote ref: {xml}"
        );
    }
}
