//! Alignment extraction over a similarity matrix.
//!
//! The matrix layer takes two embedding matrices and produces token-pair
//! alignments under one of three SimAlign strategies. The
//! [`cosine_matrix`] helper assumes rows are L2-normalised (the convention
//! used by [`CrispEmbed::encode_tokens`](crispembed::CrispEmbed)) and so
//! computes inner products directly. If your rows are not normalised,
//! normalise them before calling — the algorithm layer doesn't second-guess.

use crate::errors::Error;

/// An edge in the alignment: `(source_token_index, target_token_index,
/// similarity)`. The two indices are positions into the original token
/// vectors as returned by the encoder — subword indexes, not word indexes.
/// See [`crate::group_subwords_to_words`] for the word-level upgrade.
#[derive(Debug, Clone, Copy)]
pub struct AlignmentEdge {
    /// Source-side token index.
    pub src: usize,
    /// Target-side token index.
    pub tgt: usize,
    /// Cosine similarity at this cell.
    pub sim: f32,
}

/// Selectable extraction strategy. See the module-level docs for the
/// trade-offs between recall and precision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strategy {
    /// Each source token aligned to its argmax target. Recall-leaning;
    /// `m` edges out, source-coverage = 100%.
    ArgmaxSrcToTgt,
    /// Each target token aligned to its argmax source. `n` edges out,
    /// target-coverage = 100%.
    ArgmaxTgtToSrc,
    /// Intersection of the two argmax directions. High precision; each
    /// edge is the mutually-best match in its row AND its column.
    Intersection,
    /// Iterative max with a similarity floor. Picks the overall best
    /// cell, masks its row and column, repeats. Yields a one-to-one
    /// matching up to `min_sim`.
    Itermax {
        /// Floor on similarity. Cells below this are skipped.
        min_sim: f32,
    },
}

/// Compute the full cosine-similarity matrix `S · Tᵀ` for two embedding
/// matrices. Rows of both inputs are assumed L2-normalised.
///
/// Layout of returned matrix: row-major `[m × n]` (m = source tokens,
/// n = target tokens). Entry `(i, j)` = `S[i] · T[j]`.
pub fn cosine_matrix(src: &[Vec<f32>], tgt: &[Vec<f32>]) -> Result<Vec<f32>, Error> {
    if src.is_empty() || tgt.is_empty() {
        return Err(Error::Empty {
            src: src.len(),
            tgt: tgt.len(),
        });
    }
    let dim = src[0].len();
    let tgt_dim = tgt[0].len();
    if dim != tgt_dim {
        return Err(Error::DimMismatch {
            src_dim: dim,
            tgt_dim,
        });
    }
    for (row, v) in src.iter().enumerate() {
        if v.len() != dim {
            return Err(Error::Ragged {
                row,
                got: v.len(),
                expected: dim,
            });
        }
    }
    for (row, v) in tgt.iter().enumerate() {
        if v.len() != dim {
            return Err(Error::Ragged {
                row,
                got: v.len(),
                expected: dim,
            });
        }
    }

    let m = src.len();
    let n = tgt.len();
    let mut out = vec![0.0f32; m * n];
    for i in 0..m {
        let s = &src[i];
        for j in 0..n {
            let t = &tgt[j];
            let mut acc = 0.0f32;
            for d in 0..dim {
                acc += s[d] * t[d];
            }
            out[i * n + j] = acc;
        }
    }
    Ok(out)
}

/// For each source row, return the argmax target column as an
/// [`AlignmentEdge`]. Output length: `m` (one edge per source token).
/// Edges are ordered by source index.
pub fn argmax_src_to_tgt(matrix: &[f32], m: usize, n: usize) -> Vec<AlignmentEdge> {
    let mut out = Vec::with_capacity(m);
    for i in 0..m {
        let row = &matrix[i * n..(i + 1) * n];
        if let Some((j, &sim)) = row
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        {
            out.push(AlignmentEdge {
                src: i,
                tgt: j,
                sim,
            });
        }
    }
    out
}

/// For each target column, return the argmax source row.
pub fn argmax_tgt_to_src(matrix: &[f32], m: usize, n: usize) -> Vec<AlignmentEdge> {
    let mut out = Vec::with_capacity(n);
    for j in 0..n {
        let mut best = (0usize, f32::NEG_INFINITY);
        for i in 0..m {
            let v = matrix[i * n + j];
            if v > best.1 {
                best = (i, v);
            }
        }
        out.push(AlignmentEdge {
            src: best.0,
            tgt: j,
            sim: best.1,
        });
    }
    out
}

/// Intersection of the two argmax directions — the SimAlign "argmax+mwmf"
/// precision-leaning extraction. Edges sorted by source index.
pub fn intersection(matrix: &[f32], m: usize, n: usize) -> Vec<AlignmentEdge> {
    let s2t = argmax_src_to_tgt(matrix, m, n);
    let t2s = argmax_tgt_to_src(matrix, m, n);
    let mut t2s_lookup = vec![usize::MAX; n];
    for e in &t2s {
        t2s_lookup[e.tgt] = e.src;
    }
    let mut out = Vec::new();
    for e in s2t {
        if t2s_lookup[e.tgt] == e.src {
            out.push(e);
        }
    }
    out
}

