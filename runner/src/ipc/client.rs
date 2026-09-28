//! IPC transport client.
//!
//! Moved to the shared `karma-team-ipc` crate (PDASHOSS01-158) and re-exported
//! here so `crate::ipc::client::*` call sites inside the runner are unchanged.
pub use karma_team_ipc::client::*;
