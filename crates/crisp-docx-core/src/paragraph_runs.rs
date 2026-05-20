//! Run-level paragraph I/O.
//!
//! Sister module to [`crate::paragraph_text`], which collapses every
//! paragraph into a single run. That's the right shape when the
//! transformation is text-only (e.g. translate-then-write-back). But
//! when intra-paragraph formatting matters — bold spans, italic
//! emphasis, run-level rStyle — we need to surface every individual
//! `<w:r>` with its `<w:rPr>` so the caller can transform them
//! piecewise and we can stitch the result back together.
//!
//! The pipeline this enables:
//!
//! ```text
//! input.docx
//!   ↓ extract_paragraphs
//! Vec<ParagraphInfo>   each paragraph carries:
//!                      - pPr (paragraph properties, preserved verbatim)
//!                      - Vec<Run> with (text, rPr xml bytes, kind)
//!                      - footnote ref positions (run index + character offset)
//!                      - leading bookmark starts, trailing bookmark ends
//!   ↓ caller transforms text  (translate via LLM, align via crisp-docx-align,
//!                              redistribute runs over target text)
//!   ↓ replace_paragraphs
//! output.docx
//! ```
//!
//! `ParagraphInfo`'s representation is deliberately minimal — opaque
//! XML byte slices for things we don't introspect (pPr, rPr,
//! bookmarks). The alignment-driven format-transfer in
//! `crisp-docx-align` consumes these as-is.

use std::borrow::Cow;

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::{Error, Result};
use crate::ns::PART_DOCUMENT;
use crate::package::Package;

/// One run inside a paragraph. The `rpr_xml` bytes are the literal
/// contents of `<w:rPr>...</w:rPr>` (including the enclosing tags); if
/// the source run had no rPr, this is `None`.
#[derive(Debug, Clone, Default)]
pub struct Run {
    /// Concatenation of every `<w:t>` text node in this run, plus tabs
    /// (`\t`) and line breaks (`\n`) for `<w:tab/>` / `<w:br/>` empty
    /// children that appeared inside it. Empty if the run only carried
    /// non-text content (e.g. a lone footnoteReference — see
    /// [`Run::footnote_refs`]).
    pub text: String,
    /// Verbatim `<w:rPr>...</w:rPr>` bytes, if present. The caller can
    /// re-emit this unchanged to preserve every property Word knows
    /// about (font, colour, lang, rStyle, …).
    pub rpr_xml: Option<Vec<u8>>,
    /// `<w:footnoteReference w:id="N"/>` elements that lived inside
    /// this run, in document order. Captured separately so the caller
    /// can decide where to place them in the rebuilt paragraph; they
    /// are typically anchored to a specific word.
    pub footnote_refs: Vec<Vec<u8>>,
}

/// Everything we need to round-trip a body paragraph at run granularity.
#[derive(Debug, Clone, Default)]
pub struct ParagraphInfo {
    /// Verbatim `<w:pPr>...</w:pPr>` bytes, if present. Preserves the
    /// paragraph's style id, alignment, spacing, and so on.
    pub ppr_xml: Option<Vec<u8>>,
    /// The runs that make up the paragraph text, in document order.
    pub runs: Vec<Run>,
    /// `<w:bookmarkStart .../>` elements that appeared as direct
    /// children of the paragraph *before* the first run.
    pub leading_bookmark_starts: Vec<Vec<u8>>,
    /// `<w:bookmarkEnd .../>` elements that appeared as direct
    /// children of the paragraph *after* the last run.
    pub trailing_bookmark_ends: Vec<Vec<u8>>,
}

impl ParagraphInfo {
    /// The full paragraph text — concatenation of every run's text.
    pub fn full_text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }
}

