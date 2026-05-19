//! Classify Word style names into semantic classes + heading levels.
//!
//! Direct port of `format_transplant.py::classify_style` (lines 590-634)
//! plus its supporting pattern dictionaries (`HEADING_PATTERNS`,
//! `TITLE_PATTERNS`, `BODY_PATTERNS`, …) at lines 144-243.
//!
//! These power [`StyleMapper`](crate::style_mapper) — when the blueprint
//! lacks an exact match for a source paragraph's style name, the
//! mapper falls back on the semantic class returned here.

use std::sync::OnceLock;

/// Semantic class assigned to a style name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SemanticClass {
    /// A heading at level 1..=9.
    Heading(u8),
    /// A document title style.
    Title,
    /// A body / "Normal" / standard paragraph style.
    Body,
    /// Footnote-text style.
    Footnote,
    /// Figure / table caption style.
    Caption,
    /// Block quotation / quote style.
    Blockquote,
    /// Abstract / summary style.
    Abstract,
    /// No semantic class matched.
    Unknown,
}

impl SemanticClass {
    /// The Python implementation tags class strings like "heading1" / "body" /
    /// "title" / "footnote" / …; this exposes the same names for parity tests.
    pub fn as_str(&self) -> String {
        match self {
            SemanticClass::Heading(n) => format!("heading{n}"),
            SemanticClass::Title => "title".to_string(),
            SemanticClass::Body => "body".to_string(),
            SemanticClass::Footnote => "footnote".to_string(),
            SemanticClass::Caption => "caption".to_string(),
            SemanticClass::Blockquote => "blockquote".to_string(),
            SemanticClass::Abstract => "abstract".to_string(),
            SemanticClass::Unknown => "unknown".to_string(),
        }
    }
}

/// Result of classifying a single style name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StyleClassification {
    /// Semantic class the name maps to.
    pub class: SemanticClass,
    /// 1..=9 for headings, 0 otherwise.
    pub heading_level: u8,
}

/// Classify a Word style name. Direct port of
/// `format_transplant.py::classify_style`.
pub fn classify_style(style_name: &str) -> StyleClassification {
    let name_lo = style_name.to_lowercase();
    let name_lo = name_lo.trim();

    // 1. Heading patterns (exact / prefix)
    for &(level, patterns) in heading_patterns() {
        for pat in patterns.iter() {
            if name_lo == *pat {
                return StyleClassification {
                    class: SemanticClass::Heading(level),
                    heading_level: level,
                };
            }
        }
        for pat in patterns.iter() {
            if name_lo.starts_with(*pat) {
                return StyleClassification {
                    class: SemanticClass::Heading(level),
                    heading_level: level,
                };
            }
        }
    }

    // 2. Heading regex: catches "Ueberschrift_02", "Titre2", etc.
    if let Some(level) = match_heading_kw_re(name_lo) {
        return StyleClassification {
            class: SemanticClass::Heading(level),
            heading_level: level,
        };
    }

    // 3. Title
    if TITLE_PATTERNS.contains(&name_lo) {
        return StyleClassification {
            class: SemanticClass::Title,
            heading_level: 0,
        };
    }

    // 4. Other semantic classes — substring match
    for pat in FOOTNOTE_PATTERNS {
        if name_lo.contains(pat) {
            return StyleClassification {
                class: SemanticClass::Footnote,
                heading_level: 0,
            };
        }
    }
    for pat in CAPTION_PATTERNS {
        if name_lo.contains(pat) {
            return StyleClassification {
                class: SemanticClass::Caption,
                heading_level: 0,
            };
        }
    }
    for pat in BLOCKQUOTE_PATTERNS {
        if name_lo.contains(pat) {
            return StyleClassification {
                class: SemanticClass::Blockquote,
                heading_level: 0,
            };
        }
    }
    for pat in ABSTRACT_PATTERNS {
        if name_lo.contains(pat) {
            return StyleClassification {
                class: SemanticClass::Abstract,
                heading_level: 0,
            };
        }
    }
    for pat in BODY_PATTERNS {
        if name_lo == *pat || name_lo.starts_with(pat) {
            return StyleClassification {
                class: SemanticClass::Body,
                heading_level: 0,
            };
        }
    }

    StyleClassification {
        class: SemanticClass::Unknown,
        heading_level: 0,
    }
}

