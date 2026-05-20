//! Re-distribute source-run formatting onto translated text via word
//! alignment.
//!
//! This is the bridge between [`crate::embed::align_texts`] and a
//! caller's run-level paragraph data model. Given:
//!
//! - **Source runs**: ordered `(text, format_id)` pairs (where
//!   `format_id` is an opaque tag for the run's formatting — could
//!   be `Option<Vec<u8>>` for raw OOXML `<w:rPr>` bytes, or an enum,
//!   or anything else the caller wants to group on).
//! - **Source text**: the full paragraph text in the source language.
//!   Must equal `source_runs.iter().map(|r| r.text).join("")`.
//! - **Target text**: the translation produced externally (e.g. by an
//!   LLM). Different word order, different lengths.
//! - **Word edges**: source-word-idx → target-word-idx pairs from
//!   [`crate::embed::align_texts`].
//!
//! The function emits **target runs**: a `Vec<TargetRun>` covering the
//! whole target text exactly once. Each target run carries the
//! `format_id` of whichever source run dominates its character range.
//! Adjacent target runs with the same `format_id` are merged.
//!
//! ## Algorithm sketch
//!
//! 1. Walk the source text by character, tagging each character with
//!    its source-run-index (uses the order of `source_runs`).
//! 2. Tokenise source + target via the aligner — the aligner already
//!    splits into surface words. Use the **word boundaries** as the
//!    natural alignment unit; sub-word resolution adds complexity for
//!    little gain on typical European prose.
//! 3. For each source word index `i`, look up its char range in the
//!    source text, then the source-run-index that covers the majority
//!    of those chars → that's the word's `format_id`.
//! 4. For each target word, find any source word that aligns to it
//!    (via `word_edges`). Pick that source word's `format_id`. If no
//!    edge exists, inherit from the nearest aligned neighbour
//!    (left-then-right scan). If still nothing, use `default_format_id`.
//! 5. Walk the target text, splitting on word boundaries, and emit one
//!    `TargetRun` per maximal run of same-`format_id` words plus the
//!    inter-word whitespace.
//!
//! ## Limitations
//!
//! - Word boundaries are detected as ASCII whitespace runs. For
//!   languages that don't separate words by spaces (Chinese, Japanese,
//!   Thai) the aligner already breaks on subword units, but this
//!   coarsening step degrades — every "word" becomes one character.
//!   Treat output as best-effort there.
//! - Inline footnote anchors are NOT redistributed here — the caller
//!   is expected to thread them through via a separate channel (the
//!   `crisp-docx-core::Run::footnote_refs` field), placing them at
//!   the matching source-word-aligned target word.

#[cfg(feature = "crispembed")]
use crate::{algorithm::Strategy, embed::align_texts};

/// One input run: text plus an opaque format identifier the caller can
/// use to look up the run's `rPr` bytes (or anything else).
#[derive(Debug, Clone)]
pub struct SourceRun<F> {
    /// The literal text of this run.
    pub text: String,
    /// Opaque format tag — clone- and equality-comparable.
    pub format_id: F,
}

/// One output run, ready to be emitted on the target side.
#[derive(Debug, Clone)]
pub struct TargetRun<F> {
    /// Slice of the target text this run covers.
    pub text: String,
    /// Carried over from the source side via the alignment.
    pub format_id: F,
}

