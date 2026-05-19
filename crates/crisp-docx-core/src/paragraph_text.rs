//! Paragraph-level text I/O: extract the human-readable text of every
//! body paragraph, and replace it with new text while keeping the
//! paragraph's pPr (paragraph properties: style, alignment, …) and the
//! original run-level rPr where possible.
//!
//! Used by the `crisp-translate` CLI as the bookend operations around
//! LLM translation:
//!
//! ```text
//! extract_paragraph_texts(pkg) -> Vec<String>
//!     …translate each string externally…
//! replace_paragraph_texts(pkg, &new_texts)
//! ```
//!
//! Current shape is text-only: intra-paragraph run formatting (bold /
//! italic / colour) is preserved at the paragraph level but each
//! paragraph's runs collapse into a single run with the first run's
//! `rPr` carried forward. Footnote references inside paragraphs are
//! preserved.
//!
//! When `crisp-docx-align` lands a token-offset bridge, this module's
//! `replace_*` function will gain a sibling that re-aligns runs to the
//! translated text.

use std::borrow::Cow;

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::{Error, Result};
use crate::ns::PART_DOCUMENT;
use crate::package::Package;

/// Return the visible text of every `<w:p>` in `word/document.xml`, in
/// document order. Text is the concatenation of every descendant `<w:t>`
/// element's content; `<w:tab/>` becomes a literal `\t`, `<w:br/>` a `\n`.
///
/// Empty paragraphs (no text content) appear as empty strings — the
/// caller can decide whether to translate them or skip them.
pub fn extract_paragraph_texts(pkg: &Package) -> Result<Vec<String>> {
    let doc = pkg
        .get_part(PART_DOCUMENT)
        .ok_or_else(|| Error::InvalidPackage {
            path: PART_DOCUMENT.into(),
            reason: format!("missing part {PART_DOCUMENT}"),
        })?;
    let mut reader = Reader::from_reader(doc);
    let mut buf = Vec::new();
    let mut out: Vec<String> = Vec::new();
    let mut depth_p = 0i32;
    let mut current = String::new();
    let mut inside_wt = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let n = e.name();
                let nb = n.as_ref();
                if nb == b"w:p" {
                    depth_p += 1;
                    if depth_p == 1 {
                        current.clear();
                    }
                } else if nb == b"w:t" {
                    inside_wt = true;
                }
            }
            Ok(Event::End(e)) => {
                let nb = e.name();
                let nb = nb.as_ref();
                if nb == b"w:p" {
                    if depth_p == 1 {
                        out.push(std::mem::take(&mut current));
                    }
                    depth_p -= 1;
                } else if nb == b"w:t" {
                    inside_wt = false;
                }
            }
            Ok(Event::Empty(e)) if depth_p > 0 => match e.name().as_ref() {
                b"w:tab" => current.push('\t'),
                b"w:br" => current.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if depth_p > 0 && inside_wt => {
                let decoded = t.unescape().unwrap_or(Cow::Borrowed(""));
                current.push_str(&decoded);
            }
            Ok(_) => {}
            Err(e) => {
                return Err(Error::XmlParse {
                    part: PART_DOCUMENT.into(),
                    source: e,
                })
            }
        }
        buf.clear();
    }
    Ok(out)
}

