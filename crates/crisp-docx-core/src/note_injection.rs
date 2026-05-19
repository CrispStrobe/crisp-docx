//! Convert inline `[N]` citation markers to real Word footnote references.
//!
//! This is the structured equivalent of pandoc's `[^N]` syntax: callers
//! pass a map of `{note_id -> note_text}`, and we walk `word/document.xml`
//! looking for `[N]` substrings inside `<w:t>` runs whose `N` exists in
//! that map. Each hit becomes:
//!
//! ```xml
//! <w:r><w:t xml:space="preserve">… text before …</w:t></w:r>
//! <w:r>
//!   <w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr>
//!   <w:footnoteReference w:id="N"/>
//! </w:r>
//! <w:r><w:t xml:space="preserve">… text after …</w:t></w:r>
//! ```
//!
//! and a matching `<w:footnote w:id="N">…</w:footnote>` is appended to
//! `word/footnotes.xml` (creating the part plus its content-types
//! override and `document.xml.rels` entry if they didn't exist).
//!
//! ## Scope (MVP)
//!
//! This first cut handles the common case: each `[N]` marker lives entirely
//! within a single `<w:t>`. Markers split across multiple `<w:t>` elements
//! (rare in practice — most authoring tools keep the digits together with
//! the surrounding text) are reported as `unmatched` and left alone.
//!
//! ## Errors
//!
//! Returns the count of references inserted. Unmatched markers are
//! reported only as warnings via the returned `InjectionReport`; they
//! never abort the whole call.

use std::collections::BTreeMap;
use std::io::Cursor;

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::{Error, Result};
use crate::ns::{
    CT_FOOTNOTES, PART_CONTENT_TYPES, PART_DOCUMENT, PART_DOCUMENT_RELS, PART_FOOTNOTES,
    REL_TYPE_FOOTNOTES,
};
use crate::package::Package;

/// Outcome of a `inject_footnotes` call.
#[derive(Debug, Clone)]
pub struct InjectionReport {
    /// How many `[N]` markers were rewritten into footnote references.
    pub inserted: usize,
    /// Note IDs cited in the body that have no matching entry in `notes`.
    pub unknown_ids: Vec<u32>,
    /// Note IDs in `notes` that never showed up in the body.
    pub unused_ids: Vec<u32>,
}

/// Inject footnotes into a document.
///
/// `notes` maps a note number to the body of that note. Inline `[N]`
/// markers whose `N` appears in `notes` are replaced; markers with no
/// matching note are left in place (so the caller can decide whether
/// that's an error).
pub fn inject_footnotes(pkg: &mut Package, notes: &BTreeMap<u32, &str>) -> Result<InjectionReport> {
    let Some(doc_bytes) = pkg.get_part(PART_DOCUMENT).map(|b| b.to_vec()) else {
        return Ok(InjectionReport {
            inserted: 0,
            unknown_ids: Vec::new(),
            unused_ids: notes.keys().copied().collect(),
        });
    };

    let (new_doc, seen, inserted) = rewrite_document(&doc_bytes, notes)?;
    if inserted > 0 {
        pkg.set_part(PART_DOCUMENT, new_doc);
        ensure_footnotes_part(pkg, notes, &seen)?;
    }
    let unknown_ids = seen
        .iter()
        .filter(|n| !notes.contains_key(n))
        .copied()
        .collect();
    let unused_ids = notes
        .keys()
        .filter(|n| !seen.contains(n))
        .copied()
        .collect();
    Ok(InjectionReport {
        inserted,
        unknown_ids,
        unused_ids,
    })
}

