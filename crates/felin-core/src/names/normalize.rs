//! Text normalization for proper-noun matching: NFKC (folds full/half-width and
//! compatibility variants) so "Ｔａｎａｋａ", "ﾀﾅｶ", etc. match their canonical forms.

use unicode_normalization::UnicodeNormalization;

/// NFKC-normalize `s` for matching.
pub fn normalize(s: &str) -> String {
    s.nfkc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_fullwidth_ascii() {
        assert_eq!(normalize("Ｔａｎａｋａ"), "Tanaka");
        assert_eq!(normalize("１２３"), "123");
    }

    #[test]
    fn folds_halfwidth_katakana() {
        // Halfwidth katakana → fullwidth.
        assert_eq!(normalize("ﾀﾅｶ"), "タナカ");
    }

    #[test]
    fn leaves_normal_text_unchanged() {
        assert_eq!(normalize("田中"), "田中");
        assert_eq!(normalize("猫"), "猫");
    }
}
