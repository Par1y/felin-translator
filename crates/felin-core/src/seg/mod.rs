//! Segmentation (plan step 5): clean text, recognize chapters, split TU blocks.
//!
//! Pure algorithms live here; persisting the result (creating chapters,
//! reassigning paragraphs, rebuilding TUs) is [`crate::storage::ProjectDb::segment`].
//! Tunables (block size, chapter patterns, sentence enders) come from
//! [`crate::config`].

pub mod chapters;
pub mod clean;
pub mod tu;

pub use chapters::{ChapterCut, ChapterRecognizer};
pub use clean::clean_text;
pub use tu::{aggregate, TuPlan};
