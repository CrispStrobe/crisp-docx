//! Convert footnotes and endnotes by their XML identities, preserving text
//! and each note part's relationship owner. All rewrites are prepared before
//! the package is changed, so a malformed required part cannot leave a
//! partially converted package.

use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;
use quick_xml::Writer;

use crate::error::{Error, Result};
use crate::ns::{
    CONTENT_TYPES_NS, CT_ENDNOTES, CT_FOOTNOTES, PART_CONTENT_TYPES, PART_DOCUMENT,
    PART_DOCUMENT_RELS, PART_ENDNOTES, PART_FOOTNOTES, PKG_RELS, W,
};
use crate::package::Package;

const STRICT_W: &[u8] = b"http://purl.oclc.org/ooxml/wordprocessingml/main";
const RELATIONSHIP_BASES: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/",
    "http://purl.oclc.org/ooxml/officeDocument/relationships/",
];

fn is_word_namespace(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == W || ns.as_ref() == STRICT_W)
}

fn is_note_relationship(value: Option<&str>, kind: &str) -> bool {
    value.is_some_and(|value| {
        RELATIONSHIP_BASES.iter().any(|base| {
            value
                .strip_prefix(base)
                .is_some_and(|suffix| suffix == format!("{kind}s"))
        })
    })
}

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
/// `target`. An opposite-kind part containing only separators does not block
/// conversion. Packages with normal notes of both kinds are left unchanged;
/// their independent ID domains are not merged.
pub fn convert_notes_kind(pkg: &mut Package, target: NotesKind) -> Result<()> {
    let has_footnotes = pkg.get_part(PART_FOOTNOTES).is_some();
    let has_endnotes = pkg.get_part(PART_ENDNOTES).is_some();
    match (target, has_footnotes, has_endnotes) {
        (_, false, false) => Ok(()),
        (_, true, true) => {
            let (source, destination, from, to) = match target {
                NotesKind::Endnotes => (PART_FOOTNOTES, PART_ENDNOTES, "footnote", "endnote"),
                NotesKind::Footnotes => (PART_ENDNOTES, PART_FOOTNOTES, "endnote", "footnote"),
            };
            if normal_note_count(pkg.get_part(destination).unwrap(), destination, to)? > 0
                || normal_note_count(pkg.get_part(source).unwrap(), source, from)? == 0
            {
                return Ok(());
            }
            let doc = pkg
                .get_part(PART_DOCUMENT)
                .ok_or_else(|| invalid(PART_DOCUMENT, "missing document part"))?;
            if contains_word_element(doc, PART_DOCUMENT, &format!("{to}Reference"))? {
                return Err(invalid(
                    destination,
                    "separator-only destination has active note references",
                ));
            }
            convert(pkg, target == NotesKind::Endnotes, true)
        }
        (NotesKind::Footnotes, true, false) | (NotesKind::Endnotes, false, true) => Ok(()),
        (NotesKind::Endnotes, true, false) => convert(pkg, true, false),
        (NotesKind::Footnotes, false, true) => convert(pkg, false, false),
    }
}

struct Conversion {
    from: &'static str,
    to: &'static str,
    source_part: &'static str,
    target_part: &'static str,
    source_content_type: &'static str,
    target_content_type: &'static str,
    replace_placeholder: bool,
}

