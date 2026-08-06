//! Image-directory selection for `batch` imports.
//!
//! The `ocr-cli batch` command has no pattern/range flag and would process every
//! image it finds (PDFs included). Selection is therefore applied *app-side*: a
//! pure [`select_images`] decides which files in a directory participate, the
//! user confirms the result via a scan preview, and only those files are staged
//! for `batch` (see [`crate::ocr::batch`]). A PDF mixed into an image directory
//! is never selected — it is non-expected input and is simply skipped.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The extensions `ocr-cli batch` understands *as images*. PDFs are deliberately
/// absent: a PDF in an image directory is non-expected and is skipped.
pub const BATCH_IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];

/// Preset selection rule (the plan's "preset + simple selector + custom rule").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePreset {
    /// Every image extension `batch` understands.
    All,
    /// `*.png` only.
    Png,
    /// `*.jpg` / `*.jpeg` only.
    Jpg,
    /// Stems that are entirely digits (`001`, `155a` → no; `155` → yes).
    Numbered,
    /// Stems that *start* with a digit (`001`, `155a` → yes; `a155` → no).
    NumberedPrefix,
}

/// A selection rule applied to an image directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ImageMatchRule {
    pub preset: ImagePreset,
    /// Shell-glob over the file name (e.g. `"*.png"`); overrides the preset's
    /// extension filter when set. `*`/`?` only.
    #[serde(default)]
    pub custom_glob: Option<String>,
    /// Regex matched against the file name (full match).
    #[serde(default)]
    pub custom_regex: Option<String>,
    /// Inclusive 1-based page range in natural reading order; `None` = no cut.
    #[serde(default)]
    pub range: Option<(u32, u32)>,
}

impl Default for ImagePreset {
    fn default() -> Self {
        ImagePreset::All
    }
}

/// Lowercased extension of `path` (without the dot), or `""` when none.
pub fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// Whether `name` (a basename) matches `rule`. `name` includes its extension.
pub fn name_matches(name: &str, rule: &ImageMatchRule) -> bool {
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy()).unwrap_or_default().into_owned();
    if let Some(g) = rule.custom_glob.as_deref() {
        if !glob_match(g, name) {
            return false;
        }
    } else if !matches_preset(&stem, &ext_of(Path::new(name)), rule.preset) {
        return false;
    }
    if let Some(re) = rule.custom_regex.as_deref() {
        let Ok(r) = regex::Regex::new(re) else { return false };
        if !r.is_match(name) {
            return false;
        }
    }
    true
}

/// Does `stem` satisfy the preset's shape filter? The extension check is
/// orthogonal — for `All`/`Png`/`Jpg` it is driven by the caller's extension
/// filter, for `Numbered`/`NumberedPrefix` purely by the stem.
fn matches_preset(stem: &str, ext: &str, preset: ImagePreset) -> bool {
    match preset {
        ImagePreset::All => !ext.is_empty() && BATCH_IMAGE_EXTS.contains(&ext),
        ImagePreset::Png => ext == "png",
        ImagePreset::Jpg => ext == "jpg" || ext == "jpeg",
        ImagePreset::Numbered => !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit()),
        ImagePreset::NumberedPrefix => {
            stem.chars().next().is_some_and(|c| c.is_ascii_digit())
        }
    }
}

/// Leading run of ASCII digits of `s`, as `(number, remainder)`.
fn numeric_prefix(s: &str) -> Option<(u64, &str)> {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if end == 0 {
        None
    } else {
        s[..end].parse::<u64>().ok().map(|n| (n, &s[end..]))
    }
}

/// Natural-reading-order sort key: files whose name starts with a number sort
/// first (numerically), then everything else alphabetically. `001.png` <
/// `002.png` < `10.png` < `155a.jpg` < `cover.png`.
fn natural_key(name: &str) -> (u8, u64, String) {
    match numeric_prefix(name) {
        Some((n, rest)) => (0, n, rest.to_string()),
        None => (1, 0, name.to_string()),
    }
}

/// Convert a `*`/`?` glob into an anchored regex (the subset of glob syntax the
/// custom-rule selector supports).
fn glob_match(glob: &str, name: &str) -> bool {
    let mut pat = String::from("^");
    for c in glob.chars() {
        match c {
            '*' => pat.push_str(".*"),
            '?' => pat.push('.'),
            _ => {
                // regex::escape produces a literal that appends cleanly.
                pat.push_str(&regex::escape(&c.to_string()));
            }
        }
    }
    pat.push('$');
    regex::Regex::new(&pat).is_ok_and(|r| r.is_match(name))
}

