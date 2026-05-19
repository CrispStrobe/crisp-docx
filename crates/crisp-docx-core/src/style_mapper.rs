//! Map source-document style names to blueprint style names.
//!
//! Verbatim port of `format_transplant.py::StyleMapper` (lines 1287-1481).
//!
//! Resolution order (`_resolve`):
//!
//! 1. User override (always wins, if target exists in blueprint).
//! 2. Semantic heading match — runs BEFORE name lookup so paragraphs
//!    reclassified as headings by content analysis get the heading
//!    style, not the original (probably "Normal") style.
//! 3. Exact name match in blueprint.
//! 4. Case-insensitive name match.
//! 5. Semantic class match (title / footnote / caption / blockquote /
//!    abstract).
//! 6. Fallback to blueprint "Normal" (or first available para style).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::style_classify::{classify_style, SemanticClass};

/// Subset of `format_transplant.py::BlueprintStyleInfo` we actually need
/// for mapping. Lightweight on purpose — `BlueprintAnalyzer` (when ported)
/// will produce a richer struct; this is the minimum viable interface so
/// [`StyleMapper`] can stand on its own.
#[derive(Debug, Clone, Default)]
pub struct StyleInfo {
    /// The style's display name (the `<w:name w:val="…"/>` text).
    pub name: String,
    /// The style's `w:styleId` — what `<w:pStyle w:val="…"/>` references.
    pub style_id: String,
    /// 1 = paragraph, 2 = character, 3 = table, 4 = numbering.
    pub type_val: u8,
    /// OOXML outline level (0 = H1 … 8 = H9). `None` if not a heading style.
    pub outline_level: Option<u8>,
}

/// Subset of `BlueprintSchema` needed by `StyleMapper`. The full Python
/// schema also tracks sections, defaults, body inventory, and footnote
/// format — those live in `FootnoteFormat` and a future BlueprintAnalyzer
/// port.
#[derive(Debug, Clone, Default)]
pub struct StyleIndex {
    /// `style_name -> info`.
    pub styles: BTreeMap<String, StyleInfo>,
    /// `style_id -> display_name` lookup. Needed because `<w:pStyle
    /// w:val="…"/>` references a styleId, but `StyleMapper.map()` keys
    /// on the display name (matches Python's behaviour and gets the
    /// multilingual `classify_style` patterns right).
    pub id_to_name: BTreeMap<String, String>,
    /// Inverse of [`Self::id_to_name`]: display name -> styleId. Used to
    /// translate a mapped name back into a styleId when writing
    /// `<w:pStyle>` in the output.
    pub name_to_id: BTreeMap<String, String>,
    /// Style names that actually appear on body paragraphs (used as
    /// tie-breaker when multiple styles claim the same outline level).
    pub body_para_style_names: BTreeSet<String>,
}

impl StyleIndex {
    /// Build a [`StyleIndex`] from both `word/styles.xml` and
    /// `word/document.xml` of a package. The document is scanned for
    /// `<w:pStyle w:val="X"/>` references to populate
    /// [`Self::body_para_style_names`], which `StyleMapper` uses as a
    /// tie-breaker.
    ///
    /// Returns an empty index (with default styles) if `word/styles.xml`
    /// isn't present in `pkg`.
    pub fn from_package(pkg: &crate::Package) -> crate::Result<Self> {
        let mut idx = if let Some(styles) = pkg.get_part("word/styles.xml") {
            Self::from_styles_xml(styles)?
        } else {
            Self::default()
        };
        if let Some(doc) = pkg.get_part("word/document.xml") {
            let names = scan_body_para_styles(doc)?;
            idx.body_para_style_names = names;
        }
        Ok(idx)
    }