fn convert(pkg: &mut Package, to_endnotes: bool, replace_placeholder: bool) -> Result<()> {
    let c = if to_endnotes {
        Conversion {
            from: "footnote",
            to: "endnote",
            source_part: PART_FOOTNOTES,
            target_part: PART_ENDNOTES,
            source_content_type: CT_FOOTNOTES,
            target_content_type: CT_ENDNOTES,
            replace_placeholder,
        }
    } else {
        Conversion {
            from: "endnote",
            to: "footnote",
            source_part: PART_ENDNOTES,
            target_part: PART_FOOTNOTES,
            source_content_type: CT_ENDNOTES,
            target_content_type: CT_FOOTNOTES,
            replace_placeholder,
        }
    };
    let source_rels = format!("word/_rels/{}s.xml.rels", c.from);
    let target_rels = format!("word/_rels/{}s.xml.rels", c.to);
    if pkg.get_part(&target_rels).is_some() {
        return Err(invalid(
            &target_rels,
            "destination notes relationship part already exists",
        ));
    }
    let updates = [
        c.source_part,
        PART_DOCUMENT,
        PART_DOCUMENT_RELS,
        PART_CONTENT_TYPES,
    ]
    .into_iter()
    .map(|part| {
        let bytes = pkg
            .get_part(part)
            .ok_or_else(|| invalid(part, "missing required notes-conversion part"))?;
        Ok((part, rewrite_xml(bytes, part, &c)?))
    })
    .collect::<Result<Vec<_>>>()?;
    for (part, bytes) in updates {
        pkg.set_part(part, bytes);
    }
    if replace_placeholder {
        pkg.remove_part(c.target_part);
    }
    pkg.rename_part(c.source_part, c.target_part);
    // Both owners stay in word/, so every relative resource target is
    // unchanged. Preserve this relationship part byte-for-byte.
    pkg.rename_part(&source_rels, target_rels);
    Ok(())
}

fn normal_note_count(bytes: &[u8], part: &str, note_name: &str) -> Result<usize> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut roots = 0usize;
    let mut normal = 0usize;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| xml_error(part, e))?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let (namespace, local) = reader.resolve_element(e.name());
                let word = is_word_namespace(&namespace);
                if depth == 0 {
                    roots += 1;
                    if !word || local.as_ref() != format!("{note_name}s").as_bytes() {
                        return Err(invalid(part, "unexpected notes root element"));
                    }
                } else if depth == 1 {
                    if !word || local.as_ref() != note_name.as_bytes() {
                        return Err(invalid(part, "unsupported child in notes part"));
                    }
                    let mut kind = None;
                    for a in e.attributes() {
                        let a = a.map_err(|e| xml_error(part, e.into()))?;
                        let (namespace, local) = reader.resolve_attribute(a.key);
                        let value = a.unescape_value().map_err(|e| xml_error(part, e))?;
                        if is_word_namespace(&namespace) && local.as_ref() == b"type" {
                            kind = Some(value.into_owned());
                        }
                    }
                    match kind.as_deref() {
                        None | Some("normal") => normal += 1,
                        Some("separator" | "continuationSeparator" | "continuationNotice") => {}
                        _ => return Err(invalid(part, "unknown note kind")),
                    }
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid(part, "unbalanced notes XML"))?;
            }
            Event::Text(e) => {
                let text = e.unescape().map_err(|e| xml_error(part, e))?;
                if depth == 0 && !text.trim().is_empty() {
                    return Err(invalid(part, "text outside notes root"));
                }
            }
            _ => {}
        }
        buf.clear();
    }
    if depth != 0 || roots != 1 {
        return Err(invalid(part, "incomplete notes XML"));
    }
    Ok(normal)
}

fn contains_word_element(bytes: &[u8], part: &str, name: &str) -> Result<bool> {
    let mut reader = NsReader::from_reader(bytes);
    let mut buf = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| xml_error(part, e))?
        {
            Event::Eof => return Ok(false),
            Event::Start(e) | Event::Empty(e) => {
                let (namespace, local) = reader.resolve_element(e.name());
                if is_word_namespace(&namespace) && local.as_ref() == name.as_bytes() {
                    return Ok(true);
                }
            }
            _ => {}
        }
        buf.clear();
    }
}

