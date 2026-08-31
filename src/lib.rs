//! sleepless — keep the machine awake for exactly as long as this process lives.
//!
//! Everything lives in the library so it can be tested from `tests/`; `src/main.rs`
//! is a thin wrapper. The one rule that governs the whole crate is in [`inhibit`]:
//! only process-lifetime-bound mechanisms are allowed, so nothing here ever needs
//! cleanup after the process dies.

pub mod app;
pub mod art;
pub mod ascii;
pub mod cli;
pub mod config;
pub mod inhibit;
pub mod power;
pub mod run;
#[cfg(all(target_os = "linux", feature = "tray"))]
pub mod tray;
pub mod ui;
