//! End-to-end text alignment: encode → similarity → extract → group.
//!
//! This module is only compiled when the `crispembed` feature is enabled.
//! It wires the algorithm layer to a real multilingual encoder so callers
//! can do `align_texts(&mut model, src, tgt, Strategy::Intersection)` in
//! one go.

use crispembed::CrispEmbed;

use crate::algorithm::{
    argmax_src_to_tgt, argmax_tgt_to_src, cosine_matrix, intersection, itermax, AlignmentEdge,
    Strategy,
};
use crate::errors::Error;
use crate::subword::{group_subwords_to_words, TokenizerKind, WordGroup};

/// The result of aligning two strings end-to-end. Keeps both the
/// token-level edges (subword positions) and the word-level grouping so
/// callers can either reason about subword alignment or aggregate to
/// surface-word alignment.
#[derive(Debug)]
pub struct TextAlignment {
    /// Source tokens as the encoder produced them (with subword markers).
    pub src_tokens: Vec<String>,
    /// Target tokens.
    pub tgt_tokens: Vec<String>,
    /// Source surface words (subwords merged).
    pub src_words: Vec<WordGroup>,
    /// Target surface words.
    pub tgt_words: Vec<WordGroup>,
    /// Token-level alignment edges per the chosen strategy.
    pub edges: Vec<AlignmentEdge>,
    /// Word-level edges: `(src_word_idx, tgt_word_idx)`. A pair appears
    /// here when at least one token edge connects a subword of the
    /// source word to a subword of the target word.
    pub word_edges: Vec<(usize, usize)>,
}

/// Align `src_text` and `tgt_text` using `model` as the encoder.
///
/// `model` must be a multilingual encoder — both languages must share
/// the same embedding space. Suitable models include
/// `paraphrase-multilingual-MiniLM-L12-v2`, `multilingual-e5-*`,
/// `granite-embedding-*`, `arctic-embed-l-v2`.
pub fn align_texts(
    model: &mut CrispEmbed,
    src_text: &str,
    tgt_text: &str,
    strategy: Strategy,
) -> Result<TextAlignment, Error> {
    let src_pairs = model.encode_tokens(src_text);
    if src_pairs.is_empty() {
        return Err(Error::Untokenizable("source text"));
    }
    let tgt_pairs = model.encode_tokens(tgt_text);
    if tgt_pairs.is_empty() {
        return Err(Error::Untokenizable("target text"));
    }

    let kind = TokenizerKind::from_kind(model.tokenizer_kind());

    let (src_tokens, src_vecs): (Vec<String>, Vec<Vec<f32>>) = src_pairs.into_iter().unzip();
    let (tgt_tokens, tgt_vecs): (Vec<String>, Vec<Vec<f32>>) = tgt_pairs.into_iter().unzip();

    let m = src_vecs.len();
    let n = tgt_vecs.len();
    let matrix = cosine_matrix(&src_vecs, &tgt_vecs)?;

    let edges = match strategy {
        Strategy::ArgmaxSrcToTgt => argmax_src_to_tgt(&matrix, m, n),
        Strategy::ArgmaxTgtToSrc => argmax_tgt_to_src(&matrix, m, n),
        Strategy::Intersection => intersection(&matrix, m, n),
        Strategy::Itermax { min_sim } => itermax(&matrix, m, n, min_sim),
    };

    let src_words = group_subwords_to_words(&src_tokens, kind);
    let tgt_words = group_subwords_to_words(&tgt_tokens, kind);

    // Token index → word index lookup tables.
    let mut src_tok_to_word = vec![usize::MAX; src_tokens.len()];
    for (wi, w) in src_words.iter().enumerate() {
        for &ti in &w.token_indices {
            src_tok_to_word[ti] = wi;
        }
    }
    let mut tgt_tok_to_word = vec![usize::MAX; tgt_tokens.len()];
    for (wi, w) in tgt_words.iter().enumerate() {
        for &ti in &w.token_indices {
            tgt_tok_to_word[ti] = wi;
        }
    }

    // Collapse token edges to word edges, deduplicating.
    let mut seen = std::collections::BTreeSet::new();
    let mut word_edges = Vec::new();
    for e in &edges {
        let sw = src_tok_to_word.get(e.src).copied().unwrap_or(usize::MAX);
        let tw = tgt_tok_to_word.get(e.tgt).copied().unwrap_or(usize::MAX);
        if sw == usize::MAX || tw == usize::MAX {
            continue; // edge involved a special token
        }
        if seen.insert((sw, tw)) {
            word_edges.push((sw, tw));
        }
    }

    Ok(TextAlignment {
        src_tokens,
        tgt_tokens,
        src_words,
        tgt_words,
        edges,
        word_edges,
    })
}
