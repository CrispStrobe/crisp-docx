//! Rewrite Apple `textutil`'s non-OOXML element local names in document /
//! footnotes / endnotes parts.
//!
//! textutil emits `<w:sz-cs>`, `<w:b-cs>`, `<w:i-cs>` — those hyphenated
//! local names aren't part of the OOXML schema and Word's strict validator
//! complains. The standard names use camelCase (`<w:szCs>`, `<w:bCs>`,
//! `<w:iCs>`). Since these are well-formed XML names with a `:` separator
//! we can do this safely with a byte-level rewrite.

use crate::error::Result;
use crate::ns::{PART_DOCUMENT, PART_ENDNOTES, PART_FOOTNOTES};
use crate::package::Package;

/// Pairs of (textutil-name, OOXML-name). Stored as byte slices so the
/// rewrite can be a single `Vec::replace`-style pass per part.
const RENAMES: &[(&[u8], &[u8])] = &[
    (b"w:sz-cs", b"w:szCs"),
    (b"w:b-cs", b"w:bCs"),
    (b"w:i-cs", b"w:iCs"),
];

/// Rename non-standard tags in every relevant part. Returns the total
/// number of byte-level substitutions performed.
pub fn normalize_tags(pkg: &mut Package) -> Result<usize> {
    let mut total = 0;
    for part in [PART_DOCUMENT, PART_FOOTNOTES, PART_ENDNOTES] {
        let Some(bytes) = pkg.get_part_mut(part) else {
            continue;
        };
        let mut renamed_here = 0;
        for (old, new) in RENAMES {
            renamed_here += replace_in_place(bytes, old, new);
        }
        total += renamed_here;
    }
    Ok(total)
}

/// In-place byte replacement of `needle` with `replacement` in `data`.
/// Returns the number of replacements made.
///
/// Assumes `replacement.len() <= needle.len()` (true for our renames). If
/// you ever change `RENAMES` to grow byte length we need a slower
/// allocate-and-copy path; assert it here so a future change doesn't
/// silently lose bytes.
fn replace_in_place(data: &mut Vec<u8>, needle: &[u8], replacement: &[u8]) -> usize {
    assert!(replacement.len() <= needle.len());
    if needle.is_empty() {
        return 0;
    }
    let mut writes = 0usize;
    let mut read = 0usize;
    let mut write = 0usize;
    while read < data.len() {
        if read + needle.len() <= data.len() && &data[read..read + needle.len()] == needle {
            data[write..write + replacement.len()].copy_from_slice(replacement);
            write += replacement.len();
            read += needle.len();
            writes += 1;
        } else {
            data[write] = data[read];
            read += 1;
            write += 1;
        }
    }
    data.truncate(write);
    writes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_in_place_basic() {
        let mut v = b"<w:sz-cs val=\"24\"/>".to_vec();
        let n = replace_in_place(&mut v, b"w:sz-cs", b"w:szCs");
        assert_eq!(n, 1);
        assert_eq!(&v, b"<w:szCs val=\"24\"/>");
    }

    #[test]
    fn replace_in_place_multiple() {
        let mut v = b"<w:sz-cs/><w:sz-cs/>".to_vec();
        let n = replace_in_place(&mut v, b"w:sz-cs", b"w:szCs");
        assert_eq!(n, 2);
        assert_eq!(&v, b"<w:szCs/><w:szCs/>");
    }

    #[test]
    fn replace_in_place_no_match() {
        let mut v = b"<w:szCs/>".to_vec();
        let n = replace_in_place(&mut v, b"w:sz-cs", b"w:szCs");
        assert_eq!(n, 0);
        assert_eq!(&v, b"<w:szCs/>");
    }
}