// ─── Pattern dictionaries ──────────────────────────────────────────────────

/// `[(level, &[patterns])]` — verbatim port of Python `HEADING_PATTERNS`.
/// Stored as a static slice so the lookup is allocation-free.
type HeadingPatternBlock = (u8, &'static [&'static str]);

fn heading_patterns() -> &'static [HeadingPatternBlock] {
    static CELL: OnceLock<[HeadingPatternBlock; 9]> = OnceLock::new();
    CELL.get_or_init(|| {
        [
            (
                1,
                &[
                    "heading 1",
                    "heading1",
                    "h1",
                    "überschrift 1",
                    "titre 1",
                    "titolo 1",
                    "encabezado 1",
                    "заголовок 1",
                    "标题 1",
                    "kop 1",
                    "nagłówek 1",
                    "rubrik 1",
                    "heading1char",
                ],
            ),
            (
                2,
                &[
                    "heading 2",
                    "heading2",
                    "h2",
                    "überschrift 2",
                    "titre 2",
                    "titolo 2",
                    "encabezado 2",
                    "заголовок 2",
                    "标题 2",
                    "kop 2",
                    "nagłówek 2",
                ],
            ),
            (
                3,
                &[
                    "heading 3",
                    "heading3",
                    "h3",
                    "überschrift 3",
                    "titre 3",
                    "titolo 3",
                    "encabezado 3",
                    "заголовок 3",
                    "标题 3",
                    "kop 3",
                    "nagłówek 3",
                ],
            ),
            (
                4,
                &[
                    "heading 4",
                    "heading4",
                    "h4",
                    "überschrift 4",
                    "titre 4",
                    "заголовок 4",
                ],
            ),
            (
                5,
                &["heading 5", "heading5", "h5", "überschrift 5", "titre 5"],
            ),
            (6, &["heading 6", "heading6", "h6", "überschrift 6"]),
            (7, &["heading 7", "heading7", "h7"]),
            (8, &["heading 8", "heading8", "h8"]),
            (9, &["heading 9", "heading9", "h9"]),
        ]
    })
}

const TITLE_PATTERNS: &[&str] = &["title", "documenttitle", "thetitle", "doc title"];

const BODY_PATTERNS: &[&str] = &[
    "normal",
    "standard",
    "body text",
    "bodytext",
    "fließtext",
    "texte de corps",
    "corpo del testo",
    "cuerpo de texto",
    "основной текст",
    "no spacing",
    "default paragraph style",
    "tekst podstawowy",
];

const FOOTNOTE_PATTERNS: &[&str] = &[
    "footnote text",
    "fußnotentext",
    "note de bas de page",
    "nota a piè di pagina",
    "nota al pie",
    "сноска",
    "footnote",
    "footnotetext",
];

const CAPTION_PATTERNS: &[&str] = &[
    "caption",
    "bildunterschrift",
    "légende",
    "didascalia",
    "leyenda",
];

const BLOCKQUOTE_PATTERNS: &[&str] = &[
    "block text",
    "blockquote",
    "quote",
    "intense quote",
    "block quotation",
    "zitat",
    "citation",
    "citazione",
    "bloque de texto",
];

const ABSTRACT_PATTERNS: &[&str] = &["abstract", "zusammenfassung", "résumé", "riassunto"];

