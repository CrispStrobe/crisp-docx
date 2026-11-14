//! Paragraph run extraction and bounded replacement.
//!
//! Unchanged paragraphs retain their original XML byte-for-byte. A changed
//! paragraph is rebuilt only when its content is representable by this flat
//! run model. Rich wrappers, nested stories and positioned anchors otherwise
//! produce an error before any package mutation.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use quick_xml::Writer;

use crate::error::{Error, Result};
use crate::ns::{PART_DOCUMENT, W};
use crate::package::Package;

/// One run's visible text, formatting and note references.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    /// Visible text; tabs and line breaks use `\t`, `\n`, and `\r`.
    pub text: String,
    /// Complete original run properties, including historical properties.
    pub rpr_xml: Option<Vec<u8>>,
    /// Raw footnote or endnote reference elements. The existing field name
    /// is retained for source compatibility. References emit after the text.
    pub footnote_refs: Vec<Vec<u8>>,
}

/// An outer body paragraph, including table-cell paragraphs. Nested textbox
/// text is exposed in its containing run; changing that rich paragraph is
/// rejected because this flat representation cannot preserve its topology.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParagraphInfo {
    /// Paragraph properties, preserved as complete original XML.
    pub ppr_xml: Option<Vec<u8>>,
    /// Runs in source order.
    pub runs: Vec<Run>,
    /// Leading bookmark starts.
    pub leading_bookmark_starts: Vec<Vec<u8>>,
    /// Trailing bookmark elements. Mid-paragraph positions are retained only
    /// by an unchanged paragraph, whose original XML is passed through.
    pub trailing_bookmark_ends: Vec<Vec<u8>>,
}

impl ParagraphInfo {
    /// Concatenated visible run text.
    pub fn full_text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }
}

struct ParsedParagraph {
    info: ParagraphInfo,
    start: usize,
    open_end: usize,
    close_start: usize,
    end: usize,
    opening: BytesStart<'static>,
    empty: bool,
    depth: usize,
    unsupported: Option<String>,
    saw_bookmark_end: bool,
}

impl ParsedParagraph {
    fn unsupported(&mut self, detail: impl Into<String>) {
        if self.unsupported.is_none() {
            self.unsupported = Some(detail.into());
        }
    }
}

#[derive(Clone, Copy)]
enum CaptureKind {
    ParagraphProperties,
    RunProperties,
    Reference,
    BookmarkStart,
    BookmarkEnd,
    Ignore,
}

struct Capture {
    depth: usize,
    start: usize,
    kind: CaptureKind,
}

fn invalid(reason: impl Into<String>) -> Error {
    Error::InvalidPackage {
        path: PART_DOCUMENT.into(),
        reason: reason.into(),
    }
}

fn xml_err(source: quick_xml::Error) -> Error {
    Error::XmlParse {
        part: PART_DOCUMENT.into(),
        source,
    }
}

fn word_local(reader: &NsReader<&[u8]>, name: quick_xml::name::QName<'_>) -> Result<Vec<u8>> {
    let (ns, local) = reader.resolve_element(name);
    match ns {
        ResolveResult::Bound(ns)
            if ns.as_ref() == W
                || ns.as_ref() == b"http://purl.oclc.org/ooxml/wordprocessingml/main" =>
        {
            Ok(local.as_ref().to_vec())
        }
        ResolveResult::Unknown(prefix) => Err(xml_err(quick_xml::Error::UnknownPrefix(prefix))),
        _ => Ok(Vec::new()),
    }
}

