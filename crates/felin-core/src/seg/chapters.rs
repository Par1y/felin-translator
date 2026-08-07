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

