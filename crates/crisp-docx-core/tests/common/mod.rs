//! Shared helpers for integration tests.
//!
//! Builds minimal `.docx` packages in-memory so tests don't need any
//! external corpus. The fixtures are intentionally tiny — they exercise
//! one structural shape per primitive.

use std::io::{Cursor, Write};

/// Make a docx whose body contains a single `<w:p>` whose attributes carry
/// every rsid / paraId variant we strip.
pub fn docx_with_rsids() -> Vec<u8> {
    let body = br#"<w:p w14:paraId="A1B2" w14:textId="C3D4" w:rsidR="00112233" w:rsidRDefault="44556677" w:rsidRPr="DEADBEEF" w:rsidP="55667788"><w:r w:rsidR="11223344" w:rsidRPr="99887766"><w:t xml:space="preserve">hello</w:t></w:r></w:p>"#;
    zip_package(&[
        ("[Content_Types].xml", CONTENT_TYPES_MINIMAL.as_bytes()),
        ("word/document.xml", &build_document(body)),
    ])
}

/// Make a docx whose runs use Apple textutil's non-standard tag names.
pub fn docx_with_textutil_tags() -> Vec<u8> {
    let body = br#"<w:p><w:r><w:rPr><w:rFonts w:ascii="Arial"/><w:sz w:val="28"/><w:sz-cs w:val="28"/><w:b/><w:b-cs/><w:i/><w:i-cs/></w:rPr><w:t>hi</w:t></w:r></w:p>"#;
    zip_package(&[
        ("[Content_Types].xml", CONTENT_TYPES_MINIMAL.as_bytes()),
        ("word/document.xml", &build_document(body)),
    ])
}

/// Make a docx with a working footnotes part (id=1) referenced from the body.
pub fn docx_with_footnotes() -> Vec<u8> {
    let body = br#"<w:p><w:r><w:t xml:space="preserve">Body. </w:t></w:r><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteReference w:id="1"/></w:r><w:r><w:t xml:space="preserve"> after.</w:t></w:r></w:p>"#;
    let footnotes = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:id="-1" w:type="separator"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="0" w:type="continuationSeparator"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> the note body</w:t></w:r></w:p></w:footnote></w:footnotes>"#;
    let rels = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="footnotes.xml"/></Relationships>"#;
    let content_types = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/footnotes.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml"/></Types>"#;
    zip_package(&[
        ("[Content_Types].xml", content_types),
        ("word/document.xml", &build_document(body)),
        ("word/footnotes.xml", footnotes),
        ("word/_rels/document.xml.rels", rels),
    ])
}

const CONTENT_TYPES_MINIMAL: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;

fn build_document(body_inner: &[u8]) -> Vec<u8> {
    let mut v = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><w:body>"#.to_vec();
    v.extend_from_slice(body_inner);
    v.extend_from_slice(b"</w:body></w:document>");
    v
}

fn zip_package(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let buf = Cursor::new(Vec::new());
    let mut zw = zip::ZipWriter::new(buf);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zw.start_file(*name, opts).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap().into_inner()
}
