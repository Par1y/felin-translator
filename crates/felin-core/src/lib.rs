//! # felin-core
//!
//! Tauri-agnostic domain logic for **Felin Translator** — a semi-automated
//! Japanese→Chinese translation proofreading workbench.
//!
//! The GUI shell (`src-tauri`) depends on this crate and stays thin: it wires
//! Tauri commands/events to the functions here. Keeping the domain logic free
//! of any Tauri dependency lets `cargo test -p felin-core` run fast without
//! building the WebView stack, and keeps the core reusable and unit-testable.
//!
//! ## Module map (mirrors the implementation plan)
//! - [`storage`] — SQLite schema + migrations for the global glossary DB and
//!   per-project DB. Fully implemented.
//! - [`ocr`] — sidecar spawn, JSONL progress parsing, manifest reconciliation,
//!   per-page ingestion (incl. cross-page paragraph merge), txt import. Fully
//!   implemented.
//! - [`seg`], [`llm`], [`names`], [`pipeline`] — later-milestone modules; present
//!   as documented stubs so the architecture is visible and the crate compiles.

#![forbid(unsafe_code)]

pub mod error;
pub mod types;
pub mod util;

pub mod archive;
pub mod config;
pub mod storage;
pub mod ocr;

pub mod seg;
pub mod llm;
pub mod names;
pub mod pipeline;

pub use error::{Error, Result};
