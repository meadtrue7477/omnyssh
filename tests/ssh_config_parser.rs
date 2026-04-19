// Integration tests for the SSH config parser are located alongside the
// implementation in `src/config/ssh_config.rs` (the `#[cfg(test)]` block).
//
// This file is intentionally minimal: the project is a pure-binary crate
// (no `src/lib.rs`), so integration tests in `tests/` cannot import from
// internal modules.  All unit tests live in the module itself.
