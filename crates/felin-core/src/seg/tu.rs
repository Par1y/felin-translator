//! Splitting a chapter's paragraphs into similar-sized Translation blocks (TUs)
//! for parallel translation.
//!
//! Strategy (per project direction): **the target block size is the priority,
//! natural paragraph breaks are soft boundaries**. A long chapter (or a book
//! with no chapter divisions) is divided into blocks of roughly `target`
//! characters, splitting only at paragraph boundaries. A block is never closed
//! at a boundary while it is still far below the target — small remainder
//! paragraphs are absorbed into the current block so no tiny block is stranded.

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

/// Split `(paragraph_id, char_len)` pairs (in order) into blocks near `target`
/// characters, cutting only at paragraph boundaries.
///
/// Size wins over natural breaks: a block only closes at a boundary once it is
/// close enough to `target` (reached it, or is at least half a target and the
/// next paragraph would push it far over). Any tail smaller than half a target
/// is absorbed into the last block rather than stranded as a tiny remainder.
pub fn aggregate(paras: &[(Uuid, usize)], target: usize) -> Vec<TuPlan> {
    if paras.is_empty() {
        return Vec::new();
    }
    let target = target.max(1);
    let total: usize = paras.iter().map(|(_, l)| *l).sum();
    let half = target / 2; // "meaningful block" floor; absorb tails at or below this

    let mut blocks: Vec<TuPlan> = Vec::new();
    let mut ids: Vec<Uuid> = Vec::new();
    let mut cur_len = 0usize;
    // Length of every paragraph processed so far (never reset on block close),
    // so `remaining` stays the true tail size — not inflated by closed blocks.
    let mut consumed = 0usize;

    for (i, &(id, plen)) in paras.iter().enumerate() {
        ids.push(id);
        cur_len += plen;
        consumed += plen;
        let remaining = total - consumed;

        if i + 1 == paras.len() {
            // Last paragraph always closes the current block.
            blocks.push(TuPlan {
                paragraph_ids: std::mem::take(&mut ids),
                char_len: cur_len,
                oversize: cur_len > target,
            });
            continue;
        }
        // Absorb a small tail rather than strand it as a tiny block.
        if remaining <= half {
            continue;
        }
        // Close at a natural break once the block is close enough to target:
        //  - it already reached target, OR
        //  - it is at least half a target and the next paragraph would push it
        //    far over (> 1.5× target).
        let next_len = paras[i + 1].1;
        let reached = cur_len >= target;
        let would_overshoot = cur_len + next_len > target + half;
        if reached || (cur_len >= half && would_overshoot) {
            blocks.push(TuPlan {
                paragraph_ids: std::mem::take(&mut ids),
                char_len: cur_len,
                oversize: cur_len > target,
            });
            cur_len = 0;
        }
    }
    blocks
}

