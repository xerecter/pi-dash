//! Cancellation must stop Karma's wrapper and its Claude/tool descendants.
#![cfg(target_os = "linux")]

use karma_team::claude_code::process::ClaudeProcess;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

fn running(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(") ")
                .map(|(_, rest)| !rest.starts_with('Z'))
        })
        .unwrap_or(false)
}

async fn wait_for_pid(path: &Path) -> u32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(text) = std::fs::read_to_string(path)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child did not start")
}

async fn assert_stopped(pid: u32) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while running(pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("agent descendant survived cancellation");
}

async fn fake_wrapper(root: &Path, ignores_interrupt: bool) -> ClaudeProcess {
    let mut command = Command::new("sh");
    command.current_dir(root).args(["-c", if ignores_interrupt {
        "trap '' INT TERM; sh -c 'trap \"\" INT TERM; echo $$ > child.pid; exec sleep 300' & wait"
    } else {
        "sh -c 'trap \"\" INT TERM; echo $$ > child.pid; exec sleep 300' & wait"
    }]);
    ClaudeProcess::spawn_command(command).await.unwrap()
}

#[tokio::test]
async fn abort_stops_child_even_when_wrapper_exits_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut process = fake_wrapper(dir.path(), false).await;
    let parent = process.process_handle().pid.unwrap();
    let child = wait_for_pid(&dir.path().join("child.pid")).await;
    process.interrupt().await.unwrap();
    process.shutdown(Duration::from_millis(100)).await.unwrap();
    assert_stopped(parent).await;
    assert_stopped(child).await;
}

#[tokio::test]
async fn abort_force_kills_group_that_ignores_interrupt() {
    let dir = tempfile::tempdir().unwrap();
    let mut process = fake_wrapper(dir.path(), true).await;
    let parent = process.process_handle().pid.unwrap();
    let child = wait_for_pid(&dir.path().join("child.pid")).await;
    process.interrupt().await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        process.shutdown(Duration::from_millis(100)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_stopped(parent).await;
    assert_stopped(child).await;
}

#[tokio::test]
async fn dropped_startup_stops_owned_processes() {
    let dir = tempfile::tempdir().unwrap();
    let process = fake_wrapper(dir.path(), true).await;
    let parent = process.process_handle().pid.unwrap();
    let child = wait_for_pid(&dir.path().join("child.pid")).await;
    drop(process);
    assert_stopped(parent).await;
    assert_stopped(child).await;
}
