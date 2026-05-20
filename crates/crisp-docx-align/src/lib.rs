//! Offline cross-lingual word/token alignment.
//!
//! Implements the three core SimAlign extraction strategies — *argmax*,
//! *intersection*, and *itermax* — over per-token contextual embeddings
//! produced by a multilingual encoder (mBERT, XLM-R, multilingual-MiniLM,
//! granite-embedding, etc). The encoder lives outside this crate so the
//! same algorithm can be exercised against any source of token vectors:
//!
//!   - The optional `crispembed` feature wires in
//!     [`CrispEmbed::encode_tokens`](https://github.com/CrispStrobe/CrispEmbed),
//!     producing a pure-Rust pipeline with no Python runtime.
//!   - The default build accepts caller-supplied embeddings, so the
//!     algorithm layer can be tested in isolation against fixture
//!     vectors or fed by external sources (e.g., Python via PyO3).
//!
//! ## Algorithm summary
//!
//! Given source tokens with embedding matrix `S ∈ R^{m × d}` and target
//! tokens `T ∈ R^{n × d}` (rows L2-normalised), the pairwise cosine
//! similarity is `C = S · Tᵀ`. From `C`:
//!
//!   - **`argmax_src_to_tgt`**: for each source row, pick its
//!     argmax-column. Yields `m` alignment edges.
//!   - **`argmax_tgt_to_src`**: for each target column, pick its
//!     argmax-row. Yields `n` edges.
//!   - **`intersection`**: keep only edges present in BOTH directions.
//!     High precision, low recall.
//!   - **`itermax`**: iteratively pick the global maximum, mask out its
//!     row and column, repeat — gives a one-to-one alignment until the
//!     similarity drops below `min_sim`. Good middle ground.
//!
//! ## Subword grouping
//!
//! The encoder's tokenizer (WordPiece or SentencePiece) splits surface
//! words into subwords. [`group_subwords_to_words`] re-aggregates token
//! indices into word indices using the marker convention reported by the
//! tokenizer: WordPiece continuations are prefixed `##`; SentencePiece
//! word starts are prefixed with U+2581 (`▁`).

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod algorithm;
mod errors;
mod format_transfer;
mod subword;

#[cfg(feature = "crispembed")]
mod embed;

pub use algorithm::{
    argmax_src_to_tgt, argmax_tgt_to_src, cosine_matrix, intersection, itermax, AlignmentEdge,
    Strategy,
};
pub use errors::Error;
pub use format_transfer::{transfer_format_via_words, SourceRun, TargetRun};
pub use subword::{group_subwords_to_words, TokenizerKind, WordGroup};

#[cfg(feature = "crispembed")]
pub use embed::{align_texts, TextAlignment};
#[cfg(feature = "crispembed")]
pub use format_transfer::{translate_runs, TranslatedRuns};
