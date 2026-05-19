//! Remove cosmetic *whole-paragraph* bold.
//!
//! Some authoring workflows (notably some RTF-from-template flows that
//! get round-tripped through Apple `textutil`) bold every run of every
//! body paragraph as a styling default rather than as semantic emphasis.
//! The result is that opening the file in Word shows every body
//! paragraph in bold, which is rarely what the author intended.
//!
//! This primitive walks each `<w:p>` in the document body and, if
//! **every** `<w:r>` in that paragraph has a `<w:b/>` (and optionally
//! `<w:bCs/>`) inside its `<w:rPr>`, strips them — turning the
//! paragraph into plain text while preserving genuine intra-paragraph
//! emphasis (paragraphs with mixed bold/non-bold runs are untouched).
//!
//! Companion to the Python `strip_paragraph_bold` in
//! `CrispTranslator/rtf_to_docx_endnotes.py`, which performs the
//! analogous transform at the Markdown level (`**...**` whole-paragraph
//! wrappers).

use std::io::Cursor;

use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::{Error, Result};
use crate::ns::PART_DOCUMENT;
use crate::package::Package;

/// Strip cosmetic whole-paragraph bold from `<w:p>` elements in the
/// document body. Returns the number of paragraphs unbolded.
pub fn strip_paragraph_bold(pkg: &mut Package) -> Result<usize> {
    let Some(input) = pkg.get_part(PART_DOCUMENT).map(<[u8]>::to_vec) else {
        return Ok(0);
    };
    let (new_bytes, stripped) = rewrite(&input)?;
    if stripped > 0 {
        pkg.set_part(PART_DOCUMENT, new_bytes);
    }
    Ok(stripped)
}

fn rewrite(input: &[u8]) -> Result<(Vec<u8>, usize)> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(input.len())));
    let mut buf = Vec::with_capacity(1024);

    // We buffer one paragraph at a time. When we close </w:p>, decide
    // whether every run in the paragraph was bold; if so, drop the bold
    // markers from each run's rPr.
    let mut in_para = false;
    let mut para_events: Vec<Event<'static>> = Vec::new();
    let mut stripped = 0usize;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: PART_DOCUMENT.into(),
                source: e,
            })?;
        match event {
            Event::Eof => break,
            Event::Start(s) if s.name().as_ref() == b"w:p" => {
                in_para = true;
                para_events.clear();
                para_events.push(Event::Start(s.into_owned()));
            }
            Event::End(e) if e.name().as_ref() == b"w:p" => {
                para_events.push(Event::End(e.into_owned()));
                let changed = if para_is_all_bold(&para_events) {
                    let n = drop_bold_in_runs(&mut para_events);
                    stripped += if n > 0 { 1 } else { 0 };
                    n > 0
                } else {
                    false
                };
                for ev in std::mem::take(&mut para_events) {
                    writer.write_event(ev).map_err(xml_io)?;
                }
                in_para = false;
                let _ = changed;
            }
            other if in_para => para_events.push(other.into_owned()),
            other => writer.write_event(other).map_err(xml_io)?,
        }
        buf.clear();
    }

    Ok((writer.into_inner().into_inner(), stripped))
}

fn xml_io(err: quick_xml::Error) -> Error {
    Error::XmlParse {
        part: PART_DOCUMENT.into(),
        source: err,
    }
}

