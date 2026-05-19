//! OOXML namespace constants and qualified-name helpers.
//!
//! `quick-xml` works with byte slices, so these are `&'static [u8]`.

/// `w` — `wordprocessingml/2006/main`.
pub const W: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// `w14` — Microsoft 2010 extensions to `w` (paraId / textId live here).
pub const W14: &[u8] = b"http://schemas.microsoft.com/office/word/2010/wordml";

/// `r` — relationships namespace, used inside the document body.
pub const R: &[u8] = b"http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// `rels` — package-level relationships namespace.
pub const PKG_RELS: &[u8] = b"http://schemas.openxmlformats.org/package/2006/relationships";

/// `ct` — content-types namespace.
pub const CONTENT_TYPES_NS: &[u8] = b"http://schemas.openxmlformats.org/package/2006/content-types";

/// `xml:space` is the only foreign-namespace attribute we actively author.
pub const XML_SPACE: &[u8] = b"http://www.w3.org/XML/1998/namespace";

/// Standard part path: the main story.
pub const PART_DOCUMENT: &str = "word/document.xml";
/// Standard part path: footnotes.
pub const PART_FOOTNOTES: &str = "word/footnotes.xml";
/// Standard part path: endnotes.
pub const PART_ENDNOTES: &str = "word/endnotes.xml";
/// Standard part path: the document's rels file.
pub const PART_DOCUMENT_RELS: &str = "word/_rels/document.xml.rels";
/// Standard part path: the OPC content types listing.
pub const PART_CONTENT_TYPES: &str = "[Content_Types].xml";

/// Content type for the footnotes part.
pub const CT_FOOTNOTES: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";
/// Content type for the endnotes part.
pub const CT_ENDNOTES: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml";

/// Relationship type for the footnotes part.
pub const REL_TYPE_FOOTNOTES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
/// Relationship type for the endnotes part.
pub const REL_TYPE_ENDNOTES: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes";
