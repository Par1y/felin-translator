//! Rule-based chapter recognition over an ordered paragraph sequence.
//!
//! Heading patterns come from config (preset defaults, user-editable). A short
//! first line matching any pattern starts a new chapter; a book with no such
//! headings becomes one chapter (fallback), and content before the first
//! heading forms a leading fallback chapter.

use crate::config::DEFAULT_CHAPTER_PATTERN;
use regex::Regex;

/// A detected chapter boundary: its `title` and the index of the paragraph that
/// begins it (that paragraph is kept as the chapter's first body paragraph).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterCut {
    pub title: String,
    pub start: usize,
}

/// A compiled chapter-heading recognizer.
pub struct ChapterRecognizer {
    regexes: Vec<Regex>,
    max_chars: usize,
}

impl ChapterRecognizer {
    /// Compile `patterns` (invalid ones are skipped with a warning). `max_chars`
    /// caps how long a candidate heading line may be.
    pub fn new(patterns: &[String], max_chars: usize) -> Self {
        let regexes = patterns
            .iter()
            .filter_map(|p| match Regex::new(p) {
                Ok(re) => Some(re),
                Err(e) => {
                    tracing::warn!(pattern = p, error = %e, "invalid chapter pattern; skipping");
                    None
                }
            })
            .collect();
        Self { regexes, max_chars: max_chars.max(1) }
    }

    fn heading_title(&self, text: &str) -> Option<String> {
        let line = text.trim().lines().next()?.trim();
        if line.is_empty() || line.chars().count() > self.max_chars {
            return None;
        }
        self.regexes.iter().any(|re| re.is_match(line)).then(|| line.to_string())
    }

    /// Detect chapter cuts over `paras` (reading order). Always returns at least
    /// one cut starting at index 0.
    pub fn detect(&self, paras: &[&str], fallback_title: &str) -> Vec<ChapterCut> {
        let mut cuts: Vec<ChapterCut> = Vec::new();
        for (i, text) in paras.iter().enumerate() {
            if let Some(title) = self.heading_title(text) {
                cuts.push(ChapterCut { title, start: i });
            }
        }
        if cuts.first().map_or(true, |c| c.start != 0) {
            cuts.insert(0, ChapterCut { title: fallback_title.to_string(), start: 0 });
        }
        cuts
    }
}

impl Default for ChapterRecognizer {
    fn default() -> Self {
        Self::new(&[DEFAULT_CHAPTER_PATTERN.to_string()], 40)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let cuts = ChapterRecognizer::default().detect(&["第一章 出会い", "本文A", "第二章 別れ", "本文B"], "正文");
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
}