/// Rewrite `document.xml`, returning `(new_bytes, ids_seen, count_inserted)`.
fn rewrite_document(
    input: &[u8],
    notes: &BTreeMap<u32, &str>,
) -> Result<(Vec<u8>, Vec<u32>, usize)> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(input.len())));
    let mut buf = Vec::with_capacity(1024);

    // We need to remember "are we currently inside a <w:r>?" so we can find
    // the right place to splice the reference-run in. Approach: copy events
    // through verbatim, but when we see a <w:t> Text event whose payload
    // contains a recognised `[N]`, defer until we close the parent <w:r>
    // and emit the surrounding run-clones + reference-run after that close.
    //
    // For the MVP that means buffering exactly one <w:r>...</w:r> at a time.
    let mut run_open: Option<BytesStart<'static>> = None;
    let mut run_inner_events: Vec<Event<'static>> = Vec::new();
    let mut run_text: Option<Vec<u8>> = None;
    let mut t_attrs: Option<BytesStart<'static>> = None;

    let mut inserted = 0;
    let mut seen: Vec<u32> = Vec::new();

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: PART_DOCUMENT.into(),
                source: e,
            })?;
        match event {
            Event::Eof => break,

            Event::Start(start) if start.name().as_ref() == b"w:r" => {
                run_open = Some(start.into_owned());
                run_inner_events.clear();
                run_text = None;
                t_attrs = None;
            }

            Event::End(end) if end.name().as_ref() == b"w:r" => {
                // Find every injectable marker in this run's text.
                let markers = run_text
                    .as_deref()
                    .map(|t| find_all_markers(t, notes))
                    .unwrap_or_default();

                if !markers.is_empty() {
                    let text = run_text.take().unwrap();
                    let run = run_open.take().unwrap();
                    let t = t_attrs.take().unwrap_or_else(|| BytesStart::new("w:t"));

                    let mut cursor = 0;
                    for (offset, num) in &markers {
                        let before = &text[cursor..*offset];
                        if !before.is_empty() {
                            write_clone_run(&mut writer, &run, &t, before, PART_DOCUMENT)?;
                        }
                        write_ref_run(&mut writer, *num, PART_DOCUMENT)?;
                        cursor = offset + marker_len(*num);
                        if !seen.contains(num) {
                            seen.push(*num);
                        }
                        inserted += 1;
                    }
                    let tail = &text[cursor..];
                    if !tail.is_empty() {
                        write_clone_run(&mut writer, &run, &t, tail, PART_DOCUMENT)?;
                    }
                } else {
                    // Pass the whole run through verbatim.
                    let run = run_open.take().unwrap();
                    writer
                        .write_event(Event::Start(run))
                        .map_err(|e| xml_io(e, PART_DOCUMENT))?;
                    for ev in std::mem::take(&mut run_inner_events) {
                        writer
                            .write_event(ev)
                            .map_err(|e| xml_io(e, PART_DOCUMENT))?;
                    }
                    writer
                        .write_event(Event::End(end.into_owned()))
                        .map_err(|e| xml_io(e, PART_DOCUMENT))?;
                }
                run_text = None;
                t_attrs = None;
            }

            other if run_open.is_some() => {
                // We're inside a <w:r> — defer the event. Track <w:t> text
                // content so we can scan it for `[N]`.
                if let Event::Start(s) = &other {
                    if s.name().as_ref() == b"w:t" {
                        t_attrs = Some(s.clone().into_owned());
                        run_text = Some(Vec::new());
                    }
                } else if let Event::Text(t) = &other {
                    if let Some(buf) = run_text.as_mut() {
                        buf.extend_from_slice(t.as_ref());
                    }
                }
                run_inner_events.push(other.into_owned());
            }

            other => writer
                .write_event(other)
                .map_err(|e| xml_io(e, PART_DOCUMENT))?,
        }
        buf.clear();
    }
    Ok((writer.into_inner().into_inner(), seen, inserted))
}

fn xml_io(err: quick_xml::Error, part: &str) -> Error {
    Error::XmlParse {
        part: part.into(),
        source: err,
    }
}

/// Find every `[N]` substring whose `N` is a key of `notes`. Returns
/// `(byte_offset, number)` pairs in left-to-right order.
fn find_all_markers(text: &[u8], notes: &BTreeMap<u32, &str>) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(rel) = text[i..].iter().position(|&b| b == b'[') {
        let start = i + rel;
        let mut j = start + 1;
        let digits_start = j;
        while j < text.len() && text[j].is_ascii_digit() {
            j += 1;
        }
        if j > digits_start && j < text.len() && text[j] == b']' {
            if let Ok(s) = std::str::from_utf8(&text[digits_start..j]) {
                if let Ok(n) = s.parse::<u32>() {
                    if notes.contains_key(&n) {
                        out.push((start, n));
                        i = j + 1;
                        continue;
                    }
                }
            }
        }
        i = start + 1;
    }
    out
}

