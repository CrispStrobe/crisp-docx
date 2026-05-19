//! Convert a docx between footnotes and endnotes.
//!
//! When the OOXML notes part is footnotes, Word renders the notes at the
//! bottom of each page; when it's endnotes, they appear at the end of the
//! document. The structural difference is small but spread across several
//! parts:
//!
//! - `word/footnotes.xml` becomes `word/endnotes.xml` (or vice versa)
//! - element local names switch: `w:footnotes` ↔ `w:endnotes`,
//!   `w:footnoteRef` ↔ `w:endnoteRef`, etc.
//! - style values switch: `FootnoteText` ↔ `EndnoteText`,
//!   `FootnoteReference` ↔ `EndnoteReference`
//! - in `word/document.xml`, every `<w:footnoteReference>` becomes
//!   `<w:endnoteReference>`
//! - `[Content_Types].xml` Override needs the new content type
//! - `word/_rels/document.xml.rels` needs the new relationship type +
//!   target
//!
//! The byte-level rewrites here are safe because the strings we substitute
//! are unique within the OOXML schema — `w:footnoteRef` doesn't collide
//! with any other element name, etc.

use crate::error::Result;
use crate::ns::{
    CT_ENDNOTES, CT_FOOTNOTES, PART_CONTENT_TYPES, PART_DOCUMENT, PART_DOCUMENT_RELS,
    PART_ENDNOTES, PART_FOOTNOTES, REL_TYPE_ENDNOTES, REL_TYPE_FOOTNOTES,
};
use crate::package::Package;

/// Which kind of note the package should end up containing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NotesKind {
    /// Word footnotes (at the bottom of each page).
    Footnotes,
    /// Word endnotes (at the end of the document).
    Endnotes,
}

/// Convert a package's notes from whichever kind it currently has to
/// `target`. No-op if `target` matches the current state, or if the
/// package has no notes part at all.
pub fn convert_notes_kind(pkg: &mut Package, target: NotesKind) -> Result<()> {
    let has_footnotes = pkg.get_part(PART_FOOTNOTES).is_some();
    let has_endnotes = pkg.get_part(PART_ENDNOTES).is_some();
    match (target, has_footnotes, has_endnotes) {
        (_, false, false) => Ok(()), // nothing to do
        (NotesKind::Footnotes, true, _) | (NotesKind::Endnotes, _, true) => Ok(()),
        (NotesKind::Endnotes, true, false) => convert(pkg, Direction::FootnotesToEndnotes),
        (NotesKind::Footnotes, false, true) => convert(pkg, Direction::EndnotesToFootnotes),
    }
}

enum Direction {
    FootnotesToEndnotes,
    EndnotesToFootnotes,
}

/// Substitution table for the body / notes XML parts.
fn substitutions(dir: &Direction) -> Vec<(&'static [u8], &'static [u8])> {
    match dir {
        Direction::FootnotesToEndnotes => vec![
            (b"w:footnotes", b"w:endnotes"),
            (b"w:footnote ", b"w:endnote "),
            (b"w:footnote>", b"w:endnote>"),
            (b"w:footnoteRef", b"w:endnoteRef"),
            (b"w:footnoteReference", b"w:endnoteReference"),
            (b"FootnoteText", b"EndnoteText"),
            (b"FootnoteReference", b"EndnoteReference"),
        ],
        Direction::EndnotesToFootnotes => vec![
            (b"w:endnotes", b"w:footnotes"),
            (b"w:endnote ", b"w:footnote "),
            (b"w:endnote>", b"w:footnote>"),
            (b"w:endnoteRef", b"w:footnoteRef"),
            (b"w:endnoteReference", b"w:footnoteReference"),
            (b"EndnoteText", b"FootnoteText"),
            (b"EndnoteReference", b"FootnoteReference"),
        ],
    }
}

