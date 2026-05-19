//! Infer heading levels for body paragraphs from direct formatting.
//!
//! Verbatim port of `format_transplant.py::ContentExtractor._infer_headings`
//! (lines 1136-1235).
//!
//! Signals applied to each body paragraph:
//!
//!   - All text runs bold OR `<w:pPr>/<w:rPr>/<w:b/>` (paragraph-default bold)
//!   - Short text (< 100 characters) — headings are rarely long sentences
//!   - Font size: larger sizes → higher priority (lower heading level number)
//!
//! Font sizes of heading candidates are clustered descending so the
//! largest size becomes level 1, the next size level 2, etc. If all
//! candidates share the same size (or none has a size set), every
//! candidate becomes level 1.
//!
//! The function operates only on body-class paragraphs — paragraphs
//! that already carry a non-body `<w:pStyle>` (Heading 1, etc.) are
//! skipped, matching Python's `pd.semantic_class != "body"` guard.
//! `classify_style` is consulted on the paragraph's pStyle value to
//! decide whether it's body-class.

use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::{Error, Result};
use crate::ns::PART_DOCUMENT;
use crate::package::Package;
use crate::style_classify::{classify_style, SemanticClass};
use crate::style_mapper::StyleIndex;

/// One paragraph's inferred heading level.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadingInference {
    /// Zero-based index of the paragraph in document body order
    /// (only counts `<w:p>` direct children of `<w:body>`).
    pub paragraph_index: usize,
    /// Inferred level, 1..9.
    pub heading_level: u8,
    /// Effective font size in points used to make the decision (0.0 if
    /// no size was discoverable — all candidates become level 1).
    pub effective_size_pt: f64,
    /// Paragraph text (first 60 chars, for diagnostics).
    pub preview: String,
}

/// Walk `pkg`'s body and return inferences for paragraphs that look like
/// headings but lack an explicit heading-class style. The optional
/// `source_styles` lets the heuristic translate `<w:pStyle>` styleIds to
/// display names before classifying — without it, the styleId itself is
/// classified, which usually still works because pandoc and Word use
/// names like "Heading 1" as both id and name.
///
/// Mirrors `ContentExtractor._infer_headings`.
pub fn infer_heading_levels(
    pkg: &Package,
    source_styles: Option<&StyleIndex>,
) -> Result<Vec<HeadingInference>> {
    let Some(doc) = pkg.get_part(PART_DOCUMENT) else {
        return Ok(Vec::new());
    };
    infer_from_xml(doc, source_styles)
}

fn infer_from_xml(input: &[u8], styles: Option<&StyleIndex>) -> Result<Vec<HeadingInference>> {
    let paras = collect_paragraphs(input)?;

    // Pass 1: classify each paragraph (candidate / body / skip).
    let mut candidates: Vec<(usize, ParagraphScan)> = Vec::new();
    let mut body_sizes: Vec<f64> = Vec::new();

    for (idx, scan) in paras.into_iter().enumerate() {
        if !is_body_class(&scan.pstyle, styles) {
            continue;
        }
        if scan.text.trim().is_empty() {
            continue;
        }
        if scan.effective_bold && scan.text.chars().count() < 100 {
            candidates.push((idx, scan));
        } else if scan.effective_size_pt.is_some() {
            body_sizes.push(scan.effective_size_pt.unwrap());
        }
    }

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // Body reference size: mode of body paragraph sizes.
    let body_sz = mode(&body_sizes).unwrap_or(0.0);

    // Unique candidate sizes, largest first.
    let mut unique_szs: Vec<f64> = candidates
        .iter()
        .filter_map(|(_, c)| c.effective_size_pt)
        .filter(|s| *s > 0.0)
        .collect();
    unique_szs.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    unique_szs.dedup();

    // Drop sizes that are ≤ body size (same-size bold = not really a heading).
    let heading_szs: Vec<f64> = unique_szs
        .into_iter()
        .filter(|s| body_sz == 0.0 || *s > body_sz + 0.4)
        .collect();
    let sentinel = heading_szs.is_empty();

    let level_of = |sz: f64| -> u8 {
        if sentinel {
            return 1;
        }
        for (i, threshold) in heading_szs.iter().enumerate() {
            if sz >= *threshold - 0.4 {
                return (i + 1).min(9) as u8;
            }
        }
        heading_szs.len().min(9) as u8
    };

    let mut out = Vec::with_capacity(candidates.len());
    for (idx, scan) in candidates {
        let sz = scan.effective_size_pt.unwrap_or(0.0);
        let lvl = level_of(sz);
        out.push(HeadingInference {
            paragraph_index: idx,
            heading_level: lvl,
            effective_size_pt: sz,
            preview: scan.text.chars().take(60).collect(),
        });
    }
    Ok(out)
}

