//! Conservative macOS system care for process health and allowlisted storage.
//!
//! The executable starts on a read-only system dashboard. Process signals are
//! explicitly confirmed and identity-checked; storage cleanup paths are
//! allowlisted and revalidated before their contents are removed. See the
//! repository README for the complete safety model.

pub mod ai;
pub mod cache;
pub mod care;
pub mod cli;
pub mod downloads;
pub mod history;
pub mod plain;
pub mod processes;
pub mod relocation;
pub mod retention;
pub mod storage;
pub mod tui;
pub mod whitelist;