/// Iterative-max extraction: while the global maximum of the matrix is
/// `>= min_sim`, emit that edge, mask its row + column, repeat.
pub fn itermax(matrix: &[f32], m: usize, n: usize, min_sim: f32) -> Vec<AlignmentEdge> {
    let mut work = matrix.to_vec();
    let mut out = Vec::new();
    loop {
        let mut best = (0usize, 0usize, f32::NEG_INFINITY);
        for i in 0..m {
            for j in 0..n {
                let v = work[i * n + j];
                if v > best.2 {
                    best = (i, j, v);
                }
            }
        }
        if best.2 < min_sim {
            break;
        }
        out.push(AlignmentEdge {
            src: best.0,
            tgt: best.1,
            sim: best.2,
        });
        // Mask out this row and column.
        for k in 0..n {
            work[best.0 * n + k] = f32::NEG_INFINITY;
        }
        for k in 0..m {
            work[k * n + best.1] = f32::NEG_INFINITY;
        }
    }
    out.sort_by_key(|e| e.src);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(v: &mut [f32]) {
        let mut n = 0.0;
        for x in v.iter() {
            n += x * x;
        }
        let n = n.sqrt().max(1e-9);
        for x in v.iter_mut() {
            *x /= n;
        }
    }

    fn unit(d: usize, dim: usize) -> Vec<f32> {
        let mut v = vec![0.0; dim];
        v[d] = 1.0;
        v
    }

    #[test]
    fn cosine_matrix_orthonormal_basis_is_identity() {
        // 3 source tokens and 3 target tokens, each an orthonormal
        // basis vector. The matrix should be I_3.
        let src: Vec<Vec<f32>> = (0..3).map(|d| unit(d, 4)).collect();
        let tgt = src.clone();
        let m = cosine_matrix(&src, &tgt).unwrap();
        let expected = vec![
            1.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, //
            0.0, 0.0, 1.0, //
        ];
        for (i, (a, b)) in m.iter().zip(&expected).enumerate() {
            assert!((a - b).abs() < 1e-6, "[{i}] {a} vs {b}");
        }
    }

    #[test]
    fn argmax_picks_diagonal_for_identity() {
        let src: Vec<Vec<f32>> = (0..3).map(|d| unit(d, 4)).collect();
        let tgt = src.clone();
        let m = cosine_matrix(&src, &tgt).unwrap();
        let edges = argmax_src_to_tgt(&m, 3, 3);
        assert_eq!(edges.len(), 3);
        for (i, e) in edges.iter().enumerate() {
            assert_eq!(e.src, i);
            assert_eq!(e.tgt, i);
            assert!((e.sim - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn intersection_drops_one_directional_matches() {
        // Two source tokens that both prefer target 0; target 0 prefers
        // source 0; target 1 prefers source 1 even though source 1 is
        // closer to target 0. Build the matrix manually.
        //
        //          tgt0  tgt1
        // src0 →   0.9   0.1
        // src1 →   0.7   0.6
        //
        // argmax_s2t: 0->0, 1->0
        // argmax_t2s: 0->0 (max in col 0), 1->1 (max in col 1)
        // intersection: only 0->0.
        let mat = vec![0.9, 0.1, 0.7, 0.6];
        let edges = intersection(&mat, 2, 2);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src, 0);
        assert_eq!(edges[0].tgt, 0);
    }

    #[test]
    fn itermax_yields_one_to_one_above_threshold() {
        // 3x3, diagonal-heavy. Threshold 0.4 should keep all three.
        let mat = vec![
            0.9, 0.2, 0.1, //
            0.3, 0.8, 0.2, //
            0.1, 0.2, 0.7, //
        ];
        let edges = itermax(&mat, 3, 3, 0.4);
        assert_eq!(edges.len(), 3);
        for (i, expected) in [(0, 0, 0.9_f32), (1, 1, 0.8), (2, 2, 0.7)]
            .iter()
            .enumerate()
        {
            assert_eq!(edges[i].src, expected.0);
            assert_eq!(edges[i].tgt, expected.1);
            assert!((edges[i].sim - expected.2).abs() < 1e-6);
        }
    }

    #[test]
    fn itermax_stops_at_threshold() {
        let mat = vec![
            0.9, 0.2, //
            0.1, 0.3, // best cell after masking row0/col0 is 0.3 < 0.5
        ];
        let edges = itermax(&mat, 2, 2, 0.5);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].src, 0);
        assert_eq!(edges[0].tgt, 0);
    }

    #[test]
    fn ragged_input_is_rejected() {
        let mut a = vec![1.0, 0.0, 0.0];
        norm(&mut a);
        let src = vec![a.clone()];
        let tgt = vec![vec![1.0, 0.0]]; // dim 2, mismatched
        let e = cosine_matrix(&src, &tgt).unwrap_err();
        match e {
            Error::DimMismatch { src_dim, tgt_dim } => {
                assert_eq!(src_dim, 3);
                assert_eq!(tgt_dim, 2);
            }
            _ => panic!("expected DimMismatch, got {e:?}"),
        }
    }

    #[test]
    fn empty_input_is_rejected() {
        let e = cosine_matrix(&[], &[vec![1.0]]).unwrap_err();
        assert!(matches!(e, Error::Empty { .. }));
    }
}