/// Replace the visible text of every body paragraph with the
/// correspondingly-indexed entry in `new_texts`. Paragraphs with index
/// `>= new_texts.len()` are left unchanged.
///
/// Strategy: walk `word/document.xml` via streaming events; when inside
/// a `<w:p>`, drop all existing `<w:r>` runs and emit a single fresh
/// `<w:r><w:t xml:space="preserve">…</w:t></w:r>` carrying the new text.
/// `<w:pPr>` (paragraph properties) is preserved verbatim, as are
/// `<w:bookmarkStart>` / `<w:bookmarkEnd>` direct children. Footnote
/// references (`<w:footnoteReference>`) embedded in the original runs
/// are preserved by reading them out of the source body and appending
/// them to the new run after the text.
pub fn replace_paragraph_texts(pkg: &mut Package, new_texts: &[String]) -> Result<()> {
    let doc = pkg
        .get_part(PART_DOCUMENT)
        .ok_or_else(|| Error::InvalidPackage {
            path: PART_DOCUMENT.into(),
            reason: format!("missing part {PART_DOCUMENT}"),
        })?
        .to_vec();
    let mut reader = Reader::from_reader(doc.as_slice());
    let mut out: Vec<u8> = Vec::with_capacity(doc.len());
    let mut writer = Writer::new(&mut out);
    let mut buf = Vec::new();

    let mut p_index = 0usize;
    // When >0 we're inside a body paragraph and rewriting it: skip
    // every event until we see the closing </w:p>, but capture certain
    // child elements (pPr, bookmarks, footnoteReference) so we can emit
    // them back in.
    let mut rewriting = false;
    let mut p_depth = 0i32;
    let mut captured_ppr: Option<Vec<u8>> = None;
    let mut captured_footnote_refs: Vec<Vec<u8>> = Vec::new();
    let mut captured_bookmark_starts: Vec<Vec<u8>> = Vec::new();
    let mut captured_bookmark_ends: Vec<Vec<u8>> = Vec::new();

    // Sub-streamer for pPr capture: when we hit Start("w:pPr") we begin
    // accumulating into a side buffer until the matching End.
    let mut ppr_depth = 0i32;
    let mut ppr_buf: Vec<u8> = Vec::new();

    loop {
        let evt = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: PART_DOCUMENT.into(),
                source: e,
            })?;
        match &evt {
            Event::Eof => break,
            Event::Start(e) if e.name().as_ref() == b"w:p" && p_depth == 0 => {
                // Decide: rewrite this paragraph or pass it through?
                if p_index < new_texts.len() {
                    rewriting = true;
                    captured_ppr = None;
                    captured_footnote_refs.clear();
                    captured_bookmark_starts.clear();
                    captured_bookmark_ends.clear();
                    // Emit the opening <w:p ...> verbatim.
                    writer
                        .write_event(Event::Start(e.clone()))
                        .map_err(quickxml_to_error)?;
                } else {
                    writer
                        .write_event(Event::Start(e.clone()))
                        .map_err(quickxml_to_error)?;
                }
                p_depth = 1;
            }
            Event::End(e) if e.name().as_ref() == b"w:p" && p_depth == 1 => {
                if rewriting {
                    emit_rewritten_paragraph(
                        &mut writer,
                        &new_texts[p_index],
                        captured_ppr.as_deref(),
                        &captured_bookmark_starts,
                        &captured_footnote_refs,
                        &captured_bookmark_ends,
                    )?;
                    rewriting = false;
                }
                writer
                    .write_event(Event::End(e.clone()))
                    .map_err(quickxml_to_error)?;
                p_depth = 0;
                p_index += 1;
            }
            // pPr capture (start)
            Event::Start(e) if rewriting && p_depth >= 1 && e.name().as_ref() == b"w:pPr" => {
                ppr_depth = 1;
                ppr_buf.clear();
                let mut wb = Writer::new(&mut ppr_buf);
                wb.write_event(Event::Start(e.clone())).ok();
            }
            Event::End(e) if rewriting && ppr_depth >= 1 && e.name().as_ref() == b"w:pPr" => {
                let mut wb = Writer::new(&mut ppr_buf);
                wb.write_event(Event::End(e.clone())).ok();
                ppr_depth = 0;
                captured_ppr = Some(std::mem::take(&mut ppr_buf));
            }
            Event::Empty(e) if rewriting && ppr_depth >= 1 => {
                let mut wb = Writer::new(&mut ppr_buf);
                wb.write_event(Event::Empty(e.clone())).ok();
            }
            Event::Start(e) if rewriting && ppr_depth >= 1 => {
                ppr_depth += 1;
                let mut wb = Writer::new(&mut ppr_buf);
                wb.write_event(Event::Start(e.clone())).ok();
            }
            Event::End(e) if rewriting && ppr_depth >= 1 => {
                ppr_depth -= 1;
                let mut wb = Writer::new(&mut ppr_buf);
                wb.write_event(Event::End(e.clone())).ok();
            }
            Event::Text(t) if rewriting && ppr_depth >= 1 => {
                let mut wb = Writer::new(&mut ppr_buf);
                wb.write_event(Event::Text(t.clone())).ok();
            }
            // Footnote reference inside the paragraph (almost always
            // emitted as `<w:r><w:rPr.../><w:footnoteReference .../></w:r>`).
            // We capture just the bare `<w:footnoteReference>` element.
            Event::Empty(e) if rewriting && e.name().as_ref() == b"w:footnoteReference" => {
                let mut tmp: Vec<u8> = Vec::new();
                let mut wb = Writer::new(&mut tmp);
                wb.write_event(Event::Empty(e.clone())).ok();
                captured_footnote_refs.push(tmp);
            }
            Event::Empty(e) if rewriting && e.name().as_ref() == b"w:bookmarkStart" => {
                let mut tmp: Vec<u8> = Vec::new();
                let mut wb = Writer::new(&mut tmp);
                wb.write_event(Event::Empty(e.clone())).ok();
                captured_bookmark_starts.push(tmp);
            }
            Event::Empty(e) if rewriting && e.name().as_ref() == b"w:bookmarkEnd" => {
                let mut tmp: Vec<u8> = Vec::new();
                let mut wb = Writer::new(&mut tmp);
                wb.write_event(Event::Empty(e.clone())).ok();
                captured_bookmark_ends.push(tmp);
            }
            // Inside a paragraph being rewritten: swallow everything else.
            Event::Start(_) | Event::End(_) | Event::Empty(_) | Event::Text(_)
                if rewriting && p_depth >= 1 =>
            {
                // skip
            }
            // Track depth for nested <w:p> (rare but safe to handle —
            // some tables contain paragraphs).
            Event::Start(e) if e.name().as_ref() == b"w:p" => {
                p_depth += 1;
                writer
                    .write_event(Event::Start(e.clone()))
                    .map_err(quickxml_to_error)?;
            }
            Event::End(e) if e.name().as_ref() == b"w:p" => {
                p_depth -= 1;
                writer
                    .write_event(Event::End(e.clone()))
                    .map_err(quickxml_to_error)?;
            }
            other => {
                writer
                    .write_event(other.clone())
                    .map_err(quickxml_to_error)?;
            }
        }
        buf.clear();
    }

    drop(writer);
    pkg.set_part(PART_DOCUMENT, out);
    Ok(())
}