fn convert(pkg: &mut Package, dir: Direction) -> Result<()> {
    let (src_part, dst_part, src_ct, dst_ct, src_rel, dst_rel) = match dir {
        Direction::FootnotesToEndnotes => (
            PART_FOOTNOTES,
            PART_ENDNOTES,
            CT_FOOTNOTES,
            CT_ENDNOTES,
            REL_TYPE_FOOTNOTES,
            REL_TYPE_ENDNOTES,
        ),
        Direction::EndnotesToFootnotes => (
            PART_ENDNOTES,
            PART_FOOTNOTES,
            CT_ENDNOTES,
            CT_FOOTNOTES,
            REL_TYPE_ENDNOTES,
            REL_TYPE_FOOTNOTES,
        ),
    };

    let subs = substitutions(&dir);

    // 1. Rewrite the notes part in place (still under its old name for now).
    if let Some(bytes) = pkg.get_part_mut(src_part) {
        for (old, new) in &subs {
            replace_all(bytes, old, new);
        }
    }
    // 2. Rename the part itself.
    pkg.rename_part(src_part, dst_part);

    // 3. Rewrite the document.xml references.
    if let Some(bytes) = pkg.get_part_mut(PART_DOCUMENT) {
        // For document.xml we only want the *reference* swap, not the
        // separator/note-element swap (which doesn't appear in document.xml
        // anyway). Use the same table — extra needles are harmless.
        for (old, new) in &subs {
            replace_all(bytes, old, new);
        }
    }

    // 4. Patch the rels file.
    if let Some(bytes) = pkg.get_part_mut(PART_DOCUMENT_RELS) {
        replace_all(bytes, src_rel.as_bytes(), dst_rel.as_bytes());
        // Also fix the Target attribute. We don't want to touch other paths
        // that happen to share the filename, so look for the explicit form.
        let needle_src = format!(r#"Target="{}"#, last_segment(src_part));
        let needle_dst = format!(r#"Target="{}"#, last_segment(dst_part));
        replace_all(bytes, needle_src.as_bytes(), needle_dst.as_bytes());
    }

    // 5. Patch the content types file.
    if let Some(bytes) = pkg.get_part_mut(PART_CONTENT_TYPES) {
        replace_all(bytes, src_ct.as_bytes(), dst_ct.as_bytes());
        let needle_src = format!(r#"/{src_part}""#);
        let needle_dst = format!(r#"/{dst_part}""#);
        replace_all(bytes, needle_src.as_bytes(), needle_dst.as_bytes());
    }

    Ok(())
}

fn last_segment(part: &str) -> &str {
    part.rsplit('/').next().unwrap_or(part)
}

/// In-place byte-level replace of `needle` with `replacement`. Returns the
/// new `Vec<u8>` only when length changes (we allocate fresh in that case);
/// otherwise mutates `data` in place.
fn replace_all(data: &mut Vec<u8>, needle: &[u8], replacement: &[u8]) {
    if needle.is_empty() {
        return;
    }
    if needle.len() == replacement.len() {
        // Same length — slide-and-overwrite is allocation-free.
        let mut i = 0;
        while i + needle.len() <= data.len() {
            if &data[i..i + needle.len()] == needle {
                data[i..i + needle.len()].copy_from_slice(replacement);
                i += needle.len();
            } else {
                i += 1;
            }
        }
    } else {
        // Different lengths — build a fresh buffer.
        let mut out = Vec::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            if i + needle.len() <= data.len() && &data[i..i + needle.len()] == needle {
                out.extend_from_slice(replacement);
                i += needle.len();
            } else {
                out.push(data[i]);
                i += 1;
            }
        }
        *data = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_all_same_length() {
        let mut v = b"hello world hello".to_vec();
        replace_all(&mut v, b"hello", b"HELLO");
        assert_eq!(&v, b"HELLO world HELLO");
    }

    #[test]
    fn replace_all_growing() {
        let mut v = b"ab".to_vec();
        replace_all(&mut v, b"a", b"AAA");
        assert_eq!(&v, b"AAAb");
    }

    #[test]
    fn replace_all_shrinking() {
        let mut v = b"AAAbAAA".to_vec();
        replace_all(&mut v, b"AAA", b"a");
        assert_eq!(&v, b"aba");
    }
}
