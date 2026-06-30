//! Unify the quotation marks in a docx to a single national style.
//!
//! Word documents assembled from mixed sources (pandoc, copy-paste, Word
//! autocorrect with the wrong UI language) end up with a salad of quote
//! glyphs: English `“…”`, German `„…“`, French `«…»`, straight `"…"`. This
//! module rewrites every quotation mark in the body text to one chosen
//! [`QuoteStyle`], leaving apostrophes alone.
//!
//! ## How open vs. close is decided
//!
//! The *glyph* can't be trusted across styles — `“` (U+201C) is an English
//! *opening* but a German *closing* quote. So role is inferred from context
//! instead: a quote is **opening** if what precedes it is the start of the
//! run-stream, whitespace, or an opening delimiter; otherwise it is
//! **closing**. Double vs. single is taken from the glyph (reliable), and
//! the decision state is threaded across `<w:t>` runs so a quote split from
//! its preceding word by a run boundary still resolves correctly.
//!
//! ## Apostrophes
//!
//! A single quote flanked by letters (`l'homme`, `don't`) or trailing a word
//! (`Cusanus'`) is treated as an apostrophe and rendered `’` (U+2019),
//! regardless of style. Single *quotation* marks are converted too unless
//! [`QuoteOptions::singles`] is false.
//!
//! ## French / Swiss spacing
//!
//! French guillemets carry inner spacing (`« mot »`); the Swiss convention
//! omits it (`«mot»`). Converting *to* French inserts a narrow no-break
//! space (U+202F); converting *away from* it (or to Swiss) strips any inner
//! space — narrow, no-break, or plain — adjacent to a guillemet.

use std::io::Cursor;

use quick_xml::events::{BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;

use crate::error::{Error, Result};
use crate::ns::{PART_DOCUMENT, PART_ENDNOTES, PART_FOOTNOTES};
use crate::package::Package;

/// National quotation-mark convention to normalize to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
    /// `„…“` / `‚…‘` — Germany & Austria.
    German,
    /// `“…”` / `‘…’` — English.
    English,
    /// `« … »` / `‹ … ›` with narrow no-break inner spacing — France.
    French,
    /// `«…»` / `‹…›` without inner spacing — Swiss usage (FR & DE).
    Swiss,
}

/// Knobs for [`normalize_quotes`].
#[derive(Debug, Clone, Copy)]
pub struct QuoteOptions {
    /// Also convert single *quotation* marks (apostrophes are always kept).
    pub singles: bool,
}

impl Default for QuoteOptions {
    fn default() -> Self {
        Self { singles: true }
    }
}

const NARROW_NBSP: char = '\u{202F}';
const NBSP: char = '\u{00A0}';
const APOSTROPHE: char = '\u{2019}';

impl QuoteStyle {
    fn double_open(self) -> &'static str {
        match self {
            QuoteStyle::German => "\u{201E}",         // „
            QuoteStyle::English => "\u{201C}",        // “
            QuoteStyle::French => "\u{00AB}\u{202F}", // «␠
            QuoteStyle::Swiss => "\u{00AB}",          // «
        }
    }
    fn double_close(self) -> &'static str {
        match self {
            QuoteStyle::German => "\u{201C}",         // “
            QuoteStyle::English => "\u{201D}",        // ”
            QuoteStyle::French => "\u{202F}\u{00BB}", // ␠»
            QuoteStyle::Swiss => "\u{00BB}",          // »
        }
    }
    fn single_open(self) -> &'static str {
        match self {
            QuoteStyle::German => "\u{201A}",         // ‚
            QuoteStyle::English => "\u{2018}",        // ‘
            QuoteStyle::French => "\u{2039}\u{202F}", // ‹␠
            QuoteStyle::Swiss => "\u{2039}",          // ‹
        }
    }
    fn single_close(self) -> &'static str {
        match self {
            QuoteStyle::German => "\u{2018}",         // ‘
            QuoteStyle::English => "\u{2019}",        // ’
            QuoteStyle::French => "\u{202F}\u{203A}", // ␠›
            QuoteStyle::Swiss => "\u{203A}",          // ›
        }
    }
}

fn is_double(c: char) -> bool {
    matches!(
        c,
        '"' | '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{00AB}' | '\u{00BB}'
    )
}

fn is_single(c: char) -> bool {
    matches!(
        c,
        '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2039}' | '\u{203A}'
    )
}

fn opens_after(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(c) => {
            c.is_whitespace()
                || matches!(
                    c,
                    '(' | '['
                        | '{'
                        | '\u{00AB}'
                        | '\u{2039}'
                        | '\u{201E}'
                        | '\u{201A}'
                        | '\u{201C}'
                        | '\u{2018}'
                        | '\u{2013}'
                        | '\u{2014}'
                        | '-'
                        | '/'
                        | ':'
                )
        }
    }
}

