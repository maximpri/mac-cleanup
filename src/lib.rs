// SPDX-License-Identifier: GPL-3.0-or-later
//! Diskray: an X-ray for your Mac's disk. Understand storage and activity
//! before you delete anything.
//!
//! The executable starts on a read-only system dashboard. Process signals are
//! explicitly confirmed and identity-checked; storage cleanup paths are
//! allowlisted and revalidated before their contents are removed. See the
//! repository README for the complete safety model.

pub mod agent;
pub mod agent_tools;
pub mod ai;
pub mod artifacts;
pub mod cache;
pub mod care;
pub mod cli;
pub mod commands;
pub mod downloads;
mod file_actions;
pub mod growth;
pub mod headless;
pub mod history;
pub mod investigation;
pub mod mcp;
pub mod migrate;
pub mod paths;
pub mod pending;
pub mod plain;
pub mod processes;
pub mod relocation;
pub mod retention;
pub mod rules;
pub mod storage;
pub mod tui;
pub mod whitelist;
pub mod why;
