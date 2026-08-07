//! Cleaning raw OCR/text before segmentation: strip evaluator score tags and
//! PDF page-header artifacts the OCR layer (or a producer) may leave in the text.

use regex::Regex;
use std::sync::LazyLock;

static SCORE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[Score:\s*[0-9]+(?:\.[0-9]+)?\]").unwrap());

static PDF_HEADER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*-{2,}\s*pdf\s*\(\s*\d+\s*pages?\s*\)\s*-{2,}\s*$").unwrap()
});

/// Remove inline `[Score: x.xx]` tags and drop whole `--- PDF (N pages) ---`
/// header lines; trims the result. May return an empty string if the paragraph
/// was nothing but artifacts (the caller drops such paragraphs).
pub fn clean_text(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        if PDF_HEADER_RE.is_match(line) {
            continue;
        }
        lines.push(SCORE_RE.replace_all(line, "").into_owned());
    }
    lines.join("\n").trim().to_string()
}

