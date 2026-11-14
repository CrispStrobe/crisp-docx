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
    PART_DOCUMENT_RELS, PART_ENDNOTES, PART_FOOTNOTES, PKG_RELS, REL_TYPE_ENDNOTES,
    REL_TYPE_FOOTNOTES, W,
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
/// `target`. No-op if the target kind is already present, or if the
/// package has no notes part at all. Packages containing both kinds are
/// left unchanged; their independent ID domains are not merged.
pub fn convert_notes_kind(pkg: &mut Package, target: NotesKind) -> Result<()> {
    let has_footnotes = pkg.get_part(PART_FOOTNOTES).is_some();
    let has_endnotes = pkg.get_part(PART_ENDNOTES).is_some();
    match (target, has_footnotes, has_endnotes) {
        (_, false, false) => Ok(()),
        (NotesKind::Footnotes, true, _) | (NotesKind::Endnotes, _, true) => Ok(()),
        (NotesKind::Endnotes, true, false) => convert(pkg, true),
        (NotesKind::Footnotes, false, true) => convert(pkg, false),
    }
}

struct Conversion {
    from: &'static str,
    to: &'static str,
    source_part: &'static str,
    target_part: &'static str,
    source_content_type: &'static str,
    target_content_type: &'static str,
    source_relationship: &'static str,
    target_relationship: &'static str,
    source_style: &'static str,
    target_style: &'static str,
}

fn convert(pkg: &mut Package, to_endnotes: bool) -> Result<()> {
    let c = if to_endnotes {
        Conversion {
            from: "footnote",
            to: "endnote",
            source_part: PART_FOOTNOTES,
            target_part: PART_ENDNOTES,
            source_content_type: CT_FOOTNOTES,
            target_content_type: CT_ENDNOTES,
            source_relationship: REL_TYPE_FOOTNOTES,
            target_relationship: REL_TYPE_ENDNOTES,
            source_style: "Footnote",
            target_style: "Endnote",
        }
    } else {
        Conversion {
            from: "endnote",
            to: "footnote",
            source_part: PART_ENDNOTES,
            target_part: PART_FOOTNOTES,
            source_content_type: CT_ENDNOTES,
            target_content_type: CT_FOOTNOTES,
            source_relationship: REL_TYPE_ENDNOTES,
            target_relationship: REL_TYPE_FOOTNOTES,
            source_style: "Endnote",
            target_style: "Footnote",
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
    pkg.rename_part(c.source_part, c.target_part);
    // Both owners stay in word/, so every relative resource target is
    // unchanged. Preserve this relationship part byte-for-byte.
    pkg.rename_part(&source_rels, target_rels);
    Ok(())
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
    let is_w = matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == W);
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
    let convert_rel = is_rel && attr(b"Type") == Some(c.source_relationship);
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
        let (attr_ns, attr_local) = reader.resolve_attribute(a.key);
        let new_value = if is_w
            && matches!(local.as_ref(), b"pStyle" | b"rStyle")
            && matches!(attr_ns, ResolveResult::Bound(ns) if ns.as_ref() == W)
            && attr_local.as_ref() == b"val"
        {
            ["Text", "Reference"].into_iter().find_map(|suffix| {
                (value == &format!("{}{suffix}", c.source_style))
                    .then(|| format!("{}{suffix}", c.target_style))
            })
        } else if convert_rel && a.key.as_ref() == b"Type" {
            Some(c.target_relationship.to_string())
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
                Event::Start(rewrite_element(&e, &reader, part, c)?)
            }
            Event::Empty(e) => {
                if depth == 0 {
                    roots += 1;
                }
                Event::Empty(rewrite_element(&e, &reader, part, c)?)
            }
            Event::End(e) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| invalid(part, "unbalanced XML closing element"))?;
                let (namespace, local) = reader.resolve_element(e.name());
                if matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == W) {
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
                Event::Text(e.into_owned())
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
