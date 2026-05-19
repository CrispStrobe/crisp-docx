//! Strip non-semantic `<w:rPr>` children from `<w:r>` runs.
//!
//! Verbatim port of `format_transplant.py::DocumentBuilder._clean_runs`:
//!
//! For each `<w:r>` in the body:
//!   - if it contains a `<w:footnoteReference>` or `<w:footnoteRef>` child
//!     at any depth, leave it ENTIRELY untouched;
//!   - otherwise, find its `<w:rPr>` and remove every child whose qualified
//!     name is NOT in [`KEEP_RPR_TAGS`].
//!
//! The KEEP set comes verbatim from `format_transplant.py:128`:
//!
//!   `w:b`  `w:bCs`  `w:i`  `w:iCs`  `w:u`  `w:strike`  `w:dstrike`
//!   `w:vertAlign`  `w:highlight`  `w:smallCaps`  `w:allCaps`  `w:em`  `w:vanish`
//!
//! Everything else (fonts, sizes, colors, language, kern, rStyle, …) is
//! stripped so that the package's styles.xml — typically the blueprint's —
//! governs the visual appearance.
//!
//! This primitive is what makes `transplant_body` produce a document whose
//! runs match the blueprint's defaults rather than carrying the source's
//! incidental font/size/color choices.

use std::io::Cursor;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::{Error, Result};
use crate::ns::{PART_DOCUMENT, PART_ENDNOTES, PART_FOOTNOTES};
use crate::package::Package;

/// Tags to preserve inside `<w:rPr>` — everything else is dropped.
///
/// Order matches `format_transplant.py:128-142` (KEEP_RPR_TAGS).
const KEEP_RPR_TAGS: &[&[u8]] = &[
    b"w:b",
    b"w:bCs",
    b"w:i",
    b"w:iCs",
    b"w:u",
    b"w:strike",
    b"w:dstrike",
    b"w:vertAlign",
    b"w:highlight",
    b"w:smallCaps",
    b"w:allCaps",
    b"w:em",
    b"w:vanish",
];

/// Strip non-semantic `<w:rPr>` children from every `<w:r>` in the document
/// body, footnotes, and endnotes.
///
/// Returns the total number of `<w:rPr>` child elements removed across all
/// parts. Runs that contain a `<w:footnoteReference>` or `<w:footnoteRef>`
/// are left untouched.
///
/// This mirrors `DocumentBuilder._clean_runs` from `format_transplant.py`.
pub fn clean_runs(pkg: &mut Package) -> Result<usize> {
    let mut total = 0usize;
    for part in [PART_DOCUMENT, PART_FOOTNOTES, PART_ENDNOTES] {
        let Some(bytes) = pkg.get_part(part).map(<[u8]>::to_vec) else {
            continue;
        };
        let (rewritten, removed) = rewrite_part(&bytes, part)?;
        if removed > 0 {
            pkg.set_part(part, rewritten);
            total += removed;
        }
    }
    Ok(total)
}

fn rewrite_part(input: &[u8], part_name: &str) -> Result<(Vec<u8>, usize)> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(input.len())));
    let mut buf = Vec::with_capacity(1024);
    let mut removed_total = 0usize;

    // Walk the stream. When we enter a <w:r>, buffer until we see </w:r>.
    // While buffered, also peek inside any nested <w:rPr> + check for
    // <w:footnoteReference> / <w:footnoteRef>. On </w:r>, decide whether to
    // emit the buffer verbatim or with rPr-filtering applied.
    let mut run_open: Option<BytesStart<'static>> = None;
    let mut run_events: Vec<Event<'static>> = Vec::new();

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: part_name.into(),
                source: e,
            })?;
        match ev {
            Event::Eof => break,

            Event::Start(s) if s.name().as_ref() == b"w:r" => {
                run_open = Some(s.into_owned());
                run_events.clear();
            }

            Event::End(e) if e.name().as_ref() == b"w:r" => {
                let open = run_open.take().expect("balanced <w:r>");
                let mut payload = run_events.clone();
                run_events.clear();
                let has_footnote_ref = payload_has_footnote_ref(&payload);
                let removed_here = if has_footnote_ref {
                    0
                } else {
                    filter_rpr_children(&mut payload)
                };
                removed_total += removed_here;

                writer
                    .write_event(Event::Start(open))
                    .map_err(|e| xml_io(e, part_name))?;
                for inner in payload.into_iter() {
                    writer
                        .write_event(inner)
                        .map_err(|e| xml_io(e, part_name))?;
                }
                writer
                    .write_event(Event::End(e.into_owned()))
                    .map_err(|e| xml_io(e, part_name))?;
            }

            other if run_open.is_some() => run_events.push(other.into_owned()),

            other => writer
                .write_event(other)
                .map_err(|e| xml_io(e, part_name))?,
        }
        buf.clear();
    }
    Ok((writer.into_inner().into_inner(), removed_total))
}

fn xml_io(err: quick_xml::Error, part: &str) -> Error {
    Error::XmlParse {
        part: part.into(),
        source: err,
    }
}

