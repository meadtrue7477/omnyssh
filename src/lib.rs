//! OmnySSH library — public API exposed for integration tests.
//!
//! The binary entry point lives in `main.rs`. This lib target re-exports the
//! internal modules so that files under `tests/` can reach them.

#![allow(dead_code)]

pub mod app;
pub mod config;
pub mod event;
pub mod ssh;
pub mod ui;
pub mod utils;