fn record_capture(
    p: &mut ParsedParagraph,
    run: &mut Option<(usize, Run)>,
    kind: CaptureKind,
    xml: &[u8],
) {
    match kind {
        CaptureKind::ParagraphProperties => p.info.ppr_xml = Some(xml.to_vec()),
        CaptureKind::RunProperties => {
            if let Some((_, r)) = run {
                r.rpr_xml = Some(xml.to_vec());
            }
        }
        CaptureKind::Reference => {
            if let Some((_, r)) = run {
                r.footnote_refs.push(xml.to_vec());
            }
        }
        CaptureKind::BookmarkStart => {
            if p.info.runs.is_empty() {
                p.info.leading_bookmark_starts.push(xml.to_vec());
            } else {
                p.info.trailing_bookmark_ends.push(xml.to_vec());
            }
        }
        CaptureKind::BookmarkEnd => p.info.trailing_bookmark_ends.push(xml.to_vec()),
        CaptureKind::Ignore => {}
    }
}

fn inspect_shape(
    p: &mut ParsedParagraph,
    run: &Option<(usize, Run)>,
    e: &BytesStart<'_>,
    local: &[u8],
    depth: usize,
    reader: &NsReader<&[u8]>,
) -> Result<()> {
    let parent_is_p = depth == p.depth + 1;
    let parent_is_r = run.as_ref().is_some_and(|(d, _)| depth == d + 1);
    let represented = match local {
        b"r" if parent_is_p => {
            if p.saw_bookmark_end {
                p.unsupported("a bookmark ends before later runs");
            }
            true
        }
        b"pPr" if parent_is_p => true,
        b"rPr" | b"t" | b"tab" | b"br" | b"cr" | b"footnoteReference" | b"endnoteReference"
            if parent_is_r =>
        {
            true
        }
        b"bookmarkStart" if parent_is_p => {
            if !p.info.runs.is_empty() {
                p.unsupported("a bookmark starts between runs");
            }
            true
        }
        b"bookmarkEnd" if parent_is_p => {
            p.saw_bookmark_end = true;
            true
        }
        _ => false,
    };
    if !represented {
        p.unsupported(format!(
            "element <{}>",
            String::from_utf8_lossy(e.name().as_ref())
        ));
    }
    if local == b"br" {
        for a in e.attributes() {
            let a = a.map_err(|e| xml_err(e.into()))?;
            let (_, local) = reader.resolve_attribute(a.key);
            let value = a.unescape_value().map_err(xml_err)?;
            if (local.as_ref() == b"type" && value != "textWrapping")
                || (local.as_ref() == b"clear" && value != "none")
            {
                p.unsupported("a page/column or clearing break");
            }
        }
    }
    Ok(())
}