/// True if the run's payload contains `<w:footnoteReference>` or
/// `<w:footnoteRef>` at any depth — i.e. this is the inline reference
/// to a footnote. We never touch those runs' rPr.
fn payload_has_footnote_ref(payload: &[Event<'static>]) -> bool {
    payload.iter().any(|ev| match ev {
        Event::Start(s) | Event::Empty(s) => {
            matches!(s.name().as_ref(), b"w:footnoteReference" | b"w:footnoteRef")
        }
        _ => false,
    })
}

/// Walk the events buffered for one run; within any contained `<w:rPr>`,
/// drop element events (Start+End and Empty) whose qualified tag is not
/// in [`KEEP_RPR_TAGS`].
///
/// Returns the number of removals made.
fn filter_rpr_children(events: &mut Vec<Event<'static>>) -> usize {
    let mut out: Vec<Event<'static>> = Vec::with_capacity(events.len());
    let mut in_rpr = false;
    // depth tracker for matching Start/End within an unwanted element
    // so we can also strip its children (in practice rPr children are
    // self-closing, but be defensive).
    let mut dropping_depth = 0u32;
    let mut removed = 0usize;

    for ev in std::mem::take(events) {
        if dropping_depth > 0 {
            match &ev {
                Event::Start(_) => dropping_depth += 1,
                Event::End(_) => dropping_depth -= 1,
                _ => {}
            }
            continue;
        }
        match &ev {
            Event::Start(s) if s.name().as_ref() == b"w:rPr" => {
                in_rpr = true;
                out.push(ev);
            }
            Event::End(e) if e.name().as_ref() == b"w:rPr" => {
                in_rpr = false;
                out.push(ev);
            }
            Event::Empty(e) if in_rpr => {
                if !KEEP_RPR_TAGS.contains(&e.name().as_ref()) {
                    removed += 1;
                } else {
                    out.push(ev);
                }
            }
            Event::Start(s) if in_rpr => {
                if !KEEP_RPR_TAGS.contains(&s.name().as_ref()) {
                    removed += 1;
                    dropping_depth = 1;
                } else {
                    out.push(ev);
                }
            }
            _ => out.push(ev),
        }
    }
    *events = out;
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(body_inner: &str) -> Vec<u8> {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body_inner}</w:body></w:document>"#,
        )
        .into_bytes()
    }

    #[test]
    fn strips_fonts_sizes_colors_lang_rstyle() {
        let input = doc(
            r#"<w:p><w:r><w:rPr><w:rFonts w:ascii="Arial"/><w:sz w:val="24"/><w:szCs w:val="24"/><w:color w:val="FF0000"/><w:lang w:val="en-US"/><w:rStyle w:val="MyStyle"/><w:b/><w:i/></w:rPr><w:t>text</w:t></w:r></w:p>"#,
        );
        let (out, n) = rewrite_part(&input, "word/document.xml").unwrap();
        // 6 non-semantic children removed (rFonts, sz, szCs, color, lang, rStyle).
        assert_eq!(n, 6);
        let s = std::str::from_utf8(&out).unwrap();
        // Semantic tags survive
        assert!(s.contains("<w:b/>"));
        assert!(s.contains("<w:i/>"));
        // Non-semantic tags gone
        for needle in [
            "w:rFonts", "w:sz ", "w:szCs", "w:color", "w:lang", "w:rStyle",
        ] {
            assert!(!s.contains(needle), "should be gone: {needle}");
        }
        // Text content preserved
        assert!(s.contains(">text<"));
    }

    #[test]
    fn preserves_footnote_reference_run_verbatim() {
        let input = doc(
            r#"<w:p><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/><w:rFonts w:ascii="Arial"/><w:sz w:val="20"/></w:rPr><w:footnoteReference w:id="1"/></w:r></w:p>"#,
        );
        let (out, n) = rewrite_part(&input, "word/document.xml").unwrap();
        assert_eq!(n, 0, "footnote-reference run should be left alone");
        let s = std::str::from_utf8(&out).unwrap();
        // EVERYTHING preserved on footnote ref run, including rFonts/sz.
        assert!(s.contains("w:rStyle"));
        assert!(s.contains("w:rFonts"));
        assert!(s.contains("w:sz "));
        assert!(s.contains("w:footnoteReference"));
    }

    #[test]
    fn preserves_footnote_text_run_marker_verbatim() {
        // The first run of a <w:footnote>'s body holds <w:footnoteRef/> —
        // also preserved verbatim per the Python implementation.
        let input = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:id="1"><w:p><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/><w:rFonts w:ascii="Times"/></w:rPr><w:footnoteRef/></w:r></w:p></w:footnote></w:footnotes>"#.to_string();
        let (out, n) = rewrite_part(input.as_bytes(), "word/footnotes.xml").unwrap();
        assert_eq!(n, 0, "footnoteRef run should be left alone");
        let s = std::str::from_utf8(&out).unwrap();
        assert!(s.contains("w:rFonts"));
        assert!(s.contains("w:rStyle"));
    }

    #[test]
    fn empty_rpr_is_left_alone() {
        let input = doc(r#"<w:p><w:r><w:rPr/><w:t>x</w:t></w:r></w:p>"#);
        let (_out, n) = rewrite_part(&input, "word/document.xml").unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn run_without_rpr_is_left_alone() {
        let input = doc(r#"<w:p><w:r><w:t>plain</w:t></w:r></w:p>"#);
        let (_out, n) = rewrite_part(&input, "word/document.xml").unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn keep_set_matches_python_format_transplant() {
        // Lock the KEEP set to format_transplant.py:128-142 so future
        // edits to this list must update both sides consciously.
        let expected: &[&[u8]] = &[
            b"w:b",
            b"w:bCs",
            b"w:i",
            b"w:iCs",
            b"w:u",
            b"w:strike",
            b"w:dstrike",
            b"w:vertAlign",
            b"w:highlight",
            b"w:smallCaps",
            b"w:allCaps",
            b"w:em",
            b"w:vanish",
        ];
        assert_eq!(KEEP_RPR_TAGS, expected);
    }
}