/// Extract every body paragraph at run granularity.
pub fn extract_paragraphs(pkg: &Package) -> Result<Vec<ParagraphInfo>> {
    let doc = pkg
        .get_part(PART_DOCUMENT)
        .ok_or_else(|| Error::InvalidPackage {
            path: PART_DOCUMENT.into(),
            reason: format!("missing part {PART_DOCUMENT}"),
        })?;

    let mut reader = Reader::from_reader(doc);
    let mut buf = Vec::new();
    let mut out: Vec<ParagraphInfo> = Vec::new();
    let mut cur: Option<ParagraphInfo> = None;
    let mut p_depth = 0i32;

    // pPr capture: when inside <w:p> and we open <w:pPr>, accumulate
    // every byte through the matching close into a side buffer.
    let mut ppr_depth = 0i32;
    let mut ppr_buf: Vec<u8> = Vec::new();

    // Per-run state.
    let mut in_r = false;
    let mut r_depth = 0i32;
    let mut current_run: Run = Run::default();

    // rPr capture within a run, same shape as pPr.
    let mut rpr_depth = 0i32;
    let mut rpr_buf: Vec<u8> = Vec::new();

    // <w:t> text gating: only push char data into `current_run.text`
    // when we're inside a w:t and not inside rPr.
    let mut in_wt = false;

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
                p_depth = 1;
                cur = Some(ParagraphInfo::default());
            }
            Event::End(e) if e.name().as_ref() == b"w:p" && p_depth == 1 => {
                p_depth = 0;
                if let Some(info) = cur.take() {
                    out.push(info);
                }
            }
            // pPr (direct child of w:p)
            Event::Start(e) if p_depth == 1 && !in_r && e.name().as_ref() == b"w:pPr" => {
                ppr_depth = 1;
                ppr_buf.clear();
                let mut w = Writer::new(&mut ppr_buf);
                let _ = w.write_event(Event::Start(e.clone()));
            }
            Event::End(e) if p_depth == 1 && ppr_depth >= 1 && e.name().as_ref() == b"w:pPr" => {
                let mut w = Writer::new(&mut ppr_buf);
                let _ = w.write_event(Event::End(e.clone()));
                if let Some(info) = cur.as_mut() {
                    info.ppr_xml = Some(std::mem::take(&mut ppr_buf));
                }
                ppr_depth = 0;
            }
            _ if ppr_depth >= 1 => {
                // Forward every event inside pPr into the buffer.
                let mut w = Writer::new(&mut ppr_buf);
                if let Event::Start(_) = &evt {
                    ppr_depth += 1;
                } else if let Event::End(_) = &evt {
                    ppr_depth -= 1;
                }
                let _ = w.write_event(evt.clone());
                buf.clear();
                continue;
            }
            // Run open
            Event::Start(e) if p_depth == 1 && !in_r && e.name().as_ref() == b"w:r" => {
                in_r = true;
                r_depth = 1;
                current_run = Run::default();
            }
            Event::End(e)
                if p_depth == 1 && in_r && r_depth == 1 && e.name().as_ref() == b"w:r" =>
            {
                if let Some(info) = cur.as_mut() {
                    info.runs.push(std::mem::take(&mut current_run));
                }
                in_r = false;
                r_depth = 0;
            }
            // rPr (direct child of w:r)
            Event::Start(e) if in_r && r_depth == 1 && e.name().as_ref() == b"w:rPr" => {
                rpr_depth = 1;
                rpr_buf.clear();
                let mut w = Writer::new(&mut rpr_buf);
                let _ = w.write_event(Event::Start(e.clone()));
            }
            Event::End(e) if in_r && rpr_depth >= 1 && e.name().as_ref() == b"w:rPr" => {
                let mut w = Writer::new(&mut rpr_buf);
                let _ = w.write_event(Event::End(e.clone()));
                current_run.rpr_xml = Some(std::mem::take(&mut rpr_buf));
                rpr_depth = 0;
            }
            _ if rpr_depth >= 1 => {
                let mut w = Writer::new(&mut rpr_buf);
                if let Event::Start(_) = &evt {
                    rpr_depth += 1;
                } else if let Event::End(_) = &evt {
                    rpr_depth -= 1;
                }
                let _ = w.write_event(evt.clone());
                buf.clear();
                continue;
            }
            // <w:t> gating
            Event::Start(e) if in_r && e.name().as_ref() == b"w:t" => {
                in_wt = true;
            }
            Event::End(e) if in_r && e.name().as_ref() == b"w:t" => {
                in_wt = false;
            }
            Event::Text(t) if in_r && in_wt => {
                let decoded = t.unescape().unwrap_or(Cow::Borrowed(""));
                current_run.text.push_str(&decoded);
            }
            // tab / br inside a run
            Event::Empty(e) if in_r && e.name().as_ref() == b"w:tab" => {
                current_run.text.push('\t');
            }
            Event::Empty(e) if in_r && e.name().as_ref() == b"w:br" => {
                current_run.text.push('\n');
            }
            // Footnote ref inside a run
            Event::Empty(e) if in_r && e.name().as_ref() == b"w:footnoteReference" => {
                let mut tmp: Vec<u8> = Vec::new();
                let mut w = Writer::new(&mut tmp);
                let _ = w.write_event(Event::Empty(e.clone()));
                current_run.footnote_refs.push(tmp);
            }
            // Bookmarks at the paragraph level (direct children of w:p).
            Event::Empty(e) if p_depth == 1 && !in_r && e.name().as_ref() == b"w:bookmarkStart" => {
                let mut tmp: Vec<u8> = Vec::new();
                let mut w = Writer::new(&mut tmp);
                let _ = w.write_event(Event::Empty(e.clone()));
                if let Some(info) = cur.as_mut() {
                    if info.runs.is_empty() {
                        info.leading_bookmark_starts.push(tmp);
                    } else {
                        // Mid-paragraph bookmark — uncommon. For now we
                        // append to trailing to keep the invariant
                        // "bookmarks frame the paragraph".
                        info.trailing_bookmark_ends.push(tmp);
                    }
                }
            }
            Event::Empty(e) if p_depth == 1 && !in_r && e.name().as_ref() == b"w:bookmarkEnd" => {
                let mut tmp: Vec<u8> = Vec::new();
                let mut w = Writer::new(&mut tmp);
                let _ = w.write_event(Event::Empty(e.clone()));
                if let Some(info) = cur.as_mut() {
                    info.trailing_bookmark_ends.push(tmp);
                }
            }
            Event::Start(_) if in_r => {
                r_depth += 1;
            }
            Event::End(_) if in_r => {
                r_depth -= 1;
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
}

