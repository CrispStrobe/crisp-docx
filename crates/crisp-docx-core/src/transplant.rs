//! Transplant the body of one docx into the package of another.
//!
//! This is the operation behind `format_transplant.py` in CrispTranslator:
//! you take a *blueprint* document for its formatting (styles, page
//! layout, headers, theme) and a *source* document for its content, and
//! produce a docx that wears the blueprint's clothes around the source's
//! words.
//!
//! ## Algorithm
//!
//! 1. Copy the blueprint package wholesale (caller chooses the target —
//!    typically `blueprint.clone()`).
//! 2. In the cloned `word/document.xml`, find `<w:body>...</w:body>` and
//!    locate its trailing `<w:sectPr>` (if any). Preserve that sectPr —
//!    it owns the page size, margins, headers/footers references, etc.
//! 3. Replace the body's content (everything between `<w:body>` and the
//!    trailing sectPr — or `</w:body>` if no sectPr) with the *inner*
//!    content of the source's body, dropping any source-side body-direct
//!    `<w:sectPr>`.
//! 4. Run [`crate::strip_rsids`] on the result — the source's runs carry
//!    rsid attributes that reference revision sessions in the source's
//!    `settings.xml`, which the blueprint package doesn't have. Without
//!    stripping, Word's strict validator fires the "found unreadable
//!    content" recovery dialog.
//! 5. Carry over the source's `word/footnotes.xml` / `word/endnotes.xml`
//!    if present, patching `[Content_Types].xml` and
//!    `word/_rels/document.xml.rels` so the references in the body
//!    resolve.
//!
//! ## Scope (MVP)
//!
//! - We use byte-level `<w:body>` / `<w:sectPr>` location. This handles
//!   the common case of a single body and a single trailing sectPr.
//! - We do *not* perform style mapping: paragraphs in the source that
//!   reference styles missing from the blueprint will fall back to
//!   Word's `Normal` style. A future phase wires in a style mapper
//!   (port of `format_transplant.py::StyleMapper`).
//! - We don't transplant headers/footers (they live in the blueprint
//!   and are intentionally kept).

use std::path::Path;

use crate::error::{Error, Result};
use crate::ns::{
    CT_ENDNOTES, CT_FOOTNOTES, PART_CONTENT_TYPES, PART_DOCUMENT, PART_DOCUMENT_RELS,
    PART_ENDNOTES, PART_FOOTNOTES, REL_TYPE_ENDNOTES, REL_TYPE_FOOTNOTES,
};
use crate::package::Package;
use crate::strip_rsids;

/// Replace the body of `blueprint` with the body of `source` and bring
/// across footnotes/endnotes when the source has them.
///
/// `blueprint` is mutated in place. Use `Package::clone` first if you want
/// to keep the original around.
pub fn transplant_body(blueprint: &mut Package, source: &Package) -> Result<()> {
    let bp_doc = blueprint
        .get_part(PART_DOCUMENT)
        .ok_or_else(|| invalid(blueprint, "missing word/document.xml in blueprint"))?
        .to_vec();
    let src_doc = source
        .get_part(PART_DOCUMENT)
        .ok_or_else(|| invalid(source, "missing word/document.xml in source"))?
        .to_vec();

    let bp = extract_blueprint_frame(&bp_doc)
        .ok_or_else(|| invalid(blueprint, "blueprint document.xml has no <w:body>"))?;
    let src_body = extract_source_body(&src_doc)
        .ok_or_else(|| invalid(source, "source document.xml has no <w:body>"))?;

    // Assemble: blueprint head ... <w:body> | source body | blueprint sectPr | </w:body> ...
    let mut new_doc = Vec::with_capacity(bp_doc.len() + src_body.inner_no_sectpr.len());
    new_doc.extend_from_slice(bp.before_and_body_open);
    new_doc.extend_from_slice(src_body.inner_no_sectpr);
    if let Some(sectpr) = bp.trailing_sectpr {
        new_doc.extend_from_slice(sectpr);
    }
    new_doc.extend_from_slice(bp.from_body_close);
    blueprint.set_part(PART_DOCUMENT, new_doc);

    // The transplanted runs carry rsids from the source's revision sessions;
    // those won't resolve against the blueprint's settings.xml. Strip them.
    strip_rsids(blueprint)?;

    // Carry footnotes / endnotes if source has them.
    transplant_aux_part(
        blueprint,
        source,
        PART_FOOTNOTES,
        CT_FOOTNOTES,
        REL_TYPE_FOOTNOTES,
    );
    transplant_aux_part(
        blueprint,
        source,
        PART_ENDNOTES,
        CT_ENDNOTES,
        REL_TYPE_ENDNOTES,
    );

    Ok(())
}

