//! Character-level Levenshtein distance, used to flag OCR-typo "suspect" matches
//! (edit distance ≤ 1) without auto-replacing them.

/// Levenshtein edit distance between `a` and `b`, counted in Unicode scalar
/// values (so Japanese characters count as one each).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// True if `a` and `b` are within `max` edits (with a cheap length pre-check).
pub fn within_distance(a: &str, b: &str, max: usize) -> bool {
    let (la, lb) = (a.chars().count(), b.chars().count());
    la.abs_diff(lb) <= max && levenshtein(a, b) <= max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_distances() {
        assert_eq!(levenshtein("田中", "田中"), 0);
        assert_eq!(levenshtein("田中", "田申"), 1); // substitution
        assert_eq!(levenshtein("田中", "田中角"), 1); // insertion
        assert_eq!(levenshtein("田中角", "田中"), 1); // deletion
        assert_eq!(levenshtein("田中", "本田"), 2);
        assert_eq!(levenshtein("", "猫"), 1);
    }

    #[test]
    fn within_distance_pre_checks_length() {
        assert!(within_distance("サクラ", "サクヲ", 1)); // one substitution
        assert!(within_distance("サクラ", "サク", 1)); // one deletion
        assert!(!within_distance("あいうえお", "か", 1)); // far apart
    }
}
