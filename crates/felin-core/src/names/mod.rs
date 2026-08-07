//! Proper-noun glossary (plan step 7): normalization, fuzzy matching, the
//! Aho-Corasick matcher, CSV import, and the LLM name-extraction pass.
//!
//! Matching/normalization/fuzzy/CSV are pure and unit-tested; the extraction
//! pass drives [`crate::llm`]. Persistence lives in the storage layer
//! ([`crate::storage::GlobalDb`] glossary + [`crate::storage::ProjectDb`]
//! extracted names / overrides).

pub mod csv;
pub mod extract;
pub mod fuzzy;
pub mod matcher;
pub mod normalize;

pub use csv::{ColumnMapping, NameRow};
pub use extract::{classify_names, extract_names, parse_candidates, Candidate, TagSuggestion};
pub use fuzzy::{levenshtein, within_distance};
pub use matcher::{Hit, Matcher};
pub use normalize::normalize;