fn invalid(pkg: &Package, reason: &str) -> Error {
    Error::InvalidPackage {
        path: pkg.source().map(Path::to_path_buf).unwrap_or_default(),
        reason: reason.into(),
    }
}

struct BlueprintFrame<'a> {
    /// Bytes from the start of the document up through the `<w:body>` open
    /// element (inclusive).
    before_and_body_open: &'a [u8],
    /// The trailing `<w:sectPr>...</w:sectPr>` of the body, if present.
    trailing_sectpr: Option<&'a [u8]>,
    /// Bytes from `</w:body>` (inclusive) to end of document.
    from_body_close: &'a [u8],
}

struct SourceBody<'a> {
    /// Bytes of the body's inner content with any body-direct trailing
    /// `<w:sectPr>` stripped.
    inner_no_sectpr: &'a [u8],
}

/// Slice the blueprint document into the three regions we need to rebuild
/// it around the source's body content.
fn extract_blueprint_frame(doc: &[u8]) -> Option<BlueprintFrame<'_>> {
    let body_open_end = find_subseq(doc, b"<w:body>")? + b"<w:body>".len();
    let body_close_start = find_subseq(doc, b"</w:body>")?;
    let body_inner = &doc[body_open_end..body_close_start];

    let trailing_sectpr = find_trailing_sectpr(body_inner);

    Some(BlueprintFrame {
        before_and_body_open: &doc[..body_open_end],
        trailing_sectpr,
        from_body_close: &doc[body_close_start..],
    })
}

/// Inner body content of `doc`, with any body-direct trailing
/// `<w:sectPr>...</w:sectPr>` stripped off.
fn extract_source_body(doc: &[u8]) -> Option<SourceBody<'_>> {
    let body_open_end = find_subseq(doc, b"<w:body>")? + b"<w:body>".len();
    let body_close_start = find_subseq(doc, b"</w:body>")?;
    let body_inner = &doc[body_open_end..body_close_start];
    let trailing_sectpr = find_trailing_sectpr(body_inner);

    let inner_no_sectpr = match trailing_sectpr {
        Some(sect) => {
            let cut = body_inner.len() - sect.len();
            &body_inner[..cut]
        }
        None => body_inner,
    };

    Some(SourceBody { inner_no_sectpr })
}

/// The last `<w:sectPr ...> ... </w:sectPr>` in the body — assumed to be
/// the body-direct one (nested sectPr inside `<w:pPr>` are earlier).
fn find_trailing_sectpr(body_inner: &[u8]) -> Option<&[u8]> {
    let needle = b"<w:sectPr";
    let open_pos = body_inner
        .windows(needle.len())
        .rposition(|w| w == needle)?;
    Some(&body_inner[open_pos..])
}

