//! NotroDNS library crate: everything except the `main()` entry point
//! lives here, so it can be exercised by integration tests in `tests/`
//! as well as by the `notrodns` binary (`src/main.rs`).

pub mod api;
pub mod app;
pub mod bench;
pub mod cache;
pub mod config;
pub mod error;
pub mod restore_state;
pub mod windows;