fn parse_paragraphs(doc: &[u8]) -> Result<Vec<ParsedParagraph>> {
    let mut reader = NsReader::from_reader(doc);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    let mut current: Option<ParsedParagraph> = None;
    let mut run: Option<(usize, Run)> = None;
    let mut text_depth = None;
    let mut capture: Option<Capture> = None;
    let mut depth = 0usize;
    let mut roots = 0usize;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader.read_event_into(&mut buf).map_err(xml_err)?;
        let end = reader.buffer_position() as usize;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let local = word_local(&reader, e.name())?;
                for a in e.attributes() {
                    a.map_err(|e| xml_err(e.into()))?
                        .unescape_value()
                        .map_err(xml_err)?;
                }
                if depth == 0 {
                    roots += 1;
                }
                let element_depth = depth + 1;
                if current.is_none() && local == b"p" {
                    let p = ParsedParagraph {
                        info: ParagraphInfo::default(),
                        start,
                        open_end: end,
                        close_start: end,
                        end,
                        opening: e.clone().into_owned(),
                        empty,
                        depth: element_depth,
                        unsupported: None,
                        saw_bookmark_end: false,
                    };
                    if empty {
                        out.push(p);
                    } else {
                        current = Some(p);
                    }
                } else if let Some(p) = current.as_mut() {
                    if capture.is_none() {
                        inspect_shape(p, &run, &e, &local, element_depth, &reader)?;
                        let kind = match local.as_slice() {
                            b"pPr" => Some(if element_depth == p.depth + 1 {
                                CaptureKind::ParagraphProperties
                            } else {
                                CaptureKind::Ignore
                            }),
                            b"rPr" => Some(
                                if run.as_ref().is_some_and(|(d, _)| element_depth == d + 1) {
                                    CaptureKind::RunProperties
                                } else {
                                    CaptureKind::Ignore
                                },
                            ),
                            b"footnoteReference" | b"endnoteReference" if run.is_some() => {
                                Some(CaptureKind::Reference)
                            }
                            b"bookmarkStart" if element_depth == p.depth + 1 => {
                                Some(CaptureKind::BookmarkStart)
                            }
                            b"bookmarkEnd" if element_depth == p.depth + 1 => {
                                Some(CaptureKind::BookmarkEnd)
                            }
                            _ => None,
                        };
                        if let Some(kind) = kind {
                            if empty {
                                record_capture(p, &mut run, kind, &doc[start..end]);
                            } else {
                                capture = Some(Capture {
                                    depth: element_depth,
                                    start,
                                    kind,
                                });
                            }
                        } else if local == b"r" && run.is_none() {
                            if empty {
                                p.info.runs.push(Run::default());
                            } else {
                                run = Some((element_depth, Run::default()));
                            }
                        } else if local == b"t" && run.is_some() && !empty {
                            text_depth = Some(element_depth);
                        } else if let Some((_, r)) = run.as_mut() {
                            match local.as_slice() {
                                b"tab" => r.text.push('\t'),
                                b"br" => r.text.push('\n'),
                                b"cr" => r.text.push('\r'),
                                _ => {}
                            }
                        }
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(e) => {
                let local = word_local(&reader, e.name())?;
                if let Some(c) = &capture {
                    if c.depth == depth {
                        if let Some(p) = current.as_mut() {
                            record_capture(p, &mut run, c.kind, &doc[c.start..end]);
                        }
                        capture = None;
                    }
                } else if let Some(p) = current.as_mut() {
                    if text_depth == Some(depth) && local == b"t" {
                        text_depth = None;
                    }
                    if local == b"r" && run.as_ref().is_some_and(|(d, _)| *d == depth) {
                        p.info.runs.push(run.take().unwrap().1);
                    }
                    if local == b"p" && depth == p.depth {
                        p.close_start = start;
                        p.end = end;
                        out.push(current.take().unwrap());
                    }
                }
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid("unbalanced XML closing element"))?;
            }
            Event::Text(e) => {
                let text = e.unescape().map_err(xml_err)?;
                if depth == 0 && !text.trim().is_empty() {
                    return Err(invalid("text outside XML document element"));
                }
                if capture.is_none() && text_depth.is_some() {
                    if let Some((_, run)) = run.as_mut() {
                        run.text.push_str(&text);
                    }
                }
            }
            Event::CData(e) if capture.is_none() && text_depth.is_some() => {
                let text =
                    std::str::from_utf8(e.as_ref()).map_err(|_| invalid("invalid UTF-8 text"))?;
                if let Some((_, run)) = run.as_mut() {
                    run.text.push_str(text);
                }
            }
            _ => {}
        }
        buf.clear();
    }
    if roots != 1 || depth != 0 || current.is_some() {
        return Err(invalid("expected one complete XML document element"));
    }
    Ok(out)
}

/// Extract outer paragraphs in document order, including empty paragraphs
/// and paragraphs in table cells. Nested textbox paragraphs remain within
/// their outer run and do not consume another replacement index.
pub fn extract_paragraphs(pkg: &Package) -> Result<Vec<ParagraphInfo>> {
    let doc = pkg
        .get_part(PART_DOCUMENT)
        .ok_or_else(|| invalid("missing word/document.xml"))?;
    Ok(parse_paragraphs(doc)?.into_iter().map(|p| p.info).collect())
}