/// Rebuild every body paragraph from the structured `paragraphs` slice.
///
/// Paragraphs not covered by the slice (i.e. `index >= paragraphs.len()`)
/// are passed through unchanged. Other paragraphs are replaced byte-for-byte
/// with the new structure: leading bookmark starts, pPr, runs, trailing
/// bookmark ends.
///
/// Each run emits as `<w:r>{rpr_xml?}<w:t xml:space="preserve">{text}</w:t>
/// {footnote_refs*}</w:r>`. Footnote refs are placed at the end of the run.
pub fn replace_paragraphs(pkg: &mut Package, paragraphs: &[ParagraphInfo]) -> Result<()> {
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
    let mut rewriting = false;
    let mut p_depth = 0i32;

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
                if p_index < paragraphs.len() {
                    rewriting = true;
                    writer
                        .write_event(Event::Start(e.clone()))
                        .map_err(xml_err)?;
                    emit_paragraph(&mut writer, &paragraphs[p_index])?;
                } else {
                    writer
                        .write_event(Event::Start(e.clone()))
                        .map_err(xml_err)?;
                }
                p_depth = 1;
            }
            Event::End(e) if e.name().as_ref() == b"w:p" && p_depth == 1 => {
                writer.write_event(Event::End(e.clone())).map_err(xml_err)?;
                rewriting = false;
                p_depth = 0;
                p_index += 1;
            }
            _ if rewriting => {
                // swallow everything between Start(w:p) and End(w:p)
            }
            other => {
                writer.write_event(other.clone()).map_err(xml_err)?;
            }
        }
        buf.clear();
    }

    drop(writer);
    pkg.set_part(PART_DOCUMENT, out);
    Ok(())
}

