//! Subprocess groups owned by one runner operation, never the daemon itself.

use std::process::{Output, Stdio};
use tokio::process::Command;

pub(crate) fn kill_process_group(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        if let Err(error) = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL)
            && error != nix::errno::Errno::ESRCH
        {
            tracing::warn!(%pid, %error, "failed to stop runner-owned process group");
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

struct ProcessGroup(Option<u32>);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        kill_process_group(self.0);
    }
}

/// Collect output and stop descendants as well as the command on cancellation.
pub(crate) async fn command_output(mut command: Command) -> std::io::Result<Output> {
    #[cfg(unix)]
    command.process_group(0);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let child = command.spawn()?;
    let _group = ProcessGroup(child.id());
    child.wait_with_output().await
}
