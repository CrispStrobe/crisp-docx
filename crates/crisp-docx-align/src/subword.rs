//! Group subword tokens back into surface words.
//!
//! Aligners operate at subword granularity (because the encoder does),
//! but for downstream consumers like a format-preserving translator the
//! word-level alignment is what matters. Both supported tokenizer
//! families carry a marker convention we can rely on:
//!
//!   - **WordPiece** (BERT, distilBERT, granite, e5, MiniLM-uncased):
//!     subword *continuations* are prefixed `##`. The first token of a
//!     word has no `##`.
//!   - **SentencePiece-Unigram** (XLM-R, multilingual-MiniLM, e5
//!     multilingual): *word starts* are prefixed `▁` (U+2581). Subword
//!     continuations have no prefix.
//!
//! Special tokens (`[CLS]`, `[SEP]`, `<s>`, `</s>`, `<pad>`, `[PAD]`,
//! `[UNK]`, `<unk>`, `<mask>`) are excluded from word groups — they
//! carry no surface meaning.

/// Tokenizer family. Mirrors the kinds reported by CrispEmbed's
/// `crispembed_tokenizer_kind` C API: `1=WordPiece`, `2=SentencePiece`,
/// `3=BPE`, `0=unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizerKind {
    /// WordPiece (BERT-style). Continuations are `##` prefixed.
    WordPiece,
    /// SentencePiece-Unigram (XLM-R / mT5 / mBART). Word starts are
    /// `▁` (U+2581) prefixed.
    SentencePiece,
    /// Byte-pair encoding (Qwen / Gemma / Llama). Subword grouping
    /// requires per-tokenizer logic we don't implement — treated as
    /// every token = one word.
    Bpe,
}

impl TokenizerKind {
    /// Convert from the `crispembed_tokenizer_kind` integer. Defaults to
    /// WordPiece for unknown values (the most permissive choice — every
    /// token is treated as a word start unless `##`-prefixed).
    pub fn from_kind(k: i32) -> Self {
        match k {
            1 => TokenizerKind::WordPiece,
            2 => TokenizerKind::SentencePiece,
            3 => TokenizerKind::Bpe,
            _ => TokenizerKind::WordPiece,
        }
    }
}

/// A surface word built from one or more contiguous subword tokens.
#[derive(Debug, Clone)]
pub struct WordGroup {
    /// Indices into the original token vector that belong to this word.
    pub token_indices: Vec<usize>,
    /// Reconstructed surface text (e.g., `"schläft"` from `▁sch + lä + ft`).
    pub text: String,
}

const SP_WORD_START: char = '\u{2581}'; // ▁

fn is_special(tok: &str) -> bool {
    matches!(
        tok,
        "[CLS]"
            | "[SEP]"
            | "[PAD]"
            | "[UNK]"
            | "[MASK]"
            | "<s>"
            | "</s>"
            | "<pad>"
            | "<unk>"
            | "<mask>"
    )
}

/// Group `tokens` (in order) into surface words. Returns one [`WordGroup`]
/// per word, with `token_indices` referring back to positions in the
/// input slice. Special tokens (`[CLS]`, `<s>`, etc.) are excluded —
/// callers who want to keep them as a boundary marker can filter the
/// result themselves.
pub fn group_subwords_to_words(tokens: &[String], kind: TokenizerKind) -> Vec<WordGroup> {
    let mut groups: Vec<WordGroup> = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        if is_special(tok) {
            continue;
        }
        match kind {
            TokenizerKind::WordPiece => {
                if let Some(continuation) = tok.strip_prefix("##") {
                    match groups.last_mut() {
                        Some(g) => {
                            g.token_indices.push(i);
                            g.text.push_str(continuation);
                        }
                        None => groups.push(WordGroup {
                            token_indices: vec![i],
                            text: continuation.to_string(),
                        }),
                    }
                } else {
                    groups.push(WordGroup {
                        token_indices: vec![i],
                        text: tok.clone(),
                    });
                }
            }
            TokenizerKind::SentencePiece => {
                if let Some(stripped) = tok.strip_prefix(SP_WORD_START) {
                    groups.push(WordGroup {
                        token_indices: vec![i],
                        text: stripped.to_string(),
                    });
                } else {
                    // No ▁ prefix → continuation of the preceding word.
                    match groups.last_mut() {
                        Some(g) => {
                            g.token_indices.push(i);
                            g.text.push_str(tok);
                        }
                        None => groups.push(WordGroup {
                            token_indices: vec![i],
                            text: tok.clone(),
                        }),
                    }
                }
            }
            TokenizerKind::Bpe => {
                // No subword grouping for BPE — treat every token as its
                // own word. Callers wanting smarter behaviour can do
                // tokenizer-specific post-processing.
                groups.push(WordGroup {
                    token_indices: vec![i],
                    text: tok.clone(),
                });
            }
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wordpiece_groups_continuations() {
        // BERT tokenization: "embeddings" → "em" "##bed" "##ding" "##s"
        let toks = s(&["[CLS]", "em", "##bed", "##ding", "##s", "[SEP]"]);
        let groups = group_subwords_to_words(&toks, TokenizerKind::WordPiece);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].text, "embeddings");
        assert_eq!(groups[0].token_indices, vec![1, 2, 3, 4]);
    }

    #[test]
    fn sentencepiece_groups_via_word_start_marker() {
        // XLM-R tokenization: "schläft" → "▁sch" "lä" "ft"
        let toks = s(&[
            "<s>",
            "\u{2581}Der",
            "\u{2581}Hund",
            "\u{2581}sch",
            "lä",
            "ft",
            "</s>",
        ]);
        let groups = group_subwords_to_words(&toks, TokenizerKind::SentencePiece);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].text, "Der");
        assert_eq!(groups[1].text, "Hund");
        assert_eq!(groups[2].text, "schläft");
        assert_eq!(groups[2].token_indices, vec![3, 4, 5]);
    }

    #[test]
    fn special_tokens_are_dropped() {
        let toks = s(&["[CLS]", "hi", "[SEP]"]);
        let groups = group_subwords_to_words(&toks, TokenizerKind::WordPiece);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].text, "hi");
        assert_eq!(groups[0].token_indices, vec![1]);
    }

    #[test]
    fn bpe_groups_one_per_token() {
        let toks = s(&["Ġhello", "Ġworld"]);
        let groups = group_subwords_to_words(&toks, TokenizerKind::Bpe);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].text, "Ġhello");
        assert_eq!(groups[1].text, "Ġworld");
    }

    #[test]
    fn unknown_kind_defaults_to_wordpiece_rules() {
        assert_eq!(TokenizerKind::from_kind(0), TokenizerKind::WordPiece);
        assert_eq!(TokenizerKind::from_kind(7), TokenizerKind::WordPiece);
    }
}