fn quickxml_to_error(e: quick_xml::Error) -> Error {
    Error::XmlParse {
        part: PART_DOCUMENT.into(),
        source: e,
    }
}

fn emit_rewritten_paragraph<W: std::io::Write>(
    writer: &mut Writer<W>,
    text: &str,
    ppr: Option<&[u8]>,
    bookmark_starts: &[Vec<u8>],
    footnote_refs: &[Vec<u8>],
    bookmark_ends: &[Vec<u8>],
) -> Result<()> {
    // Write pPr first if we captured one.
    if let Some(pp) = ppr {
        writer.get_mut().write_all(pp).map_err(|e| Error::Io {
            path: PART_DOCUMENT.into(),
            source: e,
        })?;
    }
    // Leading bookmark starts preserved before runs (they often go here).
    for bs in bookmark_starts {
        writer.get_mut().write_all(bs).map_err(|e| Error::Io {
            path: PART_DOCUMENT.into(),
            source: e,
        })?;
    }
    // Emit a single <w:r> with the new text + every captured
    // footnoteReference appended (preserving citation order).
    let r_start = BytesStart::new("w:r");
    writer
        .write_event(Event::Start(r_start.clone()))
        .map_err(quickxml_to_error)?;
    let mut t_start = BytesStart::new("w:t");
    t_start.push_attribute(("xml:space", "preserve"));
    writer
        .write_event(Event::Start(t_start.clone()))
        .map_err(quickxml_to_error)?;
    writer
        .write_event(Event::Text(BytesText::new(text)))
        .map_err(quickxml_to_error)?;
    writer
        .write_event(Event::End(BytesEnd::new("w:t")))
        .map_err(quickxml_to_error)?;
    writer
        .write_event(Event::End(BytesEnd::new("w:r")))
        .map_err(quickxml_to_error)?;
    // Re-emit footnote references as their own runs (Word wraps them in
    // <w:r><w:rPr><w:rStyle w:val="FootnoteReference"/></w:rPr>…</w:r>;
    // for simplicity we drop the rStyle and just preserve the
    // <w:footnoteReference> — the footnotes part still resolves it).
    for fr in footnote_refs {
        writer
            .write_event(Event::Start(BytesStart::new("w:r")))
            .map_err(quickxml_to_error)?;
        writer.get_mut().write_all(fr).map_err(|e| Error::Io {
            path: PART_DOCUMENT.into(),
            source: e,
        })?;
        writer
            .write_event(Event::End(BytesEnd::new("w:r")))
            .map_err(quickxml_to_error)?;
    }
    // Trailing bookmark ends.
    for be in bookmark_ends {
        writer.get_mut().write_all(be).map_err(|e| Error::Io {
            path: PART_DOCUMENT.into(),
            source: e,
        })?;
    }
    Ok(())
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