fn is_placeholder_declaration(
    e: &BytesStart<'_>,
    reader: &NsReader<&[u8]>,
    part: &str,
    c: &Conversion,
) -> Result<bool> {
    if !c.replace_placeholder {
        return Ok(false);
    }
    let (namespace, local) = reader.resolve_element(e.name());
    let relationship = part == PART_DOCUMENT_RELS
        && local.as_ref() == b"Relationship"
        && matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == PKG_RELS);
    let content_type = part == PART_CONTENT_TYPES
        && local.as_ref() == b"Override"
        && matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == CONTENT_TYPES_NS);
    let mut attributes = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|e| xml_error(part, e.into()))?;
        let value = a.unescape_value().map_err(|e| xml_error(part, e))?;
        attributes.push((a.key.as_ref().to_vec(), value.into_owned()));
    }
    let attr = |key: &[u8]| {
        attributes
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    if relationship && is_note_relationship(attr(b"Type"), c.to) {
        if attr(b"TargetMode").is_some_and(|value| value != "Internal")
            || attr(b"Target").and_then(resolve_note_target).as_deref() != Some(c.target_part)
        {
            return Err(invalid(
                part,
                "destination notes relationship does not identify the placeholder",
            ));
        }
        return Ok(true);
    }
    if content_type && attr(b"PartName") == Some(&format!("/{}", c.target_part)) {
        if attr(b"ContentType") != Some(c.target_content_type) {
            return Err(invalid(
                part,
                "destination content type does not identify the placeholder",
            ));
        }
        return Ok(true);
    }
    Ok(false)
}

fn invalid(part: &str, reason: &str) -> Error {
    Error::InvalidPackage {
        path: part.into(),
        reason: reason.into(),
    }
}

fn xml_error(part: &str, source: quick_xml::Error) -> Error {
    Error::XmlParse {
        part: part.into(),
        source,
    }
}

fn word_name(local: &[u8], c: &Conversion) -> Option<String> {
    for suffix in ["", "s", "Ref", "Reference"] {
        if local == format!("{}{suffix}", c.from).as_bytes() {
            return Some(format!("{}{suffix}", c.to));
        }
    }
    None
}

fn qualified_name(original: &[u8], local: &str) -> String {
    match original.iter().position(|b| *b == b':') {
        Some(i) => format!("{}:{local}", String::from_utf8_lossy(&original[..i])),
        None => local.to_string(),
    }
}

fn resolve_note_target(target: &str) -> Option<String> {
    let mut segments = if target.starts_with('/') {
        vec![]
    } else {
        vec!["word"]
    };
    for piece in target.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            x => segments.push(x),
        }
    }
    Some(segments.join("/"))
}

fn rewrite_element(
    e: &BytesStart<'_>,
    reader: &NsReader<&[u8]>,
    part: &str,
    c: &Conversion,
) -> Result<BytesStart<'static>> {
    let name = e.name();
    let (namespace, local) = reader.resolve_element(name);
    if let ResolveResult::Unknown(prefix) = namespace {
        return Err(xml_error(part, quick_xml::Error::UnknownPrefix(prefix)));
    }
    let is_w = is_word_namespace(&namespace);
    let is_rel = matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == PKG_RELS)
        && local.as_ref() == b"Relationship"
        && part == PART_DOCUMENT_RELS;
    let is_override = matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == CONTENT_TYPES_NS)
        && local.as_ref() == b"Override"
        && part == PART_CONTENT_TYPES;
    let renamed = is_w.then(|| word_name(local.as_ref(), c)).flatten();
    let new_name = renamed.as_ref().map(|s| qualified_name(name.as_ref(), s));
    let mut result = e.clone().into_owned();
    if let Some(new_name) = &new_name {
        result.set_name(new_name.as_bytes());
    }

    let attrs = e
        .attributes()
        .map(|a| a.map_err(|err| xml_error(part, err.into())))
        .collect::<Result<Vec<_>>>()?;
    let decoded = attrs
        .iter()
        .map(|a| {
            a.unescape_value()
                .map(|v| v.into_owned())
                .map_err(|err| xml_error(part, err))
        })
        .collect::<Result<Vec<_>>>()?;
    let attr = |key: &[u8]| {
        attrs
            .iter()
            .zip(&decoded)
            .find(|(a, _)| a.key.as_ref() == key)
            .map(|(_, v)| v.as_str())
    };
    let convert_rel = is_rel && is_note_relationship(attr(b"Type"), c.from);
    let convert_override = is_override && attr(b"PartName") == Some(&format!("/{}", c.source_part));
    if convert_rel {
        if attr(b"TargetMode").is_some_and(|s| s != "Internal") {
            return Err(invalid(part, "notes relationship must be internal"));
        }
        let target =
            attr(b"Target").ok_or_else(|| invalid(part, "notes relationship has no target"))?;
        if resolve_note_target(target).as_deref() != Some(c.source_part) {
            return Err(invalid(
                part,
                "notes relationship target does not identify the source notes part",
            ));
        }
    }
    result.clear_attributes();
    for (a, value) in attrs.iter().zip(&decoded) {
        // Style IDs are arbitrary references into styles.xml. Renaming a
        // built-in-looking ID would discard or change the source formatting.
        let new_value = if convert_rel && a.key.as_ref() == b"Type" {
            let base = value.rsplit_once('/').unwrap().0;
            Some(format!("{base}/{}s", c.to))
        } else if convert_rel && a.key.as_ref() == b"Target" {
            Some(format!("{}s.xml", c.to))
        } else if convert_override && a.key.as_ref() == b"PartName" {
            Some(format!("/{}", c.target_part))
        } else if convert_override && a.key.as_ref() == b"ContentType" {
            if value != c.source_content_type {
                return Err(invalid(
                    part,
                    "source notes content type does not match its note kind",
                ));
            }
            Some(c.target_content_type.to_string())
        } else {
            None
        };
        if let Some(value) = new_value {
            // Known replacement values have no XML metacharacters.
            result.push_attribute((a.key.as_ref(), value.as_bytes()));
        } else {
            result.push_attribute(a.clone());
        }
    }
    Ok(result)
}