/// True if every `<w:r>` in `events` has a `<w:b/>` inside its `<w:rPr>`.
/// Paragraphs with zero runs return false (nothing to strip).
fn para_is_all_bold(events: &[Event<'static>]) -> bool {
    let mut total_runs = 0;
    let mut bold_runs = 0;
    let mut in_run = false;
    let mut in_rpr = false;
    let mut current_has_bold = false;

    for ev in events {
        match ev {
            Event::Start(s) if s.name().as_ref() == b"w:r" => {
                in_run = true;
                in_rpr = false;
                current_has_bold = false;
            }
            Event::End(e) if e.name().as_ref() == b"w:r" => {
                total_runs += 1;
                if current_has_bold {
                    bold_runs += 1;
                }
                in_run = false;
            }
            Event::Start(s) if in_run && s.name().as_ref() == b"w:rPr" => {
                in_rpr = true;
            }
            Event::End(e) if in_run && e.name().as_ref() == b"w:rPr" => {
                in_rpr = false;
            }
            Event::Empty(e)
                if in_run && in_rpr && is_bold_marker(e) && !is_bold_off(e) =>
            {
                current_has_bold = true;
            }
            _ => {}
        }
    }
    total_runs > 0 && bold_runs == total_runs
}

fn is_bold_marker(e: &BytesStart) -> bool {
    matches!(e.name().as_ref(), b"w:b" | b"w:bCs")
}

/// Some docs encode "unbold" as `<w:b w:val="0"/>` — don't count that
/// as a bold marker.
fn is_bold_off(e: &BytesStart) -> bool {
    e.attributes()
        .filter_map(Result::ok)
        .any(|a| a.key.as_ref() == b"w:val" && matches!(a.value.as_ref(), b"0" | b"false"))
}

/// Drop `<w:b/>` / `<w:bCs/>` from every `<w:rPr>` in `events`. Returns
/// the count removed.
fn drop_bold_in_runs(events: &mut Vec<Event<'static>>) -> usize {
    let mut removed = 0usize;
    let mut out: Vec<Event<'static>> = Vec::with_capacity(events.len());
    let mut in_run = false;
    let mut in_rpr = false;
    for ev in std::mem::take(events) {
        match &ev {
            Event::Start(s) if s.name().as_ref() == b"w:r" => {
                in_run = true;
                in_rpr = false;
                out.push(ev);
            }
            Event::End(e) if e.name().as_ref() == b"w:r" => {
                in_run = false;
                out.push(ev);
            }
            Event::Start(s) if in_run && s.name().as_ref() == b"w:rPr" => {
                in_rpr = true;
                out.push(ev);
            }
            Event::End(e) if in_run && e.name().as_ref() == b"w:rPr" => {
                in_rpr = false;
                out.push(ev);
            }
            Event::Empty(e) if in_run && in_rpr && is_bold_marker(e) && !is_bold_off(e) => {
                removed += 1;
            }
            // Also handle the rare `<w:b></w:b>` form.
            Event::Start(s) if in_run && in_rpr && is_bold_marker(s) && !is_bold_off(s) => {
                // We need to also consume the matching end; let it pass
                // and rely on End handling below to drop it. Easier to
                // skip events: track that the bold span is being elided.
                let _close: BytesEnd =
                    BytesEnd::new(String::from_utf8_lossy(s.name().as_ref()).into_owned());
                removed += 1;
            }
            Event::End(e) if in_run && in_rpr && matches!(e.name().as_ref(), b"w:b" | b"w:bCs") => {
                // Was already swallowed by the paired Start above.
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
            body_inner = body_inner,
        )
        .into_bytes()
    }

    #[test]
    fn strips_whole_paragraph_bold() {
        let input = doc(
            r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>part one </w:t></w:r><w:r><w:rPr><w:b/><w:bCs/></w:rPr><w:t>part two</w:t></w:r></w:p>"#,
        );
        let (out, n) = rewrite(&input).unwrap();
        assert_eq!(n, 1);
        let s = std::str::from_utf8(&out).unwrap();
        assert!(!s.contains("<w:b/>"));
        assert!(!s.contains("<w:bCs/>"));
        assert!(s.contains("part one"));
        assert!(s.contains("part two"));
    }

    #[test]
    fn leaves_partial_bold_alone() {
        // First run bold, second run plain — must NOT be unbolded.
        let input = doc(
            r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>emphasis</w:t></w:r><w:r><w:t> tail</w:t></w:r></w:p>"#,
        );
        let (out, n) = rewrite(&input).unwrap();
        assert_eq!(n, 0);
        assert_eq!(&out, &input);
    }

    #[test]
    fn ignores_empty_paragraphs() {
        let input = doc(r#"<w:p></w:p>"#);
        let (_out, n) = rewrite(&input).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn handles_explicit_bold_off() {
        let input = doc(
            r#"<w:p><w:r><w:rPr><w:b w:val="0"/></w:rPr><w:t>not actually bold</w:t></w:r></w:p>"#,
        );
        let (_out, n) = rewrite(&input).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn multiple_paragraphs_handled_independently() {
        let input = doc(concat!(
            r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>all bold</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t>plain</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>also all bold</w:t></w:r></w:p>"#,
        ));
        let (out, n) = rewrite(&input).unwrap();
        assert_eq!(n, 2);
        let s = std::str::from_utf8(&out).unwrap();
        assert!(!s.contains("<w:b/>"));
        assert!(s.contains("plain"));
    }
}
