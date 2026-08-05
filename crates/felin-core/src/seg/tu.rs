//! Splitting a chapter's paragraphs into similar-sized Translation blocks (TUs)
//! for parallel translation.
//!
//! Strategy (per project direction): **natural paragraph breaks are primary, the
//! character size is a soft target**. A long chapter (or a book with no chapter
//! divisions) is divided into `N ≈ round(total / target)` blocks of roughly equal
//! size, splitting only at paragraph boundaries. Blocks may run a little over or
//! under the target to respect those boundaries — the few seams that creates are
//! handled at merge/proofread time. A short chapter stays a single block.

use uuid::Uuid;

/// A planned Translation Unit (block): the paragraphs it covers and their
/// combined character length. `oversize` marks a block that is a single
/// paragraph larger than the target (it cannot be split further at a natural
/// boundary) — a hint for the UI/proofreader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuPlan {
    pub paragraph_ids: Vec<Uuid>,
    pub char_len: usize,
    pub oversize: bool,
}

/// Split `(paragraph_id, char_len)` pairs (in order) into ~equal blocks near
/// `target` characters, cutting only at paragraph boundaries.
pub fn aggregate(paras: &[(Uuid, usize)], target: usize) -> Vec<TuPlan> {
    if paras.is_empty() {
        return Vec::new();
    }
    let target = target.max(1);
    let total: usize = paras.iter().map(|(_, l)| *l).sum();
    // Number of similar-sized blocks; the char target is a soft guide, so we
    // round (a chapter only slightly over target stays one block rather than
    // spawning a tiny remainder).
    let n = ((total as f64 / target as f64).round() as usize).max(1);
    let ideal = total as f64 / n as f64;

    let mut blocks: Vec<TuPlan> = Vec::new();
    let mut ids: Vec<Uuid> = Vec::new();
    let mut cur_len = 0usize;
    let mut cumulative = 0usize;

    for &(id, plen) in paras {
        ids.push(id);
        cur_len += plen;
        cumulative += plen;
        // Close the current block once we cross its ideal cumulative boundary,
        // unless we're already filling the last block.
        let k = blocks.len() + 1;
        if k < n && cumulative as f64 >= k as f64 * ideal {
            let oversize = cur_len > target;
            blocks.push(TuPlan { paragraph_ids: std::mem::take(&mut ids), char_len: cur_len, oversize });
            cur_len = 0;
        }
    }
    if !ids.is_empty() {
        let oversize = cur_len > target;
        blocks.push(TuPlan { paragraph_ids: ids, char_len: cur_len, oversize });
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    fn lens(tus: &[TuPlan]) -> Vec<usize> {
        tus.iter().map(|t| t.char_len).collect()
    }

    #[test]
    fn short_chapter_is_one_block() {
        let id = ids(3);
        let tus = aggregate(&[(id[0], 100), (id[1], 100), (id[2], 100)], 3000);
        assert_eq!(tus.len(), 1);
        assert_eq!(tus[0].paragraph_ids, id);
    }

    #[test]
    fn long_chapter_splits_into_even_blocks() {
        // 9 × 1000 = 9000, target 3000 → 3 blocks of 3000, split at boundaries.
        let id = ids(9);
        let paras: Vec<_> = id.iter().map(|&i| (i, 1000)).collect();
        let tus = aggregate(&paras, 3000);
        assert_eq!(tus.len(), 3);
        assert_eq!(lens(&tus), vec![3000, 3000, 3000]);
        assert!(tus.iter().all(|t| !t.oversize));
    }

    #[test]
    fn blocks_are_similar_sized_no_tiny_remainder() {
        // 10 × 500 = 5000, target 3000 → 2 blocks of ~2500 (not 3000 + 2000).
        let id = ids(10);
        let paras: Vec<_> = id.iter().map(|&i| (i, 500)).collect();
        let tus = aggregate(&paras, 3000);
        assert_eq!(tus.len(), 2);
        assert_eq!(lens(&tus), vec![2500, 2500]);
    }

    #[test]
    fn slightly_over_target_stays_one_block() {
        // 4 × 1000 = 4000, target 3000 → round(1.33)=1 → one block (soft limit).
        let id = ids(4);
        let paras: Vec<_> = id.iter().map(|&i| (i, 1000)).collect();
        assert_eq!(aggregate(&paras, 3000).len(), 1);
    }

    #[test]
    fn block_over_target_is_flagged_oversize() {
        let id = ids(3);
        let tus = aggregate(&[(id[0], 100), (id[1], 9000), (id[2], 100)], 3000);
        // The block holding the 9000-char paragraph exceeds the target → flagged.
        assert!(tus.iter().any(|t| t.oversize && t.paragraph_ids.contains(&id[1])));
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert!(aggregate(&[], 3000).is_empty());
    }
}