fn emit_paragraph<W: std::io::Write>(writer: &mut Writer<W>, info: &ParagraphInfo) -> Result<()> {
    if let Some(pp) = &info.ppr_xml {
        writer.get_mut().write_all(pp).map_err(|e| Error::Io {
            path: PART_DOCUMENT.into(),
            source: e,
        })?;
    }
    for bs in &info.leading_bookmark_starts {
        writer.get_mut().write_all(bs).map_err(|e| Error::Io {
            path: PART_DOCUMENT.into(),
            source: e,
        })?;
    }
    for run in &info.runs {
        writer
            .write_event(Event::Start(BytesStart::new("w:r")))
            .map_err(xml_err)?;
        if let Some(rpr) = &run.rpr_xml {
            writer.get_mut().write_all(rpr).map_err(|e| Error::Io {
                path: PART_DOCUMENT.into(),
                source: e,
            })?;
        }
        if !run.text.is_empty() {
            let mut t_start = BytesStart::new("w:t");
            t_start.push_attribute(("xml:space", "preserve"));
            writer.write_event(Event::Start(t_start)).map_err(xml_err)?;
            writer
                .write_event(Event::Text(BytesText::new(&run.text)))
                .map_err(xml_err)?;
            writer
                .write_event(Event::End(BytesEnd::new("w:t")))
                .map_err(xml_err)?;
        }
        for fr in &run.footnote_refs {
            writer.get_mut().write_all(fr).map_err(|e| Error::Io {
                path: PART_DOCUMENT.into(),
                source: e,
            })?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("w:r")))
            .map_err(xml_err)?;
    }
    for be in &info.trailing_bookmark_ends {
        writer.get_mut().write_all(be).map_err(|e| Error::Io {
            path: PART_DOCUMENT.into(),
            source: e,
        })?;
    }
    Ok(())
}