/// Compute target runs from `source_runs` and a `translated_text` using
/// the provided `word_edges`. `word_edges` are `(src_word_idx,
/// tgt_word_idx)` pairs as returned by
/// [`crate::embed::align_texts`]'s `word_edges` field.
///
/// `default_format` is used for target words that have no aligned
/// source word (e.g. inserted determiners). When `None`, gaps fall
/// through to the nearest neighbour's format.
///
/// Returns target runs in document order, partitioning the translated
/// text exactly once.
pub fn transfer_format_via_words<F>(
    source_runs: &[SourceRun<F>],
    translated_text: &str,
    word_edges: &[(usize, usize)],
    default_format: Option<F>,
) -> Vec<TargetRun<F>>
where
    F: Clone + PartialEq,
{
    // 1. Source text + per-char run index.
    let mut src_text = String::new();
    let mut char_run: Vec<usize> = Vec::new();
    for (i, r) in source_runs.iter().enumerate() {
        for _ in r.text.chars() {
            char_run.push(i);
        }
        src_text.push_str(&r.text);
    }

    // 2. Word boundaries on both sides (whitespace-delimited).
    let src_words = words(&src_text);
    let tgt_words = words(translated_text);

    // 3. For each source word, pick the most-common run index across
    //    its character span. The character spans we have are over the
    //    source CHARACTER buffer; src_words returns char indices.
    let src_word_format: Vec<Option<F>> = src_words
        .iter()
        .map(|w| {
            let mut counts: Vec<(usize, usize)> = Vec::new();
            for c in w.start..w.end {
                let r = char_run.get(c).copied().unwrap_or(0);
                if let Some(slot) = counts.iter_mut().find(|x| x.0 == r) {
                    slot.1 += 1;
                } else {
                    counts.push((r, 1));
                }
            }
            counts
                .into_iter()
                .max_by_key(|x| x.1)
                .and_then(|(idx, _)| source_runs.get(idx))
                .map(|r| r.format_id.clone())
        })
        .collect();

    // 4. tgt_word -> Vec<src_word> via word_edges.
    let mut tgt_to_srcs: Vec<Vec<usize>> = vec![Vec::new(); tgt_words.len()];
    for (s, t) in word_edges {
        if *t < tgt_to_srcs.len() {
            tgt_to_srcs[*t].push(*s);
        }
    }

    // 5. Per target word, pick a format_id. Direct edge → pick the
    //    first source word's format. No edge → scan outward for a
    //    neighbour that has one.
    let tgt_word_format: Vec<Option<F>> = (0..tgt_words.len())
        .map(|j| {
            if let Some(&s_idx) = tgt_to_srcs[j].first() {
                return src_word_format.get(s_idx).cloned().flatten();
            }
            // scan left
            for k in (0..j).rev() {
                if let Some(&s_idx) = tgt_to_srcs[k].first() {
                    if let Some(Some(f)) = src_word_format.get(s_idx).cloned() {
                        return Some(f);
                    }
                }
            }
            // scan right
            for (_k, list) in tgt_to_srcs.iter().enumerate().skip(j + 1) {
                if let Some(&s_idx) = list.first() {
                    if let Some(Some(f)) = src_word_format.get(s_idx).cloned() {
                        return Some(f);
                    }
                }
            }
            default_format.clone()
        })
        .collect();

    // 6. Walk the translated text and emit target runs. We need to
    //    emit not just the words but ALSO the whitespace gaps between
    //    them. Assign each gap the format of the preceding word.
    let bytes = translated_text.as_bytes();
    let mut out: Vec<TargetRun<F>> = Vec::new();
    let mut cursor = 0usize;
    for (j, w) in tgt_words.iter().enumerate() {
        // leading whitespace (or any text before this word) goes with
        // the previous run's format if present, otherwise with this
        // word's format.
        if w.start > cursor {
            let prev_fmt = out
                .last()
                .map(|r| r.format_id.clone())
                .or_else(|| tgt_word_format.get(j).cloned().flatten());
            let chunk = &translated_text[byte_pos(bytes, cursor)..byte_pos(bytes, w.start)];
            if !chunk.is_empty() {
                if let Some(f) = prev_fmt {
                    push_or_merge(&mut out, chunk, f);
                }
            }
        }
        let chunk = &translated_text[byte_pos(bytes, w.start)..byte_pos(bytes, w.end)];
        if let Some(f) = tgt_word_format[j].clone() {
            push_or_merge(&mut out, chunk, f);
        } else if let Some(prev) = out.last() {
            let p = prev.format_id.clone();
            push_or_merge(&mut out, chunk, p);
        } else if let Some(def) = default_format.clone() {
            push_or_merge(&mut out, chunk, def);
        }
        cursor = w.end;
    }
    // Trailing whitespace / characters after the last word.
    let total_chars = translated_text.chars().count();
    if cursor < total_chars {
        let chunk = &translated_text[byte_pos(bytes, cursor)..];
        let fmt = out
            .last()
            .map(|r| r.format_id.clone())
            .or_else(|| default_format.clone());
        if let Some(f) = fmt {
            push_or_merge(&mut out, chunk, f);
        }
    }

    out
}