/// Streaming normalizer that threads open/close state across text fragments
/// (i.e. across `<w:t>` runs within one part).
#[derive(Debug)]
struct QuoteNormalizer {
    style: QuoteStyle,
    opts: QuoteOptions,
    prev: Option<char>,
    in_single: bool,
    changes: usize,
}

impl QuoteNormalizer {
    fn new(style: QuoteStyle, opts: QuoteOptions) -> Self {
        Self {
            style,
            opts,
            prev: None,
            in_single: false,
            changes: 0,
        }
    }

    /// Drop inner spacing (narrow/no-break/plain) that sits immediately
    /// inside a guillemet, so the target style can re-apply its own.
    fn strip_guillemet_spacing(chars: &[char]) -> Vec<char> {
        let mut out = Vec::with_capacity(chars.len());
        for (i, &c) in chars.iter().enumerate() {
            if matches!(c, ' ' | NBSP | NARROW_NBSP) {
                let after_open = matches!(
                    chars.get(i.wrapping_sub(1)),
                    Some('\u{00AB}') | Some('\u{2039}')
                );
                let before_close = matches!(chars.get(i + 1), Some('\u{00BB}') | Some('\u{203A}'));
                if after_open || before_close {
                    continue;
                }
            }
            out.push(c);
        }
        out
    }

    fn push(&mut self, text: &str) -> String {
        let chars = Self::strip_guillemet_spacing(&text.chars().collect::<Vec<_>>());
        let mut out = String::with_capacity(text.len() + 8);
        for i in 0..chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if is_double(c) {
                let emit = if opens_after(self.prev) {
                    self.style.double_open()
                } else {
                    self.style.double_close()
                };
                out.push_str(emit);
                self.changes += 1;
            } else if is_single(c) {
                self.push_single(c, next, &mut out);
            } else {
                out.push(c);
            }
            self.prev = Some(c);
        }
        out
    }

    fn push_single(&mut self, c: char, next: Option<char>, out: &mut String) {
        let prev_alnum = self.prev.is_some_and(|p| p.is_alphanumeric());
        let next_alnum = next.is_some_and(|q| q.is_alphanumeric());

        // Intra-word → apostrophe (l'homme, don't), never a quote boundary.
        if prev_alnum && next_alnum {
            out.push(APOSTROPHE);
            return;
        }
        if !self.opts.singles {
            out.push(c);
            return;
        }
        if self.in_single && !next_alnum {
            out.push_str(self.style.single_close());
            self.in_single = false;
            self.changes += 1;
        } else if !self.in_single && opens_after(self.prev) && next_alnum {
            out.push_str(self.style.single_open());
            self.in_single = true;
            self.changes += 1;
        } else if prev_alnum {
            // Trailing apostrophe (Cusanus’, Maximus’).
            out.push(APOSTROPHE);
        } else if self.in_single {
            out.push_str(self.style.single_close());
            self.in_single = false;
            self.changes += 1;
        } else {
            out.push_str(self.style.single_open());
            self.in_single = true;
            self.changes += 1;
        }
    }
}

/// Normalize the quotation marks in a single string. Convenience wrapper for
/// callers outside the docx machinery (and the test-bed for the rules).
pub fn normalize_quotes(text: &str, style: QuoteStyle, opts: QuoteOptions) -> String {
    QuoteNormalizer::new(style, opts).push(text)
}

/// Report from [`normalize_quotes_in_package`].
#[derive(Debug, Clone, Default)]
pub struct QuoteReport {
    /// Quote marks rewritten (opening/closing; apostrophes excluded).
    pub changed: usize,
    /// Parts that were rewritten (`document.xml`, `footnotes.xml`, …).
    pub parts: Vec<String>,
}

/// Rewrite the quotation marks across every text-bearing part of `pkg`
/// (`word/document.xml`, `word/footnotes.xml`, `word/endnotes.xml`).
pub fn normalize_quotes_in_package(
    pkg: &mut Package,
    style: QuoteStyle,
    opts: QuoteOptions,
) -> Result<QuoteReport> {
    let mut report = QuoteReport::default();
    for part in [PART_DOCUMENT, PART_FOOTNOTES, PART_ENDNOTES] {
        let Some(bytes) = pkg.get_part(part).map(|b| b.to_vec()) else {
            continue;
        };
        let mut norm = QuoteNormalizer::new(style, opts);
        let new_bytes = normalize_part(&bytes, &mut norm, part)?;
        if norm.changes > 0 {
            pkg.set_part(part, new_bytes);
            report.changed += norm.changes;
            report.parts.push(part.to_string());
        }
    }
    Ok(report)
}