/// Select the images in `dir` that match `rule`, in natural reading order, with
/// the (optional) 1-based range applied. PDFs and non-image files are ignored;
/// only regular files are considered. Errors only if `dir` cannot be read.
pub fn select_images(dir: &Path, rule: &ImageMatchRule) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(dir).map_err(|e| {
        Error::InvalidInput { detail: format!("cannot read directory {}: {e}", dir.display()) }
    })?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            Error::InvalidInput { detail: format!("cannot read directory entry in {}: {e}", dir.display()) }
        })?;
        let ft = entry.file_type().map_err(|e| {
            Error::InvalidInput { detail: format!("cannot stat {}: {e}", entry.path().display()) }
        })?;
        if !ft.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !BATCH_IMAGE_EXTS.contains(&ext_of(&entry.path()).as_str()) {
            continue;
        }
        if name_matches(&name, rule) {
            names.push(name);
        }
    }
    names.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)));

    let mut out: Vec<PathBuf> = names
        .into_iter()
        .map(|n| dir.join(n))
        .collect();
    if let Some((start, end)) = rule.range {
        let start = start.max(1) as usize;
        let end = end.max(start as u32) as usize;
        out = out.into_iter().enumerate().filter(|(i, _)| i + 1 >= start && i + 1 <= end).map(|(_, p)| p).collect();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(dir: &Path, names: &[&str]) {
        for n in names {
            fs::write(dir.join(n), "x").unwrap();
        }
    }

    fn rule(preset: ImagePreset) -> ImageMatchRule {
        ImageMatchRule { preset, custom_glob: None, custom_regex: None, range: None }
    }

    #[test]
    fn natural_sort_puts_number_prefixes_first() {
        let dir = tempdir().unwrap();
        write(
            dir.path(),
            &["155a.jpg", "003.png", "cover.png", "001.png", "10.png"],
        );
        let sel = select_images(dir.path(), &rule(ImagePreset::All)).unwrap();
        let names: Vec<String> = sel.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["001.png", "003.png", "10.png", "155a.jpg", "cover.png"]);
    }

    #[test]
    fn pdfs_are_never_selected() {
        let dir = tempdir().unwrap();
        write(dir.path(), &["a.png", "b.pdf", "c.jpg"]);
        let sel = select_images(dir.path(), &rule(ImagePreset::All)).unwrap();
        let names: Vec<String> = sel.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["a.png", "c.jpg"]);
    }

    #[test]
    fn preset_filters_by_extension_and_shape() {
        let dir = tempdir().unwrap();
        write(dir.path(), &["a.png", "b.jpg", "155.png", "155a.png", "155a.jpg", "c.jpeg"]);

        let png = select_images(dir.path(), &rule(ImagePreset::Png)).unwrap();
        let png: Vec<String> = png.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(png, vec!["155.png", "155a.png", "a.png"]);

        let jpg = select_images(dir.path(), &rule(ImagePreset::Jpg)).unwrap();
        let jpg: Vec<String> = jpg.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(jpg, vec!["155a.jpg", "b.jpg", "c.jpeg"]);

        let num = select_images(dir.path(), &rule(ImagePreset::Numbered)).unwrap();
        let num: Vec<String> = num.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(num, vec!["155.png"]);

        let numpref = select_images(dir.path(), &rule(ImagePreset::NumberedPrefix)).unwrap();
        let numpref: Vec<String> = numpref.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(numpref, vec!["155.png", "155a.jpg", "155a.png"]);
    }

    #[test]
    fn custom_glob_and_regex_override_preset() {
        let dir = tempdir().unwrap();
        write(dir.path(), &["a.png", "b.jpg", "keep1.png", "keep2.png"]);

        let mut r = rule(ImagePreset::Png);
        r.custom_glob = Some("keep*.png".to_string());
        let sel = select_images(dir.path(), &r).unwrap();
        let names: Vec<String> = sel.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["keep1.png", "keep2.png"]);

        let mut r2 = rule(ImagePreset::All);
        r2.custom_regex = Some(r"^(a|b)\.".to_string());
        let sel = select_images(dir.path(), &r2).unwrap();
        let names: Vec<String> = sel.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["a.png", "b.jpg"]);
    }

    #[test]
    fn range_cuts_by_natural_order() {
        let dir = tempdir().unwrap();
        write(dir.path(), &["001.png", "002.png", "003.png", "004.png"]);
        let mut r = rule(ImagePreset::All);
        r.range = Some((2, 3));
        let sel = select_images(dir.path(), &r).unwrap();
        let names: Vec<String> = sel.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["002.png", "003.png"]);
    }

    #[test]
    fn missing_dir_is_an_error() {
        let err = select_images(Path::new("/definitely/not/here"), &rule(ImagePreset::All)).unwrap_err();
        assert!(err.to_string().contains("cannot read directory"));
    }
}
