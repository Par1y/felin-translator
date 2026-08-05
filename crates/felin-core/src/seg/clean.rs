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

#[cfg(test)]
mod tests {
    use super::*;

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
}
