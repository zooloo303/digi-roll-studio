//! Every integration test in this crate, compiled into one binary.
//!
//! Cargo builds one executable per `.rs` file directly under `tests/`, and a
//! test run is dominated by linking those executables rather than by running
//! the tests. Keeping the files in `tests/all/` and declaring them here as
//! modules leaves every file, name and test exactly where it was while
//! collapsing the link step to one. A new test file costs a `mod` line here
//! and nothing at link time. See DEVELOPMENT.md, "Working on this", for the
//! measurements behind this.

mod common;
mod a4;
mod a4_safe_write;
mod copy_track;
mod dn2;
mod drive_preset;
mod dt2;
mod plocks;
mod roundtrip;
mod safe_write;
mod sound;
mod swing;
mod trig_cond;
mod trig_write;
