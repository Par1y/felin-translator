//! Segmentation integration tests: text cleaning, chapter detection, and
//! balanced TU block planning.
//!
//! Moved here from the crate's inline `#[cfg(test)]` modules (project policy:
//! no test code alongside application code).

use felin_core::seg::{aggregate, clean_text, ChapterCut, ChapterRecognizer, TuPlan};
use uuid::Uuid;

// ----- seg/clean -----------------------------------------------------------

#[test]
fn strips_score_tags() {
    assert_eq!(clean_text("これは本文です。[Score: 0.87]"), "これは本文です。");
    assert_eq!(clean_text("[Score: 0.4]先頭タグ"), "先頭タグ");
}

#[test]
fn drops_pdf_header_lines() {
    let input = "--- PDF (320 pages) ---\n第一章\n本文";
    assert_eq!(clean_text(input), "第一章\n本文");
    assert_eq!(clean_text("---  PDF ( 5 page ) ---"), "");
}

#[test]
fn keeps_ordinary_text_and_trims() {
    assert_eq!(clean_text("  普通の段落。  "), "普通の段落。");
    assert_eq!(clean_text("行1\n行2"), "行1\n行2");
}

// ----- seg/tu ---------------------------------------------------------------

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

// ----- seg/chapters ----------------------------------------------------------

fn titles(cuts: &[ChapterCut]) -> Vec<(&str, usize)> {
    cuts.iter().map(|c| (c.title.as_str(), c.start)).collect()
}

#[test]
fn no_headings_is_one_fallback_chapter() {
    let cuts = ChapterRecognizer::default().detect(&["ただの本文。", "続きの段落。"], "正文");
    assert_eq!(titles(&cuts), vec![("正文", 0)]);
}

#[test]
fn detects_japanese_chapter_headings() {
    let cuts = ChapterRecognizer::default().detect(
        &["第一章 出会い", "本文A", "第二章 別れ", "本文B"],
        "正文",
    );
    assert_eq!(titles(&cuts), vec![("第一章 出会い", 0), ("第二章 別れ", 2)]);
}

#[test]
fn leading_content_before_first_heading_gets_fallback_chapter() {
    let cuts = ChapterRecognizer::default().detect(&["まえがきの文章。", "第1話 はじまり", "本文"], "正文");
    assert_eq!(cuts[0].start, 0);
    assert_eq!(cuts[1].title, "第1話 はじまり");
    assert_eq!(cuts[1].start, 1);
}

#[test]
fn ignores_sentences_that_merely_start_like_a_heading() {
    let long = "第一章のことについて長々と説明する非常に長い一文がここに続いていく本文段落";
    assert_eq!(
        ChapterRecognizer::default().detect(&[long], "正文"),
        vec![ChapterCut { title: "正文".into(), start: 0 }]
    );
}

#[test]
fn custom_patterns_are_honored() {
    let r = ChapterRecognizer::new(&[r"^Scene\s+\d+".to_string()], 40);
    let cuts = r.detect(&["intro", "Scene 1", "body"], "start");
    assert_eq!(cuts.len(), 2);
    assert_eq!(cuts[1].title, "Scene 1");
}
