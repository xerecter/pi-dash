pub mod chat;
pub mod client;
pub mod protocol;
pub mod server;

// The Windows named-pipe derivation moved to `karma-team-ipc` alongside the
// transport client; the IPC *server* (still in the runner) needs the
// identical derivation, so re-export it under its original path.
#[cfg(windows)]
pub(crate) use karma_team_ipc::windows_pipe_name;
