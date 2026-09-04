//! Every integration test in this crate, compiled into one binary.
//!
//! Cargo builds one executable per `.rs` file directly under `tests/`, and a
//! test run is dominated by linking those executables rather than by running
//! the tests. Keeping the files in `tests/all/` and declaring them here as
//! modules leaves every file, name and test exactly where it was while
//! collapsing the link step to one. A new test file costs a `mod` line here
//! and nothing at link time. See DEVELOPMENT.md, "Working on this", for the
//! measurements behind this.

mod edit_panel;
mod engine_link;
mod harmony;
mod presets;
mod recovery;
mod restore;
mod restore_panel;
mod session_panel;
mod shell_keys;
mod sync;
mod tracks_clear;
mod tracks_clipboard;
mod tracks_transpose;
mod transport_space;
mod write;
