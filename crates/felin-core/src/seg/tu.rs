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