/// Walk one part's XML and transform the text inside every `<w:t>` element.
fn normalize_part(input: &[u8], norm: &mut QuoteNormalizer, part: &str) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(false);
    reader.config_mut().expand_empty_elements = false;
    let mut writer = Writer::new(Cursor::new(Vec::with_capacity(input.len())));
    let mut buf = Vec::with_capacity(1024);
    let mut in_t = false;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::XmlParse {
                part: part.into(),
                source: e,
            })?;
        match event {
            Event::Eof => break,
            Event::Start(s) if s.name().as_ref() == b"w:t" => {
                in_t = true;
                writer
                    .write_event(Event::Start(s))
                    .map_err(|e| xml_io(e, part))?;
            }
            Event::End(e) if e.name().as_ref() == b"w:t" => {
                in_t = false;
                writer
                    .write_event(Event::End(e))
                    .map_err(|e| xml_io(e, part))?;
            }
            Event::Text(t) if in_t => {
                let raw = t.unescape().map_err(|e| xml_io(e, part))?.into_owned();
                let replaced = norm.push(&raw);
                let escaped = xml_escape_text(&replaced);
                writer
                    .write_event(Event::Text(BytesText::from_escaped(escaped)))
                    .map_err(|e| xml_io(e, part))?;
            }
            other => writer.write_event(other).map_err(|e| xml_io(e, part))?,
        }
        buf.clear();
    }
    Ok(writer.into_inner().into_inner())
}

fn xml_escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn xml_io(err: quick_xml::Error, part: &str) -> Error {
    Error::XmlParse {
        part: part.into(),
        source: err,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn de(s: &str) -> String {
        normalize_quotes(s, QuoteStyle::German, QuoteOptions::default())
    }

    #[test]
    fn straight_doubles_to_german() {
        assert_eq!(de(r#"Das "Wort" hier."#), "Das \u{201E}Wort\u{201C} hier.");
    }

    #[test]
    fn english_curly_to_german() {
        assert_eq!(
            de("Ein \u{201C}Zitat\u{201D}."),
            "Ein \u{201E}Zitat\u{201C}."
        );
    }

    #[test]
    fn german_to_english() {
        let s = normalize_quotes(
            "\u{201E}Zitat\u{201C}",
            QuoteStyle::English,
            QuoteOptions::default(),
        );
        assert_eq!(s, "\u{201C}Zitat\u{201D}");
    }

    #[test]
    fn to_french_adds_inner_narrow_space() {
        let s = normalize_quotes(r#""mot""#, QuoteStyle::French, QuoteOptions::default());
        assert_eq!(s, "\u{00AB}\u{202F}mot\u{202F}\u{00BB}");
    }

    #[test]
    fn french_to_swiss_strips_spacing() {
        let s = normalize_quotes(
            "\u{00AB}\u{202F}mot\u{202F}\u{00BB}",
            QuoteStyle::Swiss,
            QuoteOptions::default(),
        );
        assert_eq!(s, "\u{00AB}mot\u{00BB}");
    }

    #[test]
    fn apostrophe_is_preserved_not_a_quote() {
        // l'homme and Cusanus' both yield U+2019, no opening quote.
        assert_eq!(de("l'homme"), "l\u{2019}homme");
        assert_eq!(de("Cusanus' These"), "Cusanus\u{2019} These");
    }

    #[test]
    fn nested_single_inside_double() {
        let s = de(r#"Er sagte "ein 'Wort' nur"."#);
        assert_eq!(s, "Er sagte \u{201E}ein \u{201A}Wort\u{2018} nur\u{201C}.");
    }

    #[test]
    fn singles_can_be_skipped() {
        let opts = QuoteOptions { singles: false };
        let s = normalize_quotes("'a' \"b\"", QuoteStyle::German, opts);
        // single quotes left untouched, doubles converted
        assert_eq!(s, "'a' \u{201E}b\u{201C}");
    }

    #[test]
    fn close_at_fragment_start_uses_threaded_context() {
        // Simulate a run boundary: "Wort" split as ["Wort] + ["] — the
        // closing quote must still resolve as closing.
        let mut norm = QuoteNormalizer::new(QuoteStyle::German, QuoteOptions::default());
        let a = norm.push("\"Wort");
        let b = norm.push("\" Rest");
        assert_eq!(format!("{a}{b}"), "\u{201E}Wort\u{201C} Rest");
    }
}