fn xml_err(e: quick_xml::Error) -> Error {
    Error::XmlParse {
        part: PART_DOCUMENT.into(),
        source: e,
    }
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
        zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap();
        zw.start_file("word/document.xml", opts).unwrap();
        let doc = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{body}</w:document>"#
        );
        zw.write_all(doc.as_bytes()).unwrap();
        zw.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_single_run_paragraph() {
        let body = r#"<w:body><w:p><w:r><w:t>Hello.</w:t></w:r></w:p></w:body>"#;
        let pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let ps = extract_paragraphs(&pkg).unwrap();
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].runs.len(), 1);
        assert_eq!(ps[0].runs[0].text, "Hello.");
        assert!(ps[0].runs[0].rpr_xml.is_none());
    }

    #[test]
    fn extracts_run_with_rpr() {
        let body =
            r#"<w:body><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Bold.</w:t></w:r></w:p></w:body>"#;
        let pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let ps = extract_paragraphs(&pkg).unwrap();
        assert_eq!(ps[0].runs[0].text, "Bold.");
        let rpr = ps[0].runs[0].rpr_xml.as_ref().unwrap();
        let s = std::str::from_utf8(rpr).unwrap();
        assert!(s.contains("w:b"), "rPr missing bold: {s}");
        assert!(s.starts_with("<w:rPr"), "rPr missing tags: {s}");
    }

    #[test]
    fn extracts_mixed_runs_in_order() {
        let body = r#"<w:body><w:p>
          <w:r><w:t xml:space="preserve">Plain </w:t></w:r>
          <w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r>
          <w:r><w:t xml:space="preserve"> tail.</w:t></w:r>
        </w:p></w:body>"#;
        let pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let ps = extract_paragraphs(&pkg).unwrap();
        assert_eq!(ps[0].runs.len(), 3);
        assert_eq!(ps[0].runs[0].text, "Plain ");
        assert_eq!(ps[0].runs[1].text, "bold");
        assert_eq!(ps[0].runs[2].text, " tail.");
        assert!(ps[0].runs[0].rpr_xml.is_none());
        assert!(ps[0].runs[1].rpr_xml.is_some());
        assert!(ps[0].runs[2].rpr_xml.is_none());
        assert_eq!(ps[0].full_text(), "Plain bold tail.");
    }

    #[test]
    fn extracts_ppr_and_footnote_ref() {
        let body = r#"<w:body><w:p>
          <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
          <w:r><w:t>Cite</w:t><w:footnoteReference w:id="1"/></w:r>
        </w:p></w:body>"#;
        let pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let ps = extract_paragraphs(&pkg).unwrap();
        let ppr = ps[0].ppr_xml.as_ref().unwrap();
        let s = std::str::from_utf8(ppr).unwrap();
        assert!(s.contains("Heading1"), "lost pStyle: {s}");
        assert_eq!(ps[0].runs[0].text, "Cite");
        assert_eq!(ps[0].runs[0].footnote_refs.len(), 1);
    }

    #[test]
    fn round_trips_complex_paragraph() {
        let body = r#"<w:body><w:p>
          <w:pPr><w:pStyle w:val="Body"/></w:pPr>
          <w:r><w:t xml:space="preserve">Plain </w:t></w:r>
          <w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r>
          <w:r><w:t xml:space="preserve"> tail</w:t><w:footnoteReference w:id="3"/></w:r>
        </w:p></w:body>"#;
        let mut pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let ps = extract_paragraphs(&pkg).unwrap();
        replace_paragraphs(&mut pkg, &ps).unwrap();
        let ps2 = extract_paragraphs(&pkg).unwrap();
        assert_eq!(ps.len(), ps2.len());
        assert_eq!(ps[0].full_text(), ps2[0].full_text());
        assert_eq!(ps[0].runs.len(), ps2[0].runs.len());
        assert_eq!(ps2[0].runs[1].text, "bold");
        assert!(ps2[0].runs[1].rpr_xml.is_some());
        // footnote ref survives
        assert_eq!(ps2[0].runs[2].footnote_refs.len(), 1);
    }

    #[test]
    fn replace_swaps_runs() {
        let body = r#"<w:body><w:p><w:r><w:t>Hello.</w:t></w:r></w:p></w:body>"#;
        let mut pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let mut info = ParagraphInfo::default();
        info.runs.push(Run {
            text: "Hallo".into(),
            rpr_xml: None,
            footnote_refs: vec![],
        });
        info.runs.push(Run {
            text: " Welt.".into(),
            rpr_xml: Some(b"<w:rPr><w:i/></w:rPr>".to_vec()),
            footnote_refs: vec![],
        });
        replace_paragraphs(&mut pkg, &[info]).unwrap();
        let ps2 = extract_paragraphs(&pkg).unwrap();
        assert_eq!(ps2[0].full_text(), "Hallo Welt.");
        assert_eq!(ps2[0].runs.len(), 2);
        assert!(ps2[0].runs[0].rpr_xml.is_none());
        assert!(ps2[0].runs[1].rpr_xml.is_some());
    }

    #[test]
    fn multiple_paragraphs_round_trip() {
        let body = r#"<w:body>
          <w:p><w:r><w:t>One.</w:t></w:r></w:p>
          <w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Two.</w:t></w:r></w:p>
          <w:p><w:r><w:t>Three.</w:t></w:r></w:p>
        </w:body>"#;
        let mut pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let ps = extract_paragraphs(&pkg).unwrap();
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[0].full_text(), "One.");
        assert_eq!(ps[1].full_text(), "Two.");
        assert_eq!(ps[2].full_text(), "Three.");
        assert!(ps[1].runs[0].rpr_xml.is_some());
        replace_paragraphs(&mut pkg, &ps).unwrap();
        let ps2 = extract_paragraphs(&pkg).unwrap();
        assert_eq!(ps2.len(), 3);
        for (a, b) in ps.iter().zip(ps2.iter()) {
            assert_eq!(a.full_text(), b.full_text());
            assert_eq!(a.runs.len(), b.runs.len());
        }
    }

    #[test]
    fn replace_fewer_paragraphs_passes_through_extras() {
        let body = r#"<w:body>
          <w:p><w:r><w:t>One.</w:t></w:r></w:p>
          <w:p><w:r><w:t>Two.</w:t></w:r></w:p>
        </w:body>"#;
        let mut pkg = Package::from_bytes(&make_doc(body)).unwrap();
        let mut info = ParagraphInfo::default();
        info.runs.push(Run {
            text: "Eins.".into(),
            ..Default::default()
        });
        replace_paragraphs(&mut pkg, &[info]).unwrap();
        let ps = extract_paragraphs(&pkg).unwrap();
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0].full_text(), "Eins.");
        assert_eq!(ps[1].full_text(), "Two.");
    }
}
