//! Crate error type.

use thiserror::Error;

/// Errors surfaced by the aligner. Most callers only see [`Error::Empty`]
/// (one side had no tokens) or [`Error::DimMismatch`] (embedding row
/// lengths differ).
#[derive(Debug, Error)]
pub enum Error {
    /// At least one of the input embedding matrices was empty.
    #[error("empty input: source has {src} tokens, target has {tgt}")]
    Empty {
        /// Number of source tokens.
        src: usize,
        /// Number of target tokens.
        tgt: usize,
    },

    /// Source and target embedding dimensions disagree.
    #[error("embedding dim mismatch: source dim {src_dim}, target dim {tgt_dim}")]
    DimMismatch {
        /// Source-side embedding dimensionality.
        src_dim: usize,
        /// Target-side embedding dimensionality.
        tgt_dim: usize,
    },

    /// An embedding row had a length that did not match the dim implied
    /// by the first row.
    #[error("ragged matrix: row {row} has {got} entries, expected {expected}")]
    Ragged {
        /// Row index that was wrong.
        row: usize,
        /// Length of that row.
        got: usize,
        /// Expected length.
        expected: usize,
    },

    /// Encoder produced no tokens for one of the input texts (e.g. empty
    /// string).
    #[cfg(feature = "crispembed")]
    #[error("encoder produced no tokens for {0}")]
    Untokenizable(&'static str),
}