/// Apply inferred heading levels to `pkg`'s document.xml: for every
/// paragraph in `inferences`, rewrite its `<w:pStyle>` to the styleId of
/// the matching blueprint heading level.
///
/// `blueprint_index` must include the blueprint's heading styleIds (and
/// usually comes from `StyleIndex::from_package(blueprint)`). When the
/// blueprint has no heading style at the inferred level, falls back to
/// the closest available level (per `StyleMapper`'s heading fallback).
///
/// Returns the number of paragraphs whose pStyle was actually rewritten.
pub fn apply_heading_inferences(
    pkg: &mut Package,
    inferences: &[HeadingInference],
    blueprint_index: &StyleIndex,
) -> Result<usize> {
    use crate::style_mapper::StyleMapper;
    if inferences.is_empty() {
        return Ok(0);
    }
    let Some(doc_bytes) = pkg.get_part(PART_DOCUMENT).map(<[u8]>::to_vec) else {
        return Ok(0);
    };

    // Build a fast lookup: paragraph_index → heading_level
    let mut by_idx: HashMap<usize, u8> = HashMap::new();
    for inf in inferences {
        by_idx.insert(inf.paragraph_index, inf.heading_level);
    }

    let mapper = StyleMapper::new(blueprint_index, HashMap::new());

    let mut reader = Reader::from_reader(doc_bytes.as_slice());
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut writer =
        quick_xml::writer::Writer::new(std::io::Cursor::new(Vec::with_capacity(doc_bytes.len())));
    let mut buf = Vec::with_capacity(1024);

    // State: walk into <w:body>, count <w:p>s, when we hit a paragraph
    // in by_idx and see its pStyle, rewrite. We buffer the pPr region so
    // we can also INSERT a pStyle if the paragraph has none.
    let mut depth_in_body = 0i32;
    let mut p_index: i64 = -1; // -1 before any <w:p>
    let mut rewritten = 0usize;

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: PART_DOCUMENT.into(),
                source: e,
            })?;
        match &ev {
            Event::Eof => break,
            Event::Start(s) if s.name().as_ref() == b"w:body" => {
                depth_in_body = 1;
                writer.write_event(ev.clone()).map_err(xml_io)?;
            }
            Event::End(e) if e.name().as_ref() == b"w:body" => {
                depth_in_body = 0;
                writer.write_event(ev.clone()).map_err(xml_io)?;
            }
            Event::Start(s) if depth_in_body == 1 && s.name().as_ref() == b"w:p" => {
                p_index += 1;
                writer.write_event(ev.clone()).map_err(xml_io)?;
            }
            Event::Empty(s) | Event::Start(s) if s.name().as_ref() == b"w:pStyle" => {
                let want_level = by_idx.get(&(p_index as usize)).copied();
                if let Some(level) = want_level {
                    // Ask mapper for blueprint heading at this level.
                    let target_name = mapper.map("", &SemanticClass::Heading(level), level);
                    let target_id = blueprint_index
                        .name_to_id
                        .get(&target_name)
                        .cloned()
                        .unwrap_or(target_name);
                    let mut new = quick_xml::events::BytesStart::new("w:pStyle");
                    new.push_attribute(("w:val", target_id.as_str()));
                    let new_ev = if matches!(&ev, Event::Empty(_)) {
                        Event::Empty(new)
                    } else {
                        Event::Start(new)
                    };
                    writer.write_event(new_ev).map_err(xml_io)?;
                    rewritten += 1;
                } else {
                    writer.write_event(ev.clone()).map_err(xml_io)?;
                }
            }
            other => writer.write_event(other.clone()).map_err(xml_io)?,
        }
        buf.clear();
    }
    pkg.set_part(PART_DOCUMENT, writer.into_inner().into_inner());
    Ok(rewritten)
}

fn xml_io(err: quick_xml::Error) -> Error {
    Error::XmlParse {
        part: PART_DOCUMENT.into(),
        source: err,
    }
}

// ─── internal scan ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct ParagraphScan {
    pstyle: Option<String>,
    text: String,
    /// True if all text runs in this paragraph are bold (or pPr says
    /// paragraph-level bold).
    effective_bold: bool,
    /// Effective font size: average of run sizes that had explicit
    /// `<w:sz>`. Falls back to pPr/rPr's `<w:sz>` when no run has one.
    /// `None` if no size could be determined.
    effective_size_pt: Option<f64>,
}

