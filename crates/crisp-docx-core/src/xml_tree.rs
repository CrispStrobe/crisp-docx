//! Minimal lossless XML tree for surgical edits.
//!
//! The existing streaming modules rewrite whole paragraphs; edits that must
//! leave everything else byte-identical (format one word, change one
//! paragraph's spacing) need random access instead. This tree keeps every
//! node it does not understand verbatim: attribute values and text stay in
//! their escaped source form, comments / PIs / CDATA / doctype are kept raw,
//! and self-closing elements are re-emitted self-closing.

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::{Error, Result};

/// A node of the tree.
#[derive(Debug, Clone)]
pub enum Node {
    /// An element.
    Elem(Elem),
    /// Character data, still XML-escaped exactly as in the source.
    Text(Vec<u8>),
    /// Anything else (declaration, comment, PI, CDATA, doctype), verbatim.
    Raw(Vec<u8>),
}

/// An element with raw (escaped) attribute values.
#[derive(Debug, Clone, Default)]
pub struct Elem {
    /// Qualified name as written, e.g. `w:p`.
    pub name: Vec<u8>,
    /// Attributes in source order: (qualified name, escaped value).
    pub attrs: Vec<(Vec<u8>, Vec<u8>)>,
    /// Child nodes.
    pub children: Vec<Node>,
    /// Emit as `<x/>` when it has no children.
    pub self_closing: bool,
}

impl Elem {
    /// New empty element.
    pub fn new(name: impl Into<Vec<u8>>) -> Self {
        Elem {
            name: name.into(),
            attrs: Vec::new(),
            children: Vec::new(),
            self_closing: true,
        }
    }

    /// Local part of the name (after the prefix).
    pub fn local(&self) -> &[u8] {
        match self.name.iter().position(|&b| b == b':') {
            Some(i) => &self.name[i + 1..],
            None => &self.name,
        }
    }

    /// Prefix of the name, if any.
    pub fn prefix(&self) -> &[u8] {
        match self.name.iter().position(|&b| b == b':') {
            Some(i) => &self.name[..i],
            None => b"",
        }
    }

    /// Raw attribute value by qualified name.
    pub fn attr(&self, name: &[u8]) -> Option<&[u8]> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_slice())
    }

    /// Set (or add) an attribute; `value` is plain text and gets escaped.
    pub fn set_attr(&mut self, name: &[u8], value: &str) {
        let v = escape(value).into_bytes();
        if let Some(slot) = self.attrs.iter_mut().find(|(k, _)| k == name) {
            slot.1 = v;
        } else {
            self.attrs.push((name.to_vec(), v));
        }
    }

    /// Remove an attribute if present.
    pub fn remove_attr(&mut self, name: &[u8]) {
        self.attrs.retain(|(k, _)| k != name);
    }

    /// Child elements (skipping text/raw nodes).
    pub fn elems(&self) -> impl Iterator<Item = &Elem> {
        self.children.iter().filter_map(|n| match n {
            Node::Elem(e) => Some(e),
            _ => None,
        })
    }

    /// Index of the first child element with this qualified name.
    pub fn child_index(&self, name: &[u8]) -> Option<usize> {
        self.children
            .iter()
            .position(|n| matches!(n, Node::Elem(e) if e.name == name))
    }

    /// First child element with this qualified name.
    pub fn child(&self, name: &[u8]) -> Option<&Elem> {
        self.elems().find(|e| e.name == name)
    }

    /// Mutable first child element with this qualified name.
    pub fn child_mut(&mut self, name: &[u8]) -> Option<&mut Elem> {
        self.children.iter_mut().find_map(|n| match n {
            Node::Elem(e) if e.name == name => Some(e),
            _ => None,
        })
    }

    /// Concatenated unescaped text of direct text children.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for n in &self.children {
            if let Node::Text(t) = n {
                out.push_str(&unescape(t));
            }
        }
        out
    }

    /// Serialize this element (and descendants).
    pub fn write(&self, out: &mut Vec<u8>) {
        out.push(b'<');
        out.extend_from_slice(&self.name);
        for (k, v) in &self.attrs {
            out.push(b' ');
            out.extend_from_slice(k);
            out.extend_from_slice(b"=\"");
            for &b in v {
                if b == b'"' {
                    out.extend_from_slice(b"&quot;");
                } else {
                    out.push(b);
                }
            }
            out.push(b'"');
        }
        if self.children.is_empty() && self.self_closing {
            out.extend_from_slice(b"/>");
            return;
        }
        out.push(b'>');
        for c in &self.children {
            match c {
                Node::Elem(e) => e.write(out),
                Node::Text(t) | Node::Raw(t) => out.extend_from_slice(t),
            }
        }
        out.extend_from_slice(b"</");
        out.extend_from_slice(&self.name);
        out.push(b'>');
    }
}

/// A parsed document: prolog nodes, the root element, trailing nodes.
#[derive(Debug, Clone)]
pub struct Document {
    /// Nodes before the root element (XML declaration, comments).
    pub prolog: Vec<Node>,
    /// The root element.
    pub root: Elem,
    /// Nodes after the root element.
    pub epilog: Vec<Node>,
}