/// Heading regex — port of `_HEADING_KW_RE`.
///
/// Matches one of the keywords (English, German, French, Italian, Spanish,
/// Russian, Dutch, Swedish, Polish) followed by optional separators and
/// a 1-digit level. We hand-roll the matching because the keyword set is
/// fixed; this keeps us regex-engine-free in the core.
fn match_heading_kw_re(name_lo: &str) -> Option<u8> {
    static KEYWORDS: &[&str] = &[
        "heading",
        "ueberschrift",
        "überschrift",
        "titre",
        "titolo",
        "encabezado",
        "заголовок",
        "kop",
        "rubrik",
        "nagłówek",
    ];
    for kw in KEYWORDS {
        if let Some(pos) = find_substr(name_lo, kw) {
            let after = &name_lo[pos + kw.len()..];
            // Optional separators: whitespace, _, -
            let mut tail = after.trim_start_matches([' ', '\t', '_', '-']);
            // Optional leading zeros.
            tail = tail.trim_start_matches('0');
            if let Some(first) = tail.chars().next() {
                if first.is_ascii_digit() {
                    let level = (first as u8) - b'0';
                    if (1..=9).contains(&level) {
                        return Some(level);
                    }
                }
            }
        }
    }
    None
}

fn find_substr(hay: &str, needle: &str) -> Option<usize> {
    hay.find(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_of(name: &str) -> (String, u8) {
        let c = classify_style(name);
        (c.class.as_str(), c.heading_level)
    }

    #[test]
    fn english_heading_levels() {
        assert_eq!(class_of("Heading 1"), ("heading1".into(), 1));
        assert_eq!(class_of("Heading 2"), ("heading2".into(), 2));
        assert_eq!(class_of("heading 9"), ("heading9".into(), 9));
        assert_eq!(class_of("H1"), ("heading1".into(), 1));
        assert_eq!(class_of("Heading 1 Char"), ("heading1".into(), 1));
    }

    #[test]
    fn multilingual_headings() {
        assert_eq!(class_of("Überschrift 1"), ("heading1".into(), 1));
        assert_eq!(class_of("Titre 2"), ("heading2".into(), 2));
        assert_eq!(class_of("заголовок 3"), ("heading3".into(), 3));
        assert_eq!(class_of("Kop 1"), ("heading1".into(), 1));
    }

    #[test]
    fn heading_regex_with_separators_and_zeros() {
        assert_eq!(class_of("Heading_02"), ("heading2".into(), 2));
        assert_eq!(class_of("Ueberschrift_01"), ("heading1".into(), 1));
        assert_eq!(class_of("Titolo3"), ("heading3".into(), 3));
        assert_eq!(class_of("Titre2"), ("heading2".into(), 2));
    }

    #[test]
    fn title() {
        assert_eq!(class_of("Title"), ("title".into(), 0));
        assert_eq!(class_of("DocumentTitle"), ("title".into(), 0));
    }

    #[test]
    fn body_class() {
        assert_eq!(class_of("Normal"), ("body".into(), 0));
        assert_eq!(class_of("Body Text"), ("body".into(), 0));
        assert_eq!(class_of("Fließtext"), ("body".into(), 0));
    }

    #[test]
    fn footnote() {
        assert_eq!(class_of("Footnote Text"), ("footnote".into(), 0));
        assert_eq!(class_of("Fußnotentext"), ("footnote".into(), 0));
    }

    #[test]
    fn caption() {
        assert_eq!(class_of("Caption"), ("caption".into(), 0));
        assert_eq!(class_of("Bildunterschrift"), ("caption".into(), 0));
    }

    #[test]
    fn blockquote() {
        assert_eq!(class_of("Quote"), ("blockquote".into(), 0));
        assert_eq!(class_of("Intense Quote"), ("blockquote".into(), 0));
    }

    #[test]
    fn abstract_class() {
        assert_eq!(class_of("Abstract"), ("abstract".into(), 0));
        assert_eq!(class_of("Zusammenfassung"), ("abstract".into(), 0));
    }

    #[test]
    fn unknown() {
        assert_eq!(class_of("MyRandomStyle"), ("unknown".into(), 0));
        assert_eq!(class_of("RuntimeBlah"), ("unknown".into(), 0));
    }
}