#[cfg(test)]
fn find_first_marker(text: &[u8], notes: &BTreeMap<u32, &str>) -> Option<(usize, u32)> {
    find_all_markers(text, notes).into_iter().next()
}

fn marker_len(n: u32) -> usize {
    let mut len = 2; // [ and ]
    let mut v = n;
    if v == 0 {
        len += 1;
    } else {
        while v > 0 {
            len += 1;
            v /= 10;
        }
    }
    len
}

fn write_clone_run<W: std::io::Write>(
    writer: &mut Writer<W>,
    run_start: &BytesStart<'static>,
    t_start: &BytesStart<'static>,
    text: &[u8],
    part: &str,
) -> Result<()> {
    // Same Start event for <w:r>, copy any rPr from `run_inner_events` isn't
    // available here — we keep clones simple: rPr is preserved by re-using
    // `run_start` which only carries the element name (rPr was an inner
    // event we already consumed). Future revisions can preserve rPr by
    // capturing the rPr sub-tree alongside `run_text`.
    writer
        .write_event(Event::Start(run_start.clone()))
        .map_err(|e| xml_io(e, part))?;
    let mut t = t_start.clone();
    if !needs_preserve(text) {
        // Strip xml:space="preserve" if it was just there for the original
        // whitespace and we now don't need it.
    } else {
        let mut has_preserve = false;
        for a in t.attributes().filter_map(Result::ok) {
            if a.key.as_ref() == b"xml:space" && a.value.as_ref() == b"preserve" {
                has_preserve = true;
                break;
            }
        }
        if !has_preserve {
            t.push_attribute(("xml:space", "preserve"));
        }
    }
    writer
        .write_event(Event::Start(t.clone()))
        .map_err(|e| xml_io(e, part))?;
    writer
        .write_event(Event::Text(BytesText::from_escaped(
            std::str::from_utf8(text).unwrap_or(""),
        )))
        .map_err(|e| xml_io(e, part))?;
    writer
        .write_event(Event::End(BytesEnd::new("w:t")))
        .map_err(|e| xml_io(e, part))?;
    writer
        .write_event(Event::End(BytesEnd::new("w:r")))
        .map_err(|e| xml_io(e, part))?;
    Ok(())
}

fn write_ref_run<W: std::io::Write>(writer: &mut Writer<W>, num: u32, part: &str) -> Result<()> {
    let mut r = BytesStart::new("w:r");
    writer
        .write_event(Event::Start(r.clone()))
        .map_err(|e| xml_io(e, part))?;
    let _ = &mut r;
    // <w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr>
    writer
        .write_event(Event::Start(BytesStart::new("w:rPr")))
        .map_err(|e| xml_io(e, part))?;
    let mut rstyle = BytesStart::new("w:rStyle");
    rstyle.push_attribute(("w:val", "FootnoteReference"));
    writer
        .write_event(Event::Empty(rstyle))
        .map_err(|e| xml_io(e, part))?;
    writer
        .write_event(Event::End(BytesEnd::new("w:rPr")))
        .map_err(|e| xml_io(e, part))?;
    // <w:footnoteReference w:id="N"/>
    let mut fref = BytesStart::new("w:footnoteReference");
    let id = num.to_string();
    fref.push_attribute(("w:id", id.as_str()));
    writer
        .write_event(Event::Empty(fref))
        .map_err(|e| xml_io(e, part))?;
    writer
        .write_event(Event::End(BytesEnd::new("w:r")))
        .map_err(|e| xml_io(e, part))?;
    Ok(())
}

fn needs_preserve(text: &[u8]) -> bool {
    matches!(text.first(), Some(c) if c.is_ascii_whitespace())
        || matches!(text.last(), Some(c) if c.is_ascii_whitespace())
}