impl Document {
    /// Parse a part. `part` is only used in error messages.
    pub fn parse(bytes: &[u8], part: &str) -> Result<Self> {
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(false);
        reader.config_mut().check_end_names = false;
        let mut stack: Vec<Elem> = Vec::new();
        let mut prolog = Vec::new();
        let mut epilog = Vec::new();
        let mut root: Option<Elem> = None;
        let mut buf = Vec::new();
        let err = |e: quick_xml::Error| Error::XmlParse {
            part: part.to_string(),
            source: e,
        };
        loop {
            let ev = reader.read_event_into(&mut buf).map_err(err)?;
            let node: Option<Node> = match ev {
                Event::Eof => break,
                Event::Start(e) => {
                    let mut el = Elem::new(e.name().as_ref().to_vec());
                    el.self_closing = false;
                    for a in e.attributes().with_checks(false) {
                        let a = a.map_err(|e| err(e.into()))?;
                        el.attrs.push((a.key.as_ref().to_vec(), a.value.to_vec()));
                    }
                    stack.push(el);
                    None
                }
                Event::Empty(e) => {
                    let mut el = Elem::new(e.name().as_ref().to_vec());
                    for a in e.attributes().with_checks(false) {
                        let a = a.map_err(|e| err(e.into()))?;
                        el.attrs.push((a.key.as_ref().to_vec(), a.value.to_vec()));
                    }
                    Some(Node::Elem(el))
                }
                Event::End(_) => {
                    let el = stack
                        .pop()
                        .ok_or_else(|| Error::Step(format!("{part}: unbalanced end tag")))?;
                    Some(Node::Elem(el))
                }
                Event::Text(t) => Some(Node::Text(t.as_ref().to_vec())),
                Event::CData(t) => {
                    let mut v = b"<![CDATA[".to_vec();
                    v.extend_from_slice(t.as_ref());
                    v.extend_from_slice(b"]]>");
                    Some(Node::Raw(v))
                }
                Event::Comment(t) => {
                    let mut v = b"<!--".to_vec();
                    v.extend_from_slice(t.as_ref());
                    v.extend_from_slice(b"-->");
                    Some(Node::Raw(v))
                }
                Event::Decl(d) => {
                    let mut v = b"<?".to_vec();
                    v.extend_from_slice(d.as_ref());
                    v.extend_from_slice(b"?>");
                    Some(Node::Raw(v))
                }
                Event::PI(p) => {
                    let mut v = b"<?".to_vec();
                    v.extend_from_slice(p.as_ref());
                    v.extend_from_slice(b"?>");
                    Some(Node::Raw(v))
                }
                Event::DocType(d) => {
                    let mut v = b"<!DOCTYPE ".to_vec();
                    v.extend_from_slice(d.as_ref());
                    v.push(b'>');
                    Some(Node::Raw(v))
                }
            };
            if let Some(node) = node {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(node);
                } else if let Node::Elem(el) = node {
                    if root.is_none() {
                        root = Some(el);
                    } else {
                        return Err(Error::Step(format!("{part}: multiple root elements")));
                    }
                } else if root.is_none() {
                    prolog.push(node);
                } else {
                    epilog.push(node);
                }
            }
            buf.clear();
        }
        if !stack.is_empty() {
            return Err(Error::Step(format!("{part}: unclosed element")));
        }
        let root = root.ok_or_else(|| Error::Step(format!("{part}: no root element")))?;
        Ok(Document {
            prolog,
            root,
            epilog,
        })
    }

    /// Serialize back to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for n in &self.prolog {
            if let Node::Raw(t) | Node::Text(t) = n {
                out.extend_from_slice(t);
            }
        }
        self.root.write(&mut out);
        for n in &self.epilog {
            if let Node::Raw(t) | Node::Text(t) = n {
                out.extend_from_slice(t);
            }
        }
        out
    }

    /// Prefix bound to `ns` on the root element (e.g. `w`), if any.
    pub fn prefix_for(&self, ns: &[u8]) -> Option<Vec<u8>> {
        self.root.attrs.iter().find_map(|(k, v)| {
            if v.as_slice() == ns {
                k.strip_prefix(b"xmlns:").map(|p| p.to_vec())
            } else {
                None
            }
        })
    }
}

/// Escape text for element content or attribute values.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Unescape XML text (predefined entities and numeric references).
pub fn unescape(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    if !s.contains('&') {
        return s.into_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest: &str = &s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = match rest.find(';') {
            Some(e) if e < 12 => e,
            _ => {
                out.push('&');
                rest = &rest[1..];
                continue;
            }
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                u32::from_str_radix(&ent[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_is_lossless_for_untouched_content() {
        let src = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="urn:w"><!-- c --><w:body><w:p><w:r><w:t xml:space="preserve"> a &amp; b &#x263A; </w:t></w:r><w:r/></w:p></w:body></w:document>"#;
        let doc = Document::parse(src, "t").unwrap();
        assert_eq!(doc.to_bytes(), src.to_vec());
        assert_eq!(doc.prefix_for(b"urn:w").unwrap(), b"w".to_vec());
    }

    #[test]
    fn unescape_handles_entities() {
        assert_eq!(
            unescape(b"a &amp; &#x41;&#66; &bogus; &"),
            "a & AB &bogus; &"
        );
    }
}