/// Replace supported paragraphs at matching indices. Unchanged paragraphs
/// and indices beyond `paragraphs.len()` retain exact original XML. Changed
/// rich paragraphs fail atomically instead of losing unrepresented content.
pub fn replace_paragraphs(pkg: &mut Package, paragraphs: &[ParagraphInfo]) -> Result<()> {
    let doc = pkg
        .get_part(PART_DOCUMENT)
        .ok_or_else(|| invalid("missing word/document.xml"))?;
    let originals = parse_paragraphs(doc)?;
    let mut out = Vec::with_capacity(doc.len());
    let mut cursor = 0usize;
    let mut changed = false;
    for (index, (old, new)) in originals.iter().zip(paragraphs).enumerate() {
        if old.info == *new {
            continue;
        }
        if let Some(reason) = &old.unsupported {
            return Err(invalid(format!(
                "paragraph {index} replacement cannot preserve {reason}"
            )));
        }
        changed = true;
        out.extend_from_slice(&doc[cursor..old.start]);
        let name = std::str::from_utf8(old.opening.name().as_ref())
            .map_err(|_| invalid("invalid paragraph name"))?
            .to_string();
        let prefix = name.strip_suffix('p').unwrap_or("w:");
        let mut writer = Writer::new(&mut out);
        if old.empty {
            writer
                .write_event(Event::Start(old.opening.clone()))
                .map_err(xml_err)?;
        } else {
            writer
                .get_mut()
                .extend_from_slice(&doc[old.start..old.open_end]);
        }
        emit_paragraph(&mut writer, new, prefix)?;
        if old.empty {
            writer
                .write_event(Event::End(BytesEnd::new(name)))
                .map_err(xml_err)?;
        } else {
            writer
                .get_mut()
                .extend_from_slice(&doc[old.close_start..old.end]);
        }
        cursor = old.end;
    }
    if changed {
        out.extend_from_slice(&doc[cursor..]);
        if parse_paragraphs(&out)?.len() != originals.len() {
            return Err(invalid("replacement changed paragraph topology"));
        }
        pkg.set_part(PART_DOCUMENT, out);
    }
    Ok(())
}

fn write_text<W: std::io::Write>(writer: &mut Writer<W>, text: &str, prefix: &str) -> Result<()> {
    if text.chars().any(|c| !matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')) {
        return Err(invalid("replacement contains an invalid XML character"));
    }
    let mut start = 0;
    for (index, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), '\0')))
    {
        if matches!(c, '\t' | '\n' | '\r' | '\0') {
            if start < index {
                let name = format!("{prefix}t");
                let mut tag = BytesStart::new(&name);
                tag.push_attribute(("xml:space", "preserve"));
                writer.write_event(Event::Start(tag)).map_err(xml_err)?;
                writer
                    .write_event(Event::Text(BytesText::new(&text[start..index])))
                    .map_err(xml_err)?;
                writer
                    .write_event(Event::End(BytesEnd::new(name)))
                    .map_err(xml_err)?;
            }
            if c != '\0' {
                let local = match c {
                    '\t' => "tab",
                    '\n' => "br",
                    _ => "cr",
                };
                writer
                    .write_event(Event::Empty(BytesStart::new(format!("{prefix}{local}"))))
                    .map_err(xml_err)?;
                start = index + c.len_utf8();
            }
        }
    }
    Ok(())
}

fn emit_paragraph<W: std::io::Write>(
    writer: &mut Writer<W>,
    info: &ParagraphInfo,
    prefix: &str,
) -> Result<()> {
    if let Some(pp) = &info.ppr_xml {
        writer.get_mut().write_all(pp)?;
    }
    for start in &info.leading_bookmark_starts {
        writer.get_mut().write_all(start)?;
    }
    for run in &info.runs {
        let name = format!("{prefix}r");
        writer
            .write_event(Event::Start(BytesStart::new(&name)))
            .map_err(xml_err)?;
        if let Some(rpr) = &run.rpr_xml {
            writer.get_mut().write_all(rpr)?;
        }
        write_text(writer, &run.text, prefix)?;
        for reference in &run.footnote_refs {
            writer.get_mut().write_all(reference)?;
        }
        writer
            .write_event(Event::End(BytesEnd::new(name)))
            .map_err(xml_err)?;
    }
    for end in &info.trailing_bookmark_ends {
        writer.get_mut().write_all(end)?;
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