    /// Build a [`StyleIndex`] from a `word/styles.xml` byte payload.
    /// Reads `<w:style>` elements and their `<w:name>`, `<w:outlineLvl>`,
    /// and `w:type` attributes.
    pub fn from_styles_xml(bytes: &[u8]) -> crate::Result<Self> {
        let mut idx = StyleIndex::default();
        let mut reader = quick_xml::reader::Reader::from_reader(bytes);
        reader.config_mut().trim_text(false);
        reader.config_mut().expand_empty_elements = false;
        let mut buf = Vec::with_capacity(1024);

        let mut current: Option<StyleInfo> = None;
        loop {
            let ev = reader
                .read_event_into(&mut buf)
                .map_err(|e| crate::Error::XmlParse {
                    part: "word/styles.xml".into(),
                    source: e,
                })?;
            match ev {
                quick_xml::events::Event::Eof => break,

                quick_xml::events::Event::Start(s) if s.name().as_ref() == b"w:style" => {
                    let mut info = StyleInfo::default();
                    for a in s.attributes().filter_map(Result::ok) {
                        let val = std::str::from_utf8(a.value.as_ref()).unwrap_or("");
                        match a.key.as_ref() {
                            b"w:type" => {
                                info.type_val = match val {
                                    "paragraph" => 1,
                                    "character" => 2,
                                    "table" => 3,
                                    "numbering" => 4,
                                    _ => 0,
                                };
                            }
                            b"w:styleId" => {
                                info.style_id = val.to_string();
                                if info.name.is_empty() {
                                    info.name = val.to_string();
                                }
                            }
                            _ => {}
                        }
                    }
                    current = Some(info);
                }
                quick_xml::events::Event::End(e) if e.name().as_ref() == b"w:style" => {
                    if let Some(info) = current.take() {
                        if !info.name.is_empty() {
                            if !info.style_id.is_empty() {
                                idx.id_to_name
                                    .insert(info.style_id.clone(), info.name.clone());
                                idx.name_to_id
                                    .insert(info.name.clone(), info.style_id.clone());
                            }
                            idx.styles.insert(info.name.clone(), info);
                        }
                    }
                }
                // <w:name w:val="…"/>  — visible name, overrides styleId
                quick_xml::events::Event::Empty(s) | quick_xml::events::Event::Start(s)
                    if current.is_some() && s.name().as_ref() == b"w:name" =>
                {
                    if let Some(c) = current.as_mut() {
                        for a in s.attributes().filter_map(Result::ok) {
                            if a.key.as_ref() == b"w:val" {
                                if let Ok(v) = std::str::from_utf8(a.value.as_ref()) {
                                    c.name = v.to_string();
                                }
                            }
                        }
                    }
                }
                // <w:outlineLvl w:val="N"/>
                quick_xml::events::Event::Empty(s) | quick_xml::events::Event::Start(s)
                    if current.is_some() && s.name().as_ref() == b"w:outlineLvl" =>
                {
                    if let Some(c) = current.as_mut() {
                        for a in s.attributes().filter_map(Result::ok) {
                            if a.key.as_ref() == b"w:val" {
                                if let Ok(v) = std::str::from_utf8(a.value.as_ref()) {
                                    if let Ok(n) = v.parse::<u8>() {
                                        c.outline_level = Some(n);
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            buf.clear();
        }
        Ok(idx)
    }
}

/// Walk `document.xml` and collect the set of style names referenced by
/// `<w:pStyle w:val="…"/>` directly inside paragraph properties — i.e.
/// the styles actually used on body paragraphs. Mirrors part of
/// `BlueprintAnalyzer._body_inventory`.
fn scan_body_para_styles(input: &[u8]) -> crate::Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let mut reader = quick_xml::reader::Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut buf = Vec::with_capacity(1024);
    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| crate::Error::XmlParse {
                part: "word/document.xml".into(),
                source: e,
            })?;
        match ev {
            quick_xml::events::Event::Eof => break,
            quick_xml::events::Event::Empty(s) | quick_xml::events::Event::Start(s)
                if s.name().as_ref() == b"w:pStyle" =>
            {
                for a in s.attributes().filter_map(Result::ok) {
                    if a.key.as_ref() == b"w:val" {
                        if let Ok(v) = std::str::from_utf8(a.value.as_ref()) {
                            out.insert(v.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

/// Rewrite every `<w:pStyle w:val="X"/>` in `pkg`'s body so the
/// referenced styleId points at the blueprint's equivalent.
///
/// The translation per `<w:pStyle val="X">` is:
///
/// 1. `source_index.id_to_name[X]` → source style's display name
/// 2. `mapper.map(name, classify_style(name), …)` → blueprint display name
/// 3. `blueprint_index.name_to_id[name]` → blueprint styleId
///
/// Steps 1 and 3 fall back to the original `val` if the lookup fails,
/// preserving robustness on docx whose ids don't match between sides.
///
/// Mirrors `DocumentBuilder._insert_elements` + `_style_id` in
/// `format_transplant.py`. Returns the number of paragraph styles whose
/// `val` ended up different from the input.
pub fn apply_style_mapping(
    pkg: &mut crate::Package,
    mapper: &StyleMapper,
    source_index: &StyleIndex,
    blueprint_index: &StyleIndex,
) -> crate::Result<usize> {
    let Some(bytes) = pkg.get_part("word/document.xml").map(<[u8]>::to_vec) else {
        return Ok(0);
    };
    let (new_bytes, rewritten) = rewrite_pstyles(&bytes, mapper, source_index, blueprint_index)?;
    if rewritten > 0 {
        pkg.set_part("word/document.xml", new_bytes);
    }
    Ok(rewritten)
}

fn rewrite_pstyles(
    input: &[u8],
    mapper: &StyleMapper,
    source_index: &StyleIndex,
    blueprint_index: &StyleIndex,
) -> crate::Result<(Vec<u8>, usize)> {
    use crate::style_classify::classify_style;

    let mut reader = quick_xml::reader::Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut writer =
        quick_xml::writer::Writer::new(std::io::Cursor::new(Vec::with_capacity(input.len())));
    let mut buf = Vec::with_capacity(1024);
    let mut rewritten = 0usize;

    loop {
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| crate::Error::XmlParse {
                part: "word/document.xml".into(),
                source: e,
            })?;
        let is_pstyle = matches!(
            &ev,
            quick_xml::events::Event::Empty(s) | quick_xml::events::Event::Start(s)
                if s.name().as_ref() == b"w:pStyle"
        );
        if is_pstyle {
            let is_empty = matches!(&ev, quick_xml::events::Event::Empty(_));
            let s = match &ev {
                quick_xml::events::Event::Empty(s) | quick_xml::events::Event::Start(s) => s,
                _ => unreachable!(),
            };
            let mut new = quick_xml::events::BytesStart::new("w:pStyle");
            let mut target_val: Option<String> = None;
            for a in s.attributes().filter_map(Result::ok) {
                if a.key.as_ref() == b"w:val" {
                    if let Ok(src_id) = std::str::from_utf8(a.value.as_ref()) {
                        // Translate src styleId -> src display name.
                        let src_name = source_index
                            .id_to_name
                            .get(src_id)
                            .map(String::as_str)
                            .unwrap_or(src_id);
                        // Classify by name; ask mapper for blueprint name.
                        let c = classify_style(src_name);
                        let mapped_name = mapper.map(src_name, &c.class, c.heading_level);
                        // Translate blueprint name -> blueprint styleId.
                        let target_id = blueprint_index
                            .name_to_id
                            .get(&mapped_name)
                            .cloned()
                            .unwrap_or_else(|| mapped_name.clone());
                        if target_id != src_id {
                            rewritten += 1;
                        }
                        target_val = Some(target_id);
                    }
                } else {
                    new.push_attribute(a);
                }
            }
            if let Some(val) = target_val {
                new.push_attribute(("w:val", val.as_str()));
            }
            let new_ev = if is_empty {
                quick_xml::events::Event::Empty(new)
            } else {
                quick_xml::events::Event::Start(new)
            };
            writer
                .write_event(new_ev)
                .map_err(|e| crate::Error::XmlParse {
                    part: "word/document.xml".into(),
                    source: e,
                })?;
        } else if matches!(&ev, quick_xml::events::Event::Eof) {
            break;
        } else {
            writer.write_event(ev).map_err(|e| crate::Error::XmlParse {
                part: "word/document.xml".into(),
                source: e,
            })?;
        }
        buf.clear();
    }
    Ok((writer.into_inner().into_inner(), rewritten))
}

/// Source-name → blueprint-name mapping policy.
#[derive(Debug)]
pub struct StyleMapper {
    bp_headings: BTreeMap<u8, String>,
    bp_title: Option<String>,
    bp_body: Option<String>,
    bp_footnote: Option<String>,
    bp_caption: Option<String>,
    bp_blockquote: Option<String>,
    bp_abstract: Option<String>,
    user_overrides: HashMap<String, String>,
    blueprint_styles: BTreeSet<String>,
    cache: std::sync::Mutex<HashMap<String, String>>,
}

impl StyleMapper {
    /// Build a `StyleMapper` from a [`StyleIndex`] + optional user-supplied
    /// `{src_style → bp_style}` overrides.
    ///
    /// Mirrors `StyleMapper.__init__` + `_build_lookup`.
    pub fn new(index: &StyleIndex, user_overrides: HashMap<String, String>) -> Self {
        let mut bp_headings: BTreeMap<u8, String> = BTreeMap::new();
        let mut bp_title: Option<String> = None;
        let mut bp_body: Option<String> = None;
        let mut bp_footnote: Option<String> = None;
        let mut bp_caption: Option<String> = None;
        let mut bp_blockquote: Option<String> = None;
        let mut bp_abstract: Option<String> = None;

        // Pass 1: outlineLvl in style XML (most reliable, language-independent).
        for (name, info) in &index.styles {
            if info.type_val != 1 {
                continue;
            }
            let Some(ol) = info.outline_level else {
                continue;
            };
            let level = ol + 1;
            if !(1..=9).contains(&level) {
                continue;
            }
            let used_first = index.body_para_style_names.contains(name);
            let entry = bp_headings.entry(level).or_default();
            if entry.is_empty() || used_first {
                *entry = name.clone();
            }
        }

        // Pass 2: semantic name classification fills gaps.
        for (name, info) in &index.styles {
            if info.type_val != 1 {
                continue;
            }
            let c = classify_style(name);
            match c.class {
                SemanticClass::Title if bp_title.is_none() => bp_title = Some(name.clone()),
                SemanticClass::Heading(level) => {
                    let entry = bp_headings.entry(level).or_default();
                    // Fill empty slot, OR prefer styles actually used in
                    // the blueprint body (used_first wins on ties).
                    if entry.is_empty() || index.body_para_style_names.contains(name) {
                        *entry = name.clone();
                    }
                }
                SemanticClass::Body if bp_body.is_none() => bp_body = Some(name.clone()),
                SemanticClass::Footnote if bp_footnote.is_none() => {
                    bp_footnote = Some(name.clone())
                }
                SemanticClass::Caption if bp_caption.is_none() => bp_caption = Some(name.clone()),
                SemanticClass::Blockquote if bp_blockquote.is_none() => {
                    bp_blockquote = Some(name.clone())
                }
                SemanticClass::Abstract if bp_abstract.is_none() => {
                    bp_abstract = Some(name.clone())
                }
                _ => {}
            }
        }

        // Fallback body style.
        if bp_body.is_none() {
            if index.styles.contains_key("Normal") {
                bp_body = Some("Normal".to_string());
            } else {
                bp_body = index
                    .styles
                    .iter()
                    .find(|(_, i)| i.type_val == 1)
                    .map(|(n, _)| n.clone())
                    .or_else(|| Some("Normal".to_string()));
            }
        }

        let blueprint_styles: BTreeSet<String> = index.styles.keys().cloned().collect();

        Self {
            bp_headings,
            bp_title,
            bp_body,
            bp_footnote,
            bp_caption,
            bp_blockquote,
            bp_abstract,
            user_overrides,
            blueprint_styles,
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Resolve a source style name to a blueprint style name.
    ///
    /// `sem_class` and `heading_level` are typically obtained by running
    /// [`classify_style`] on `src_name`, but the caller can override them
    /// after content analysis (e.g. when a paragraph styled "Normal" gets
    /// reclassified as a heading by bold/short-text heuristics).
    ///
    /// Mirrors `StyleMapper.map` + `_resolve`.
    pub fn map(&self, src_name: &str, sem_class: &SemanticClass, heading_level: u8) -> String {
        let cache_key = format!("{src_name}::{}::{heading_level}", sem_class.as_str());
        {
            let cache = self.cache.lock().unwrap();
            if let Some(hit) = cache.get(&cache_key) {
                return hit.clone();
            }
        }
        let resolved = self.resolve(src_name, sem_class, heading_level);
        self.cache
            .lock()
            .unwrap()
            .insert(cache_key, resolved.clone());
        resolved
    }

    fn resolve(&self, src_name: &str, sem_class: &SemanticClass, heading_level: u8) -> String {
        // 1. User override.
        if let Some(target) = self.user_overrides.get(src_name) {
            if self.blueprint_styles.contains(target) {
                return target.clone();
            }
        }

        // 2a. Semantic heading match — runs BEFORE name lookup.
        if matches!(sem_class, SemanticClass::Heading(_)) && heading_level > 0 {
            if let Some(name) = self.bp_headings.get(&heading_level) {
                return name.clone();
            }
            for delta in [1i32, -1, 2, -2, 3, -3] {
                let adj = heading_level as i32 + delta;
                if (1..=9).contains(&adj) {
                    if let Some(name) = self.bp_headings.get(&(adj as u8)) {
                        return name.clone();
                    }
                }
            }
            if let Some((_, name)) = self.bp_headings.iter().next() {
                return name.clone();
            }
        }

        // 2b. Exact name match.
        if self.blueprint_styles.contains(src_name) {
            return src_name.to_string();
        }

        // 3. Case-insensitive name match.
        let src_lo = src_name.to_lowercase();
        for bp_name in &self.blueprint_styles {
            if bp_name.to_lowercase() == src_lo {
                return bp_name.clone();
            }
        }

        // 4. Semantic class match (non-heading classes).
        match sem_class {
            SemanticClass::Title => {
                if let Some(t) = &self.bp_title {
                    return t.clone();
                }
                if let Some(h1) = self.bp_headings.get(&1) {
                    return h1.clone();
                }
            }
            SemanticClass::Footnote => {
                if let Some(n) = &self.bp_footnote {
                    return n.clone();
                }
            }
            SemanticClass::Caption => {
                if let Some(n) = &self.bp_caption {
                    return n.clone();
                }
            }
            SemanticClass::Blockquote => {
                if let Some(n) = &self.bp_blockquote {
                    return n.clone();
                }
            }
            SemanticClass::Abstract => {
                if let Some(n) = &self.bp_abstract {
                    return n.clone();
                }
            }
            _ => {}
        }

        // 5. Fallback.
        self.bp_body.clone().unwrap_or_else(|| "Normal".to_string())
    }

    /// Inspector: the blueprint's resolved heading lookup, for diagnostics
    /// and parity tests.
    pub fn bp_headings(&self) -> &BTreeMap<u8, String> {
        &self.bp_headings
    }
    /// The blueprint's resolved body / "Normal" style name, if any.
    pub fn bp_body(&self) -> Option<&str> {
        self.bp_body.as_deref()
    }
    /// The blueprint's resolved title style name, if any.
    pub fn bp_title(&self) -> Option<&str> {
        self.bp_title.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_with(styles: &[(&str, u8, Option<u8>)]) -> StyleIndex {
        let mut idx = StyleIndex::default();
        for (name, type_val, outline_level) in styles {
            idx.styles.insert(
                name.to_string(),
                StyleInfo {
                    name: name.to_string(),
                    style_id: name.to_string(),
                    type_val: *type_val,
                    outline_level: *outline_level,
                },
            );
            idx.id_to_name.insert(name.to_string(), name.to_string());
            idx.name_to_id.insert(name.to_string(), name.to_string());
        }
        idx
    }

    #[test]
    fn outline_level_drives_heading_assignments() {
        let idx = index_with(&[
            ("Normal", 1, None),
            ("Heading1Char", 2, Some(0)), // character style — must be ignored
            ("Heading 1", 1, Some(0)),
            ("Heading 2", 1, Some(1)),
            ("MyH3", 1, Some(2)),
        ]);
        let mapper = StyleMapper::new(&idx, HashMap::new());
        assert_eq!(mapper.bp_headings().get(&1), Some(&"Heading 1".to_string()));
        assert_eq!(mapper.bp_headings().get(&2), Some(&"Heading 2".to_string()));
        assert_eq!(mapper.bp_headings().get(&3), Some(&"MyH3".to_string()));
    }

    #[test]
    fn semantic_class_fills_gaps() {
        let idx = index_with(&[
            ("Normal", 1, None),
            ("Title", 1, None),
            ("Footnote Text", 1, None),
            ("Caption", 1, None),
            ("Quote", 1, None),
        ]);
        let mapper = StyleMapper::new(&idx, HashMap::new());
        assert_eq!(mapper.bp_title(), Some("Title"));
        assert_eq!(mapper.bp_body(), Some("Normal"));
    }

    #[test]
    fn user_override_wins_if_target_exists() {
        let idx = index_with(&[("Normal", 1, None), ("Heading 1", 1, Some(0))]);
        let mut overrides = HashMap::new();
        overrides.insert("SrcStyle".to_string(), "Heading 1".to_string());
        let mapper = StyleMapper::new(&idx, overrides);
        let result = mapper.map("SrcStyle", &SemanticClass::Unknown, 0);
        assert_eq!(result, "Heading 1");
    }

    #[test]
    fn user_override_ignored_if_target_missing() {
        let idx = index_with(&[("Normal", 1, None), ("Heading 1", 1, Some(0))]);
        let mut overrides = HashMap::new();
        overrides.insert("SrcStyle".to_string(), "DoesNotExist".to_string());
        let mapper = StyleMapper::new(&idx, overrides);
        let result = mapper.map("SrcStyle", &SemanticClass::Body, 0);
        // Should fall back to Normal (bp_body).
        assert_eq!(result, "Normal");
    }

    #[test]
    fn heading_reclassification_uses_blueprint_heading() {
        let idx = index_with(&[
            ("Normal", 1, None),
            ("Heading 1", 1, Some(0)),
            ("Heading 2", 1, Some(1)),
        ]);
        let mapper = StyleMapper::new(&idx, HashMap::new());
        // Source paragraph styled "Normal" but reclassified as Heading 2:
        let result = mapper.map("Normal", &SemanticClass::Heading(2), 2);
        assert_eq!(result, "Heading 2");
    }

    #[test]
    fn heading_falls_back_to_adjacent_level() {
        let idx = index_with(&[
            ("Normal", 1, None),
            ("Heading 1", 1, Some(0)),
            // No H4 in blueprint
            ("Heading 5", 1, Some(4)),
        ]);
        let mapper = StyleMapper::new(&idx, HashMap::new());
        let result = mapper.map("Foo", &SemanticClass::Heading(4), 4);
        // adj +1 = 5 ✓
        assert_eq!(result, "Heading 5");
    }

    #[test]
    fn exact_name_match_for_non_heading() {
        let idx = index_with(&[("Normal", 1, None), ("BodyText", 1, None)]);
        let mapper = StyleMapper::new(&idx, HashMap::new());
        assert_eq!(
            mapper.map("BodyText", &SemanticClass::Unknown, 0),
            "BodyText"
        );
    }

    #[test]
    fn case_insensitive_match() {
        let idx = index_with(&[("Normal", 1, None), ("BodyText", 1, None)]);
        let mapper = StyleMapper::new(&idx, HashMap::new());
        assert_eq!(
            mapper.map("bodytext", &SemanticClass::Unknown, 0),
            "BodyText"
        );
    }

    #[test]
    fn fallback_to_body_when_no_match() {
        let idx = index_with(&[("Normal", 1, None)]);
        let mapper = StyleMapper::new(&idx, HashMap::new());
        let result = mapper.map("Whatever", &SemanticClass::Unknown, 0);
        assert_eq!(result, "Normal");
    }

    #[test]
    fn fallback_uses_first_para_style_when_no_normal() {
        let idx = index_with(&[("OnlyStyle", 1, None)]);
        let mapper = StyleMapper::new(&idx, HashMap::new());
        let result = mapper.map("Whatever", &SemanticClass::Unknown, 0);
        assert_eq!(result, "OnlyStyle");
    }

    #[test]
    fn parses_styles_xml() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="Heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="character" w:styleId="FootnoteReference"><w:name w:val="Footnote Reference"/></w:style></w:styles>"#;
        let idx = StyleIndex::from_styles_xml(xml).unwrap();
        assert!(idx.styles.contains_key("Heading 1"));
        assert!(idx.styles.contains_key("Normal"));
        let h1 = idx.styles.get("Heading 1").unwrap();
        assert_eq!(h1.type_val, 1);
        assert_eq!(h1.outline_level, Some(0));
    }
}