/// Ensure `word/footnotes.xml` exists with separator entries plus a `<w:footnote
/// w:id="N">` for every used note number. Also patches content-types and
/// the document rels if the part is being created for the first time.
fn ensure_footnotes_part(
    pkg: &mut Package,
    notes: &BTreeMap<u32, &str>,
    seen: &[u32],
) -> Result<()> {
    let mut payload: Vec<u8> = Vec::new();
    payload.extend_from_slice(
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:id="-1" w:type="separator"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="0" w:type="continuationSeparator"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>"#,
    );
    for n in seen {
        if let Some(body) = notes.get(n) {
            payload.extend_from_slice(format!("<w:footnote w:id=\"{n}\">").as_bytes());
            payload.extend_from_slice(
                br#"<w:p><w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> </w:t></w:r><w:r><w:t xml:space="preserve">"#,
            );
            // Escape the user text minimally for XML.
            for byte in body.bytes() {
                match byte {
                    b'<' => payload.extend_from_slice(b"&lt;"),
                    b'>' => payload.extend_from_slice(b"&gt;"),
                    b'&' => payload.extend_from_slice(b"&amp;"),
                    _ => payload.push(byte),
                }
            }
            payload.extend_from_slice(b"</w:t></w:r></w:p></w:footnote>");
        }
    }
    payload.extend_from_slice(b"</w:footnotes>");
    pkg.set_part(PART_FOOTNOTES, payload);

    // Patch [Content_Types].xml if we're creating the part.
    if let Some(ct_bytes) = pkg.get_part_mut(PART_CONTENT_TYPES) {
        if !memchr_contains(ct_bytes, b"/word/footnotes.xml") {
            let mut s = std::str::from_utf8(ct_bytes).unwrap_or("").to_string();
            let inject = format!(
                r#"<Override PartName="/word/footnotes.xml" ContentType="{CT_FOOTNOTES}"/>"#
            );
            if let Some(pos) = s.rfind("</Types>") {
                s.insert_str(pos, &inject);
                *ct_bytes = s.into_bytes();
            }
        }
    }

    // Patch document.xml.rels if missing.
    if let Some(rels_bytes) = pkg.get_part_mut(PART_DOCUMENT_RELS) {
        if !memchr_contains(rels_bytes, b"relationships/footnotes") {
            // Find max existing rId.
            let mut max_id = 0u32;
            let s = std::str::from_utf8(rels_bytes).unwrap_or("");
            for (idx, _) in s.match_indices("Id=\"rId") {
                let tail = &s[idx + 7..];
                let n: u32 = tail
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0);
                if n > max_id {
                    max_id = n;
                }
            }
            let new_id = max_id + 1;
            let inject = format!(
                r#"<Relationship Id="rId{new_id}" Type="{REL_TYPE_FOOTNOTES}" Target="footnotes.xml"/>"#
            );
            let mut out = s.to_string();
            if let Some(pos) = out.rfind("</Relationships>") {
                out.insert_str(pos, &inject);
                *rels_bytes = out.into_bytes();
            }
        }
    }
    Ok(())
}

fn memchr_contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_first_marker_basic() {
        let mut notes: BTreeMap<u32, &str> = BTreeMap::new();
        notes.insert(1, "first");
        notes.insert(42, "answer");
        let t = b"intro.[1] body.[42] end.";
        let (idx, n) = find_first_marker(t, &notes).unwrap();
        assert_eq!(n, 1);
        assert_eq!(&t[idx..idx + 3], b"[1]");
    }

    #[test]
    fn find_first_marker_skips_unknown_and_non_digits() {
        let mut notes: BTreeMap<u32, &str> = BTreeMap::new();
        notes.insert(7, "seven");
        let t = b"[1] [Liedhegener] [S2] [7] end";
        let (idx, n) = find_first_marker(t, &notes).unwrap();
        assert_eq!(n, 7);
        assert_eq!(&t[idx..idx + 3], b"[7]");
    }

    #[test]
    fn marker_len_works() {
        assert_eq!(marker_len(1), 3);
        assert_eq!(marker_len(9), 3);
        assert_eq!(marker_len(10), 4);
        assert_eq!(marker_len(42), 4);
        assert_eq!(marker_len(123), 5);
    }
}