fn find_subseq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Copy `part` from `source` into `blueprint` if present; ensure the
/// content-types Override and the document.xml.rels relationship exist
/// so the new part is referenced.
fn transplant_aux_part(
    blueprint: &mut Package,
    source: &Package,
    part_name: &str,
    content_type: &str,
    rel_type: &str,
) {
    let Some(bytes) = source.get_part(part_name) else {
        return;
    };
    blueprint.set_part(part_name, bytes.to_vec());

    // Patch [Content_Types].xml.
    if let Some(ct) = blueprint.get_part_mut(PART_CONTENT_TYPES) {
        let pn = format!("/{part_name}");
        if !memmem(ct, pn.as_bytes()) {
            let mut s = String::from_utf8_lossy(ct).into_owned();
            let inject = format!(r#"<Override PartName="{pn}" ContentType="{content_type}"/>"#);
            if let Some(pos) = s.rfind("</Types>") {
                s.insert_str(pos, &inject);
                *ct = s.into_bytes();
            }
        }
    }

    // Patch word/_rels/document.xml.rels.
    if let Some(rels) = blueprint.get_part_mut(PART_DOCUMENT_RELS) {
        if !memmem(rels, rel_type.as_bytes()) {
            let mut s = String::from_utf8_lossy(rels).into_owned();
            let target = part_name.rsplit('/').next().unwrap_or(part_name);
            let new_id = next_rid(&s);
            let inject =
                format!(r#"<Relationship Id="rId{new_id}" Type="{rel_type}" Target="{target}"/>"#);
            if let Some(pos) = s.rfind("</Relationships>") {
                s.insert_str(pos, &inject);
                *rels = s.into_bytes();
            }
        }
    }
}

fn memmem(hay: &[u8], needle: &[u8]) -> bool {
    find_subseq(hay, needle).is_some()
}

fn next_rid(rels_xml: &str) -> u32 {
    let mut max = 0u32;
    for (idx, _) in rels_xml.match_indices("Id=\"rId") {
        let tail = &rels_xml[idx + 7..];
        let n: u32 = tail
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        if n > max {
            max = n;
        }
    }
    max + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const BP: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>blueprint para 1</w:t></w:r></w:p><w:p><w:r><w:t>blueprint para 2</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr></w:body></w:document>"#;

    const SRC: &[u8] = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>source paragraph A</w:t></w:r></w:p><w:p><w:r><w:t>source paragraph B</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="8000" w:h="10000"/></w:sectPr></w:body></w:document>"#;

    #[test]
    fn extract_blueprint_frame_finds_sectpr() {
        let f = extract_blueprint_frame(BP).unwrap();
        assert!(f.before_and_body_open.ends_with(b"<w:body>"));
        assert!(f.from_body_close.starts_with(b"</w:body>"));
        let sect = f.trailing_sectpr.expect("blueprint has sectPr");
        assert!(sect.starts_with(b"<w:sectPr"));
        assert!(sect.contains_subseq(b"w=\"12240\""));
    }

    #[test]
    fn extract_source_body_strips_sectpr() {
        let s = extract_source_body(SRC).unwrap();
        let txt = std::str::from_utf8(s.inner_no_sectpr).unwrap();
        assert!(txt.contains("source paragraph A"));
        assert!(txt.contains("source paragraph B"));
        assert!(!txt.contains("w:sectPr"));
    }

    #[test]
    fn missing_body_returns_none() {
        let no_body = br#"<w:document xmlns:w="...">no body here</w:document>"#;
        assert!(extract_blueprint_frame(no_body).is_none());
        assert!(extract_source_body(no_body).is_none());
    }

    #[test]
    fn next_rid_picks_next_available() {
        assert_eq!(
            next_rid(r#"<Rels><Relationship Id="rId1"/><Relationship Id="rId3"/></Rels>"#),
            4
        );
        assert_eq!(next_rid("<Rels></Rels>"), 1);
    }

    // Tiny helper trait so the `contains_subseq` call above reads well.
    trait Subseq {
        fn contains_subseq(&self, needle: &[u8]) -> bool;
    }
    impl Subseq for &[u8] {
        fn contains_subseq(&self, needle: &[u8]) -> bool {
            find_subseq(self, needle).is_some()
        }
    }
}