fn rewrite_xml(bytes: &[u8], part: &str, c: &Conversion) -> Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(bytes);
    let mut writer = Writer::new(Vec::with_capacity(bytes.len()));
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut roots = 0usize;
    let mut skipped_depth = None;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| xml_error(part, e))?;
        let rewritten = match event {
            Event::Eof => break,
            Event::Start(e) => {
                if depth == 0 {
                    roots += 1;
                }
                depth += 1;
                if skipped_depth.is_some() || is_placeholder_declaration(&e, &reader, part, c)? {
                    skipped_depth.get_or_insert(depth);
                    buf.clear();
                    continue;
                }
                Event::Start(rewrite_element(&e, &reader, part, c)?)
            }
            Event::Empty(e) => {
                if depth == 0 {
                    roots += 1;
                }
                if skipped_depth.is_some() || is_placeholder_declaration(&e, &reader, part, c)? {
                    buf.clear();
                    continue;
                }
                Event::Empty(rewrite_element(&e, &reader, part, c)?)
            }
            Event::End(e) => {
                let closing_depth = depth;
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid(part, "unbalanced XML closing element"))?;
                if let Some(skipped) = skipped_depth {
                    if closing_depth == skipped {
                        skipped_depth = None;
                    }
                    buf.clear();
                    continue;
                }
                let (namespace, local) = reader.resolve_element(e.name());
                if is_word_namespace(&namespace) {
                    if let Some(local) = word_name(local.as_ref(), c) {
                        Event::End(BytesEnd::new(qualified_name(e.name().as_ref(), &local)))
                    } else {
                        Event::End(e.into_owned())
                    }
                } else {
                    Event::End(e.into_owned())
                }
            }
            Event::Text(e) => {
                let value = e.unescape().map_err(|e| xml_error(part, e))?;
                if depth == 0 && !value.trim().is_empty() {
                    return Err(invalid(part, "text outside the XML document element"));
                }
                if skipped_depth.is_some() {
                    buf.clear();
                    continue;
                }
                Event::Text(e.into_owned())
            }
            _ if skipped_depth.is_some() => {
                buf.clear();
                continue;
            }
            other => other.into_owned(),
        };
        writer
            .write_event(rewritten)
            .map_err(|e| xml_error(part, e))?;
        buf.clear();
    }
    if depth != 0 || roots != 1 {
        return Err(invalid(part, "expected one complete XML document element"));
    }
    Ok(writer.into_inner())
}
