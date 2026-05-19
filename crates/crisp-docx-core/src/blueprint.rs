//! `BlueprintSchema` — combined snapshot of a blueprint docx package.
//!
//! Aggregates the slices that other primitives in this crate produce:
//!
//! - [`StyleIndex`] — full table of `<w:style>` entries + styleId↔name
//!   maps + body-paragraph style inventory (already ported in
//!   `style_mapper`).
//! - [`FootnoteFormat`] — marker rPr + separator (already ported in
//!   `footnote_format`).
//! - [`SectionInfo`] *(new)* — page size, margins, gutter, header /
//!   footer distance, orientation — ported from
//!   `BlueprintAnalyzer._sections`.
//! - default font + size — resolved from styles.xml's `<w:docDefaults>`
//!   and the `Normal` style — ported from `BlueprintAnalyzer._defaults`.
//!
//! `analyze_blueprint(pkg)` wraps all of those into one call, matching
//! `format_transplant.py::BlueprintAnalyzer.analyze`.

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::{Error, Result};
use crate::footnote_format::{extract_footnote_format, FootnoteFormat};
use crate::ns::{PART_DOCUMENT, PART_FOOTNOTES};
use crate::package::Package;
use crate::style_mapper::StyleIndex;

/// One section's geometry in points. `None` means the attribute wasn't
/// present on the `<w:sectPr>` (Word's defaults apply).
///
/// Mirrors `format_transplant.py::BlueprintAnalyzer._sections` output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SectionInfo {
    /// Index of this section in document order (0-based).
    pub index: usize,
    /// Page width in points (twips / 20).
    pub page_width_pt: Option<f64>,
    /// Page height in points.
    pub page_height_pt: Option<f64>,
    /// Left margin in points.
    pub left_margin_pt: Option<f64>,
    /// Right margin in points.
    pub right_margin_pt: Option<f64>,
    /// Top margin in points.
    pub top_margin_pt: Option<f64>,
    /// Bottom margin in points.
    pub bottom_margin_pt: Option<f64>,
    /// Gutter in points (extra space for binding).
    pub gutter_pt: Option<f64>,
    /// Header distance in points.
    pub header_distance_pt: Option<f64>,
    /// Footer distance in points.
    pub footer_distance_pt: Option<f64>,
    /// `"portrait"` / `"landscape"` — from `<w:pgSz w:orient="…"/>`.
    pub orientation: Option<String>,
}

/// Full schema extracted from a blueprint docx package.
///
/// Mirrors `format_transplant.py::BlueprintSchema` minus the fields we
/// already cover with dedicated structs (`StyleIndex`, `FootnoteFormat`).
#[derive(Debug, Clone, Default)]
pub struct BlueprintSchema {
    /// Sections in document order. Most docs have exactly one.
    pub sections: Vec<SectionInfo>,
    /// Default font name (the value of `<w:rFonts w:ascii="…"/>` in
    /// docDefaults / Normal). Defaults to `"Times New Roman"` matching
    /// the Python implementation.
    pub default_font: String,
    /// Default font size in points (the value of `<w:sz w:val="N"/>` / 2).
    /// Defaults to 12.0 matching the Python implementation.
    pub default_font_size_pt: f64,
    /// Full style table + body inventory + styleId↔name maps.
    pub styles: StyleIndex,
    /// Footnote-marker formatting.
    pub footnote_format: FootnoteFormat,
}

/// Read all relevant blueprint metadata in one pass.
///
/// Mirrors `BlueprintAnalyzer.analyze`. Errors during section parsing
/// are caught and skipped (matches Python's behaviour: log + continue).
pub fn analyze_blueprint(pkg: &Package) -> Result<BlueprintSchema> {
    let mut schema = BlueprintSchema {
        default_font: "Times New Roman".into(),
        default_font_size_pt: 12.0,
        ..Default::default()
    };
    schema.styles = StyleIndex::from_package(pkg)?;
    schema.footnote_format = if pkg.get_part(PART_FOOTNOTES).is_some() {
        extract_footnote_format(pkg)?
    } else {
        FootnoteFormat::default()
    };
    if let Some(doc) = pkg.get_part(PART_DOCUMENT) {
        schema.sections = parse_sections(doc)?;
    }
    if let Some(styles) = pkg.get_part("word/styles.xml") {
        if let Some((font, size)) = parse_defaults(styles)? {
            if let Some(f) = font {
                schema.default_font = f;
            }
            if let Some(s) = size {
                schema.default_font_size_pt = s;
            }
        }
    }
    Ok(schema)
}

