//! Strip revision-session tracking attributes (`w14:paraId`, `w:rsidR`, …)
//! from every `<w:p>` and `<w:r>` element in the document, footnotes, and
//! endnotes parts.
//!
//! Word's strict validator rejects documents whose body references session
//! IDs that aren't listed in `settings.xml`'s `<w:rsids>`. The most common
//! way for this to happen is grafting a body fragment from one document
//! into another (transplant scenarios) or recovering content from a
//! partially-corrupt file. Stripping is safe — Word regenerates fresh
//! IDs on next save. Cure for the "_Word found unreadable content_"
//! recovery dialog.

use std::io::Cursor;

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::{Error, Result};
use crate::ns::{PART_DOCUMENT, PART_ENDNOTES, PART_FOOTNOTES};
use crate::package::Package;

/// Attribute (qualified-name) keys that we drop from `<w:p>` and `<w:r>`.
const RSID_ATTRS: &[&[u8]] = &[
    b"w14:paraId",
    b"w14:textId",
    b"w:rsidR",
    b"w:rsidRPr",
    b"w:rsidDel",
    b"w:rsidRDefault",
    b"w:rsidP",
    b"w:rsidTr",
    b"w:rsidSect",
];

/// Strip rsid/paraId tracking attributes from every body, footnote, and
/// endnote XML part in `pkg`. Returns the total number of attributes
/// removed (across all parts).
///
/// On `Ok(0)` the package is byte-identical to its input. On `Ok(n > 0)`
/// the parts have been rewritten in place.
pub fn strip_rsids(pkg: &mut Package) -> Result<usize> {
    let mut total = 0usize;
    for part in [PART_DOCUMENT, PART_FOOTNOTES, PART_ENDNOTES] {
        let Some(bytes) = pkg.get_part(part) else {
            continue;
        };
        let (rewritten, removed) = strip_rsids_in_xml(bytes, part)?;
        if removed > 0 {
            pkg.set_part(part, rewritten);
            total += removed;
        }
    }
    Ok(total)
}

/// Pure function over a single XML part. Returns `(new_bytes, removed_count)`.
///
/// If nothing matched, `new_bytes` is allocated but byte-equivalent to the
/// input; callers should check `removed_count == 0` and skip writing back.
fn strip_rsids_in_xml(input: &[u8], part_name: &str) -> Result<(Vec<u8>, usize)> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;

    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(input.len())));
    let mut buf = Vec::with_capacity(1024);
    let mut removed = 0usize;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: part_name.into(),
                source: e,
            })?;
        match event {
            Event::Start(mut e) => {
                removed += filter_rsid_attrs(&mut e);
                writer
                    .write_event(Event::Start(e))
                    .map_err(|e| io_to_xml(e, part_name))?;
            }
            Event::Empty(mut e) => {
                removed += filter_rsid_attrs(&mut e);
                writer
                    .write_event(Event::Empty(e))
                    .map_err(|e| io_to_xml(e, part_name))?;
            }
            Event::Eof => break,
            other => writer
                .write_event(other)
                .map_err(|e| io_to_xml(e, part_name))?,
        }
        buf.clear();
    }

    Ok((writer.into_inner().into_inner(), removed))
}

fn io_to_xml(err: quick_xml::Error, part: &str) -> Error {
    Error::XmlParse {
        part: part.into(),
        source: err,
    }
}

/// Walk an element's attributes; drop those whose qualified name appears in
/// [`RSID_ATTRS`]. Returns the number of attrs removed.
///
/// quick-xml requires us to consume the existing attributes, decide on
/// which to keep, then rebuild the element via [`BytesStart::with_attributes`].
fn filter_rsid_attrs(start: &mut BytesStart) -> usize {
    // Collect attributes we keep into a vec; quick-xml's iterator can't be
    // mutated in place because attribute parsing borrows from the same
    // backing buffer that `start.set_attributes` would write to.
    let kept: Vec<_> = start
        .attributes()
        .filter_map(|a| a.ok())
        .filter(|a: &Attribute| !RSID_ATTRS.contains(&a.key.as_ref()))
        .map(|a| (a.key.as_ref().to_owned(), a.value.into_owned()))
        .collect();
    // If nothing changed we can avoid the rebuild.
    let original_count = start.attributes().filter_map(|a| a.ok()).count();
    let removed = original_count - kept.len();
    if removed == 0 {
        return 0;
    }
    // Rebuild attribute list. BytesStart needs a fresh attribute set; we
    // clear and re-push.
    start.clear_attributes();
    for (k, v) in kept {
        start.push_attribute((k.as_slice(), v.as_slice()));
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml">"#;
    const TAIL: &[u8] = b"</w:document>";

    fn doc(body_xml: &[u8]) -> Vec<u8> {
        let mut v = HEAD.to_vec();
        v.extend_from_slice(b"<w:body>");
        v.extend_from_slice(body_xml);
        v.extend_from_slice(b"</w:body>");
        v.extend_from_slice(TAIL);
        v
    }

    #[test]
    fn strips_known_rsid_attrs() {
        let body = br#"<w:p w14:paraId="A1B2" w14:textId="C3D4" w:rsidR="00112233" w:rsidRDefault="44556677" w:rsidRPr="DEADBEEF"><w:r w:rsidR="11223344" w:rsidRPr="99887766"><w:t xml:space="preserve">hello</w:t></w:r></w:p>"#;
        let input = doc(body);
        let (out, removed) = strip_rsids_in_xml(&input, "word/document.xml").unwrap();
        assert_eq!(removed, 7);
        let s = std::str::from_utf8(&out).unwrap();
        for needle in ["paraId", "textId", "rsidR", "rsidRDefault", "rsidRPr"] {
            assert!(!s.contains(needle), "should be gone: {needle}");
        }
        assert!(s.contains("<w:t"));
        assert!(s.contains("hello"));
    }

    #[test]
    fn noop_when_nothing_to_strip() {
        let body = br#"<w:p><w:r><w:t>plain</w:t></w:r></w:p>"#;
        let input = doc(body);
        let (_out, removed) = strip_rsids_in_xml(&input, "word/document.xml").unwrap();
        assert_eq!(removed, 0);
    }

    // Note: quick-xml is intentionally lenient with malformed input in
    // streaming mode, so there's no clean "broken XML -> Error" test we can
    // assert on the XML path itself. The package layer catches broken zips
    // (Error::InvalidPackage), which is the realistic failure mode.
}
