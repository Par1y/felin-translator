//! Text normalization for proper-noun matching: NFKC (folds full/half-width and
//! compatibility variants) so "Ｔａｎａｋａ", "ﾀﾅｶ", etc. match their canonical forms.

use unicode_normalization::UnicodeNormalization;

/// NFKC-normalize `s` for matching.
pub fn normalize(s: &str) -> String {
    s.nfkc().collect()
}