/// Twips (1/20th of a point) → points.
fn twips_to_pt(s: &str) -> Option<f64> {
    s.parse::<f64>()
        .ok()
        .map(|n| (n / 20.0 * 100.0).round() / 100.0)
}

/// Walk `document.xml` and harvest every `<w:sectPr>` element's
/// page-size + margin attributes. The body's trailing `<w:sectPr>` is
/// the document's primary section; earlier `<w:sectPr>` elements inside
/// `<w:pPr>` are section breaks.
fn parse_sections(input: &[u8]) -> Result<Vec<SectionInfo>> {
    let mut out = Vec::new();
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::with_capacity(1024);

    let mut current: Option<SectionInfo> = None;
    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: PART_DOCUMENT.into(),
                source: e,
            })?;
        match &ev {
            Event::Eof => break,
            // Self-closing `<w:sectPr/>` carries no child elements but
            // still represents a section — emit an empty SectionInfo.
            Event::Empty(s) if s.name().as_ref() == b"w:sectPr" => {
                out.push(SectionInfo {
                    index: out.len(),
                    ..Default::default()
                });
            }
            Event::Start(s) if s.name().as_ref() == b"w:sectPr" => {
                current = Some(SectionInfo {
                    index: out.len(),
                    ..Default::default()
                });
            }
            Event::End(e) if e.name().as_ref() == b"w:sectPr" => {
                if let Some(s) = current.take() {
                    out.push(s);
                }
            }
            Event::Empty(s) | Event::Start(s) if current.is_some() => {
                let Some(cur) = current.as_mut() else {
                    unreachable!()
                };
                match s.name().as_ref() {
                    b"w:pgSz" => {
                        for a in s.attributes().filter_map(Result::ok) {
                            let val = std::str::from_utf8(a.value.as_ref()).unwrap_or("");
                            match a.key.as_ref() {
                                b"w:w" => cur.page_width_pt = twips_to_pt(val),
                                b"w:h" => cur.page_height_pt = twips_to_pt(val),
                                b"w:orient" => cur.orientation = Some(val.to_string()),
                                _ => {}
                            }
                        }
                    }
                    b"w:pgMar" => {
                        for a in s.attributes().filter_map(Result::ok) {
                            let val = std::str::from_utf8(a.value.as_ref()).unwrap_or("");
                            match a.key.as_ref() {
                                b"w:left" => cur.left_margin_pt = twips_to_pt(val),
                                b"w:right" => cur.right_margin_pt = twips_to_pt(val),
                                b"w:top" => cur.top_margin_pt = twips_to_pt(val),
                                b"w:bottom" => cur.bottom_margin_pt = twips_to_pt(val),
                                b"w:gutter" => cur.gutter_pt = twips_to_pt(val),
                                b"w:header" => cur.header_distance_pt = twips_to_pt(val),
                                b"w:footer" => cur.footer_distance_pt = twips_to_pt(val),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// Read `<w:docDefaults>` and the `Normal` style's `<w:rPr>` from
/// `styles.xml`. Returns `Some((maybe_font, maybe_size_pt))` on success.
/// `<w:sz w:val="N"/>` is in half-points (Word stores it that way), so
/// we divide by 2 to convert to points.
fn parse_defaults(input: &[u8]) -> Result<Option<(Option<String>, Option<f64>)>> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::with_capacity(1024);

    let mut in_defaults = false;
    let mut in_default_rpr = false;
    let mut in_normal_style = false;
    let mut in_normal_rpr = false;
    let mut font: Option<String> = None;
    let mut size: Option<f64> = None;

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: "word/styles.xml".into(),
                source: e,
            })?;
        match &ev {
            Event::Eof => break,
            Event::Start(s) if s.name().as_ref() == b"w:docDefaults" => in_defaults = true,
            Event::End(e) if e.name().as_ref() == b"w:docDefaults" => in_defaults = false,
            Event::Start(s) if in_defaults && s.name().as_ref() == b"w:rPrDefault" => {}
            Event::Start(s) if in_defaults && s.name().as_ref() == b"w:rPr" => {
                in_default_rpr = true;
            }
            Event::End(e) if in_default_rpr && e.name().as_ref() == b"w:rPr" => {
                in_default_rpr = false;
            }
            Event::Start(s) if s.name().as_ref() == b"w:style" => {
                let is_normal = s
                    .attributes()
                    .filter_map(Result::ok)
                    .any(|a| a.key.as_ref() == b"w:styleId" && a.value.as_ref() == b"Normal");
                in_normal_style = is_normal;
            }
            Event::End(e) if e.name().as_ref() == b"w:style" => {
                in_normal_style = false;
                in_normal_rpr = false;
            }
            Event::Start(s) if in_normal_style && s.name().as_ref() == b"w:rPr" => {
                in_normal_rpr = true;
            }
            Event::End(e) if in_normal_rpr && e.name().as_ref() == b"w:rPr" => {
                in_normal_rpr = false;
            }
            Event::Empty(s) | Event::Start(s) if (in_default_rpr || in_normal_rpr) => {
                match s.name().as_ref() {
                    b"w:rFonts" if font.is_none() => {
                        for a in s.attributes().filter_map(Result::ok) {
                            if a.key.as_ref() == b"w:ascii" {
                                if let Ok(v) = std::str::from_utf8(a.value.as_ref()) {
                                    font = Some(v.to_string());
                                }
                            }
                        }
                    }
                    b"w:sz" if size.is_none() => {
                        for a in s.attributes().filter_map(Result::ok) {
                            if a.key.as_ref() == b"w:val" {
                                if let Ok(v) = std::str::from_utf8(a.value.as_ref()) {
                                    if let Ok(half_pts) = v.parse::<f64>() {
                                        size = Some(half_pts / 2.0);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(Some((font, size)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg_with(parts: &[(&str, &[u8])]) -> Package {
        use std::io::Write;
        let buf = std::io::Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(buf);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("[Content_Types].xml", opts).unwrap();
        zw.write_all(br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#)
            .unwrap();
        for (name, bytes) in parts {
            zw.start_file(*name, opts).unwrap();
            zw.write_all(bytes).unwrap();
        }
        let bytes = zw.finish().unwrap().into_inner();
        Package::from_bytes(&bytes).unwrap()
    }

    #[test]
    fn parses_letter_size_section() {
        let doc = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#;
        let sections = parse_sections(doc).unwrap();
        assert_eq!(sections.len(), 1);
        let s = &sections[0];
        assert_eq!(s.page_width_pt, Some(612.0));
        assert_eq!(s.page_height_pt, Some(792.0));
        assert_eq!(s.left_margin_pt, Some(72.0));
        assert_eq!(s.right_margin_pt, Some(72.0));
        assert_eq!(s.top_margin_pt, Some(72.0));
        assert_eq!(s.bottom_margin_pt, Some(72.0));
        assert_eq!(s.header_distance_pt, Some(36.0));
        assert_eq!(s.footer_distance_pt, Some(36.0));
        assert_eq!(s.gutter_pt, Some(0.0));
    }

    #[test]
    fn detects_landscape_orientation() {
        let doc = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:sectPr><w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/></w:sectPr></w:body></w:document>"#;
        let sections = parse_sections(doc).unwrap();
        assert_eq!(sections[0].orientation.as_deref(), Some("landscape"));
    }

    #[test]
    fn reads_doc_defaults_font_and_size() {
        let styles = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Times New Roman"/><w:sz w:val="28"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#;
        let (font, size) = parse_defaults(styles).unwrap().unwrap();
        assert_eq!(font.as_deref(), Some("Times New Roman"));
        assert_eq!(size, Some(14.0));
    }

    #[test]
    fn falls_back_to_normal_style_rpr() {
        let styles = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/><w:rPr><w:rFonts w:ascii="Calibri"/><w:sz w:val="22"/></w:rPr></w:style></w:styles>"#;
        let (font, size) = parse_defaults(styles).unwrap().unwrap();
        assert_eq!(font.as_deref(), Some("Calibri"));
        assert_eq!(size, Some(11.0));
    }

    #[test]
    fn analyze_combines_all_slices() {
        let doc = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Heading</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:body></w:document>"#;
        let styles = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Arial"/><w:sz w:val="20"/></w:rPr></w:rPrDefault></w:docDefaults><w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style></w:styles>"#;
        let pkg = pkg_with(&[("word/document.xml", doc), ("word/styles.xml", styles)]);
        let schema = analyze_blueprint(&pkg).unwrap();
        assert_eq!(schema.default_font, "Arial");
        assert_eq!(schema.default_font_size_pt, 10.0);
        assert_eq!(schema.sections.len(), 1);
        assert_eq!(schema.sections[0].page_width_pt, Some(612.0));
        assert!(schema.styles.styles.contains_key("heading 1"));
        assert!(schema.styles.body_para_style_names.contains("Heading1"));
    }
}
