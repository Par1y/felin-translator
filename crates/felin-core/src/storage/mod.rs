//! Storage layer.
//!
//! WAL SQLite with a single serialized write connection plus a read pool, a
//! small transactional migration runner (with pre-upgrade backup and a
//! forward-version guard), and typed wrappers for the two databases:
//! [`GlobalDb`] (the shared glossary) and [`ProjectDb`] (per project).

mod db;
mod lock;
mod migrations;

pub mod global;
pub mod project;

pub use db::{Db, DbTuning};
pub use global::{GlobalDb, GLOBAL_MIGRATIONS};
pub use lock::ProjectLock;
pub use migrations::{latest_version, Migration};
pub use project::{ProjectDb, PROJECT_MIGRATIONS};