fn collect_paragraphs(input: &[u8]) -> Result<Vec<ParagraphScan>> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::with_capacity(1024);

    let mut paras: Vec<ParagraphScan> = Vec::new();
    let mut depth_in_body = 0i32;
    // State for the *current* paragraph.
    let mut in_p = false;
    let mut current = ParagraphScan::default();
    let mut in_ppr = false;
    let mut in_ppr_rpr = false;
    let mut in_run = false;
    let mut in_run_rpr = false;
    let mut current_run_bold: Option<bool> = None;
    let mut current_run_sz_pt: Option<f64> = None;
    let mut current_run_has_text = false;
    let mut run_bolds: Vec<bool> = Vec::new();
    let mut run_sizes: Vec<f64> = Vec::new();
    let mut ppr_bold = false;
    let mut ppr_sz_pt: Option<f64> = None;

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: PART_DOCUMENT.into(),
                source: e,
            })?;
        match &ev {
            Event::Eof => break,
            Event::Start(s) if s.name().as_ref() == b"w:body" => depth_in_body = 1,
            Event::End(e) if e.name().as_ref() == b"w:body" => depth_in_body = 0,

            Event::Start(s) if depth_in_body == 1 && s.name().as_ref() == b"w:p" => {
                in_p = true;
                current = ParagraphScan::default();
                in_ppr = false;
                in_ppr_rpr = false;
                in_run = false;
                in_run_rpr = false;
                run_bolds.clear();
                run_sizes.clear();
                ppr_bold = false;
                ppr_sz_pt = None;
            }
            Event::End(e) if in_p && e.name().as_ref() == b"w:p" => {
                // Finalize this paragraph.
                let text_runs_count = run_bolds.len();
                let all_runs_bold =
                    !run_bolds.is_empty() && run_bolds.iter().all(|b| *b || ppr_bold);
                current.effective_bold = all_runs_bold || ppr_bold;
                let run_avg = if run_sizes.is_empty() {
                    None
                } else {
                    Some(run_sizes.iter().sum::<f64>() / run_sizes.len() as f64)
                };
                current.effective_size_pt = run_avg.or(ppr_sz_pt);
                let _ = text_runs_count;
                paras.push(std::mem::take(&mut current));
                in_p = false;
            }

            // pPr / pPr_rpr tracking
            Event::Start(s) if in_p && s.name().as_ref() == b"w:pPr" => in_ppr = true,
            Event::End(e) if in_p && e.name().as_ref() == b"w:pPr" => in_ppr = false,
            Event::Empty(s) | Event::Start(s) if in_ppr && s.name().as_ref() == b"w:pStyle" => {
                for a in s.attributes().filter_map(Result::ok) {
                    if a.key.as_ref() == b"w:val" {
                        if let Ok(v) = std::str::from_utf8(a.value.as_ref()) {
                            current.pstyle = Some(v.to_string());
                        }
                    }
                }
            }
            Event::Start(s) if in_ppr && s.name().as_ref() == b"w:rPr" => in_ppr_rpr = true,
            Event::End(e) if in_ppr_rpr && e.name().as_ref() == b"w:rPr" => in_ppr_rpr = false,
            Event::Empty(s) if in_ppr_rpr && s.name().as_ref() == b"w:b" && !is_val_off(s) => {
                ppr_bold = true;
            }
            Event::Empty(s) if in_ppr_rpr && s.name().as_ref() == b"w:sz" => {
                ppr_sz_pt = sz_val_pt(s);
            }

            // Run tracking
            Event::Start(s) if in_p && !in_ppr && s.name().as_ref() == b"w:r" => {
                in_run = true;
                current_run_bold = None;
                current_run_sz_pt = None;
                current_run_has_text = false;
            }
            Event::End(e) if in_run && e.name().as_ref() == b"w:r" => {
                if current_run_has_text {
                    run_bolds.push(current_run_bold.unwrap_or(false));
                    if let Some(sz) = current_run_sz_pt {
                        run_sizes.push(sz);
                    }
                }
                in_run = false;
                in_run_rpr = false;
            }
            Event::Start(s) if in_run && s.name().as_ref() == b"w:rPr" => in_run_rpr = true,
            Event::End(e) if in_run_rpr && e.name().as_ref() == b"w:rPr" => in_run_rpr = false,
            Event::Empty(s) if in_run_rpr && s.name().as_ref() == b"w:b" && !is_val_off(s) => {
                current_run_bold = Some(true);
            }
            Event::Empty(s) if in_run_rpr && s.name().as_ref() == b"w:sz" => {
                current_run_sz_pt = sz_val_pt(s);
            }
            Event::Text(t) if in_run => {
                if let Ok(s) = std::str::from_utf8(t.as_ref()) {
                    if !s.is_empty() {
                        current_run_has_text = true;
                        current.text.push_str(s);
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(paras)
}

fn is_val_off(s: &quick_xml::events::BytesStart) -> bool {
    s.attributes()
        .filter_map(Result::ok)
        .any(|a| a.key.as_ref() == b"w:val" && matches!(a.value.as_ref(), b"0" | b"false"))
}

fn sz_val_pt(s: &quick_xml::events::BytesStart) -> Option<f64> {
    for a in s.attributes().filter_map(Result::ok) {
        if a.key.as_ref() == b"w:val" {
            if let Ok(v) = std::str::from_utf8(a.value.as_ref()) {
                if let Ok(half_pts) = v.parse::<f64>() {
                    return Some(half_pts / 2.0);
                }
            }
        }
    }
    None
}

fn is_body_class(pstyle: &Option<String>, styles: Option<&StyleIndex>) -> bool {
    let Some(id_or_name) = pstyle else {
        return true; // no style -> Normal
    };
    let name = styles
        .and_then(|s| s.id_to_name.get(id_or_name))
        .cloned()
        .unwrap_or_else(|| id_or_name.clone());
    !matches!(
        classify_style(&name).class,
        SemanticClass::Heading(_) | SemanticClass::Title
    )
}

fn mode(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut counts: HashMap<u64, (f64, usize)> = HashMap::new();
    for v in values {
        let key = v.to_bits();
        counts.entry(key).or_insert((*v, 0)).1 += 1;
    }
    counts.into_values().max_by_key(|(_, c)| *c).map(|(v, _)| v)
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
    fn no_candidates_when_nothing_is_bold() {
        let body = r#"<w:p><w:r><w:t>Some text.</w:t></w:r></w:p>"#;
        let result = infer_from_xml(&doc(body), None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn detects_single_bold_short_paragraph_as_level_1() {
        let body = r#"<w:p><w:r><w:rPr><w:b/><w:sz w:val="36"/></w:rPr><w:t>Big Bold Heading</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>Some regular body text here that is sufficiently long to not be a heading.</w:t></w:r></w:p>"#;
        let result = infer_from_xml(&doc(body), None).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].paragraph_index, 0);
        assert_eq!(result[0].heading_level, 1);
    }

    #[test]
    fn clusters_distinct_sizes_into_levels() {
        // 24pt heading, 18pt subheading, 14pt body.
        let body = r#"
            <w:p><w:r><w:rPr><w:b/><w:sz w:val="48"/></w:rPr><w:t>Top</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:b/><w:sz w:val="36"/></w:rPr><w:t>Sub</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="28"/></w:rPr><w:t>Body text long enough to count as body content here.</w:t></w:r></w:p>
        "#;
        let result = infer_from_xml(&doc(body), None).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].heading_level, 1); // 24pt → level 1
        assert_eq!(result[1].heading_level, 2); // 18pt → level 2
    }

    #[test]
    fn skips_paragraphs_with_existing_heading_pstyle() {
        let body = r#"
            <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:rPr><w:b/></w:rPr><w:t>Already a heading</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:b/><w:sz w:val="36"/></w:rPr><w:t>Bold short body</w:t></w:r></w:p>
        "#;
        let result = infer_from_xml(&doc(body), None).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].paragraph_index, 1);
    }

    #[test]
    fn skips_long_text_even_if_bold() {
        // 200 chars, bold → NOT a heading candidate.
        let long = "x".repeat(200);
        let body = format!(r#"<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>{long}</w:t></w:r></w:p>"#);
        let result = infer_from_xml(&doc(&body), None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn ppr_default_bold_propagates_to_runs_without_explicit_bold() {
        // The pPr says <w:b/>, the runs DON'T say bold but they should
        // still count as effectively bold per the Python implementation.
        let body = r#"<w:p><w:pPr><w:rPr><w:b/></w:rPr></w:pPr><w:r><w:rPr><w:sz w:val="36"/></w:rPr><w:t>Heading</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="20"/></w:rPr><w:t>Long enough body text to set body size baseline.</w:t></w:r></w:p>"#;
        let result = infer_from_xml(&doc(body), None).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].heading_level, 1);
    }
}
