//! Turning per-page OCR results into paragraphs.
//!
//! Two responsibilities, both pure and heavily unit-tested:
//! 1. Split each page's text into paragraph blocks on blank lines.
//! 2. Stitch a sentence that a page break cut in half: if a page's last block
//!    does not end in sentence-ending punctuation, it is joined with the *next*
//!    page's first block (only across directly adjacent pages — never across a
//!    failed/blank/missing page, where content is unknown).
//!
//! The merged paragraph records its **starting** page as `page_num` and the
//! original file as `source_file` (plan §1).

use crate::types::{IngestedParagraph, OcrParagraphStatus};

/// A single OK page's data, ready for paragraph assembly. Blank/failed pages are
/// filtered out by the caller before they reach here.
#[derive(Debug, Clone)]
pub struct PageForIngest {
    pub page: i64,
    pub text: String,
    /// `None` when the evaluator was disabled / no score was present.
    pub score: Option<f64>,
    pub quality_warning: bool,
    pub blank: bool,
    pub best_score: Option<f64>,
    pub fallback: bool,
    pub source_file: String,
    /// True when this page came from a rescue re-OCR of a previously-failed page.
    pub recovered: bool,
}

/// True if `s` (trimmed) ends with one of the sentence-terminating `enders`.
pub fn ends_with_sentence_punct(s: &str, enders: &[char]) -> bool {
    s.trim_end().chars().next_back().is_some_and(|c| enders.contains(&c))
}

/// The "worse" (lower) of two optional scores, ignoring absent ones. Used to
/// propagate the weakest page's score onto a cross-page-merged paragraph.
fn worse_score(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Split page text into non-empty paragraph blocks on blank-line boundaries.
/// Lines within a block keep their internal newlines (reversible); the block is
/// trimmed at its edges, and leading/trailing whitespace-only lines are dropped.
pub fn split_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                blocks.push(cur.join("\n").trim().to_string());
                cur.clear();
            }
        } else {
            cur.push(line);
        }
    }
    if !cur.is_empty() {
        blocks.push(cur.join("\n").trim().to_string());
    }
    blocks.retain(|b| !b.is_empty());
    blocks
}

// Internal working paragraph, before conversion to `IngestedParagraph`.
struct ParaBuilder {
    text: String,
    start_page: i64,
    end_page: i64,
    score: Option<f64>,
    quality_warning: bool,
    best_score: Option<f64>,
    fallback: bool,
    source_file: String,
    recovered: bool,
}

/// Assemble paragraphs from OK pages, performing cross-page sentence merging.
///
/// `low_score_threshold`: pages with a present score below this (or with
/// `quality_warning`) yield paragraphs flagged [`OcrParagraphStatus::LowScore`].
pub fn build_paragraphs(pages: &[PageForIngest], low_score_threshold: f64, enders: &[char]) -> Vec<IngestedParagraph> {
    // Only OK, non-blank pages contribute text; process in ascending page order.
    let mut pages: Vec<&PageForIngest> = pages.iter().filter(|p| !p.blank && !p.text.trim().is_empty()).collect();
    pages.sort_by_key(|p| p.page);

    let mut work: Vec<ParaBuilder> = Vec::new();
    for p in pages {
        let blocks = split_blocks(&p.text);
        for (i, block) in blocks.into_iter().enumerate() {
            let is_first = i == 0;
            // Merge only at a page seam: this is the page's first block, the
            // previous paragraph ended on the immediately-preceding page, and it
            // did not end in sentence punctuation.
            let merge = is_first
                && work.last().is_some_and(|last| {
                    // checked_add avoids an overflow panic on adversarial page
                    // numbers (e.g. i64::MAX) from untrusted per-page JSON.
                    last.end_page.checked_add(1) == Some(p.page)
                        && !ends_with_sentence_punct(&last.text, enders)
                });
            if merge {
                let last = work.last_mut().expect("checked is_some");
                last.text.push_str(&block);
                last.end_page = p.page;
                // Fold the absorbed page's quality signals in, so a merged
                // paragraph spanning a low-score/warned page is still flagged.
                last.quality_warning |= p.quality_warning;
                last.recovered |= p.recovered;
                last.score = worse_score(last.score, p.score);
                last.best_score = worse_score(last.best_score, p.best_score);
            } else {
                work.push(ParaBuilder {
                    text: block,
                    start_page: p.page,
                    end_page: p.page,
                    score: p.score,
                    quality_warning: p.quality_warning,
                    best_score: p.best_score,
                    fallback: p.fallback,
                    source_file: p.source_file.clone(),
                    recovered: p.recovered,
                });
            }
        }
    }

    work.into_iter()
        .map(|b| {
            let status = if b.recovered {
                OcrParagraphStatus::PageFailedRecovered
            } else if b.quality_warning || b.score.is_some_and(|s| s < low_score_threshold) {
                OcrParagraphStatus::LowScore
            } else {
                OcrParagraphStatus::Ok
            };
            let meta = serde_json::json!({
                "quality_warning": b.quality_warning,
                "best_score": b.best_score,
                "fallback": b.fallback,
                "start_page": b.start_page,
                "end_page": b.end_page,
            });
            IngestedParagraph::new(b.text, Some(b.start_page), b.source_file, b.score, status, meta)
        })
        .collect()
}