fn push_or_merge<F: PartialEq + Clone>(out: &mut Vec<TargetRun<F>>, chunk: &str, fmt: F) {
    if let Some(last) = out.last_mut() {
        if last.format_id == fmt {
            last.text.push_str(chunk);
            return;
        }
    }
    out.push(TargetRun {
        text: chunk.to_string(),
        format_id: fmt,
    });
}

#[derive(Debug, Clone, Copy)]
struct WordSpan {
    /// Inclusive char index.
    start: usize,
    /// Exclusive char index.
    end: usize,
}

/// Whitespace-delimited word spans, in character indices.
fn words(s: &str) -> Vec<WordSpan> {
    let mut out: Vec<WordSpan> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, c) in s.chars().enumerate() {
        if c.is_whitespace() {
            if let Some(st) = start.take() {
                out.push(WordSpan { start: st, end: i });
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    let len = s.chars().count();
    if let Some(st) = start {
        out.push(WordSpan {
            start: st,
            end: len,
        });
    }
    out
}

/// Convert a character index into a byte offset for safe slicing.
fn byte_pos(bytes: &[u8], char_idx: usize) -> usize {
    let s = std::str::from_utf8(bytes).unwrap_or("");
    for (count, (b, _)) in s.char_indices().enumerate() {
        if count == char_idx {
            return b;
        }
    }
    s.len()
}

/// End-to-end convenience wrapper: align two texts with `model` and
/// transfer the source-side runs' formatting onto the translation.
///
/// Returns the runs ready to be written back, plus the underlying
/// alignment so callers can introspect it (e.g. place footnote refs).
///
/// Only available when the `crispembed` feature is enabled.
#[cfg(feature = "crispembed")]
pub fn translate_runs<F>(
    model: &mut crispembed::CrispEmbed,
    source_runs: &[SourceRun<F>],
    translated_text: &str,
    strategy: Strategy,
    default_format: Option<F>,
) -> Result<TranslatedRuns<F>, crate::Error>
where
    F: Clone + PartialEq,
{
    let src_text: String = source_runs.iter().map(|r| r.text.as_str()).collect();
    let alignment = align_texts(model, &src_text, translated_text, strategy)?;
    let runs = transfer_format_via_words(
        source_runs,
        translated_text,
        &alignment.word_edges,
        default_format,
    );
    Ok(TranslatedRuns {
        runs,
        word_edges: alignment.word_edges,
        src_words: alignment.src_words.iter().map(|w| w.text.clone()).collect(),
        tgt_words: alignment.tgt_words.iter().map(|w| w.text.clone()).collect(),
    })
}

/// Bundled result of [`translate_runs`].
#[cfg(feature = "crispembed")]
#[derive(Debug)]
pub struct TranslatedRuns<F> {
    /// Final target runs, ready to write back.
    pub runs: Vec<TargetRun<F>>,
    /// Underlying word-level alignment edges (src_word_idx,
    /// tgt_word_idx), useful for placing footnote refs.
    pub word_edges: Vec<(usize, usize)>,
    /// Surface words on the source side, in order.
    pub src_words: Vec<String>,
    /// Surface words on the target side, in order.
    pub tgt_words: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_run_passes_through_unchanged() {
        let src = vec![SourceRun {
            text: "Hello world.".to_string(),
            format_id: "plain",
        }];
        let edges = vec![(0, 0), (1, 1)];
        let out = transfer_format_via_words(&src, "Hallo Welt.", &edges, Some("plain"));
        // All target runs should be "plain" — concat == "Hallo Welt."
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Hallo Welt.");
        assert_eq!(out[0].format_id, "plain");
    }

    #[test]
    fn bold_word_in_middle_carries_through() {
        let src = vec![
            SourceRun {
                text: "I love ".into(),
                format_id: "plain",
            },
            SourceRun {
                text: "dogs".into(),
                format_id: "bold",
            },
            SourceRun {
                text: " a lot.".into(),
                format_id: "plain",
            },
        ];
        // English source words: I, love, dogs, a, lot.
        // German translation:    Ich, liebe, Hunde, sehr.
        // Alignment:             I↔Ich, love↔liebe, dogs↔Hunde, lot↔sehr
        let edges = vec![(0, 0), (1, 1), (2, 2), (4, 3)];
        let out = transfer_format_via_words(&src, "Ich liebe Hunde sehr.", &edges, Some("plain"));
        // We expect: "Ich liebe " plain, "Hunde" bold, " sehr." plain
        let concat: String = out.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(concat, "Ich liebe Hunde sehr.");
        // The "Hunde" word should be in a bold run.
        let bold_runs: Vec<&str> = out
            .iter()
            .filter(|r| r.format_id == "bold")
            .map(|r| r.text.as_str())
            .collect();
        assert!(
            bold_runs.iter().any(|s| s.contains("Hunde")),
            "no bold run contains Hunde: runs={:?}",
            out.iter()
                .map(|r| (&r.text, r.format_id))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn unaligned_words_inherit_from_neighbour() {
        let src = vec![
            SourceRun {
                text: "bold".into(),
                format_id: "bold",
            },
            SourceRun {
                text: " plain".into(),
                format_id: "plain",
            },
        ];
        // src words: bold(0), plain(1)
        // tgt words: foo(0), bar(1), baz(2)  — only middle aligned to plain
        let edges = vec![(1, 1)];
        let out = transfer_format_via_words(&src, "foo bar baz", &edges, None);
        // foo: no edge → scan finds bar→plain → foo gets plain.
        // bar: edge → plain.
        // baz: no edge → scan left finds bar→plain → baz gets plain.
        let concat: String = out.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(concat, "foo bar baz");
        for r in &out {
            assert_eq!(r.format_id, "plain");
        }
    }

    #[test]
    fn default_format_used_when_no_alignment_at_all() {
        let src = vec![SourceRun {
            text: "bold".into(),
            format_id: "bold",
        }];
        let edges: Vec<(usize, usize)> = vec![];
        let out = transfer_format_via_words(&src, "Worte hier", &edges, Some("default"));
        for r in &out {
            assert_eq!(r.format_id, "default");
        }
    }

    #[test]
    fn adjacent_same_format_runs_merge() {
        let src = vec![
            SourceRun {
                text: "a ".into(),
                format_id: 0u8,
            },
            SourceRun {
                text: "b ".into(),
                format_id: 0u8,
            },
            SourceRun {
                text: "c".into(),
                format_id: 0u8,
            },
        ];
        let edges = vec![(0, 0), (1, 1), (2, 2)];
        let out = transfer_format_via_words(&src, "x y z", &edges, Some(0u8));
        // All same format → one merged run.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "x y z");
        assert_eq!(out[0].format_id, 0);
    }

    #[test]
    fn whitespace_keeps_format_of_preceding_word() {
        let src = vec![
            SourceRun {
                text: "Hi ".into(),
                format_id: "plain",
            },
            SourceRun {
                text: "BOLD".into(),
                format_id: "bold",
            },
        ];
        // src: Hi(0), BOLD(1)
        // tgt: Hallo(0), FETT(1)
        // edges: (0,0),(1,1)
        let out = transfer_format_via_words(&src, "Hallo FETT", &[(0, 0), (1, 1)], Some("plain"));
        let concat: String = out.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(concat, "Hallo FETT");
        // The space between Hallo and FETT should attach to "Hallo" (the
        // preceding run, which is "plain") — keeping "FETT" as its own
        // bold run.
        // So expected: ["Hallo ", "FETT"] with formats [plain, bold].
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].text, "Hallo ");
        assert_eq!(out[0].format_id, "plain");
        assert_eq!(out[1].text, "FETT");
        assert_eq!(out[1].format_id, "bold");
    }
}
