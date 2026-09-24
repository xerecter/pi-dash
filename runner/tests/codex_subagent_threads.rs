//! Codex multi-agent runs: sub-agent threads share the app-server stream
//! with the run's own thread. Only the run's own thread may end the run.
//!
//! Regression: a sub-agent interrupted by its parent emitted `turn/completed`
//! on its own thread, and the runner failed the whole run with
//! "turn/completed without conclusion" while the main thread kept working.

use pidash::codex::app_server::AppServer;
use pidash::codex::bridge::{Bridge, BridgeCursor, BridgeEvent, RunPayload};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use uuid::Uuid;

const MAIN: &str = "th_main";
const SUB: &str = "th_sub";

/// Fake `codex app-server`: completes the handshake on thread `th_main`,
/// then prints `frames` (one JSON-RPC notification per line) and idles.
fn fake_codex(frames: &[String]) -> Command {
    let mut script = format!(
        r#"
        read _
        printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{}}}}'
        read _
        read _
        printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"threadId":"{MAIN}"}}}}'
        read _
        "#
    );
    for f in frames {
        script.push_str(&format!("printf '%s\\n' '{f}'\n"));
    }
    script.push_str("sleep 2\n");
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(script);
    cmd
}

fn turn_completed(thread: &str, status: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"turn/completed","params":{{"threadId":"{thread}","turn":{{"id":"t_{thread}","status":"{status}","error":null}}}}}}"#
    )
}

fn fatal_error(thread: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","method":"error","params":{{"threadId":"{thread}","turnId":"t_{thread}","willRetry":false,"error":{{"message":"boom on {thread}"}}}}}}"#
    )
}

async fn start(frames: &[String]) -> (Bridge, BridgeCursor) {
    let server = AppServer::spawn_command(fake_codex(frames))
        .await
        .expect("spawn fake codex");
    let mut bridge = Bridge::from_server(server, None);
    let payload = RunPayload {
        run_id: Uuid::new_v4(),
        prompt: "build the portal".into(),
        model: None,
    };
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cursor = bridge.run(&payload, &cwd).await.expect("bridge run setup");
    assert_eq!(cursor.thread_id, MAIN);
    (bridge, cursor)
}

/// Translate frames until the first terminal event (or the stream idles),
/// returning every event seen along the way.
async fn events_until_terminal(bridge: &mut Bridge, cursor: &mut BridgeCursor) -> Vec<BridgeEvent> {
    let mut seen = Vec::new();
    while let Ok(Some(frame)) =
        tokio::time::timeout(Duration::from_secs(1), bridge.next_frame()).await
    {
        for ev in cursor.translate(frame) {
            let terminal = matches!(
                ev,
                BridgeEvent::Completed { .. } | BridgeEvent::Failed { .. }
            );
            seen.push(ev);
            if terminal {
                return seen;
            }
        }
    }
    seen
}

fn terminal(events: &[BridgeEvent]) -> Option<&BridgeEvent> {
    events.iter().find(|e| {
        matches!(
            e,
            BridgeEvent::Completed { .. } | BridgeEvent::Failed { .. }
        )
    })
}

#[tokio::test]
async fn sub_agent_turn_end_does_not_end_the_run() {
    let (mut bridge, mut cursor) = start(&[
        turn_completed(SUB, "interrupted"),
        turn_completed(SUB, "completed"),
        turn_completed(MAIN, "completed"),
    ])
    .await;
    let events = events_until_terminal(&mut bridge, &mut cursor).await;

    // Both sub-agent frames pass through as plain events...
    let raw_turn_ends = events
        .iter()
        .filter(|e| matches!(e, BridgeEvent::Raw { method, .. } if method == "turn/completed"))
        .count();
    assert_eq!(raw_turn_ends, 2, "{events:?}");
    // ...and only the main thread's turn completes the run.
    assert!(
        matches!(terminal(&events), Some(BridgeEvent::Completed { .. })),
        "{events:?}"
    );
}

#[tokio::test]
async fn sub_agent_fatal_error_does_not_fail_the_run() {
    let (mut bridge, mut cursor) =
        start(&[fatal_error(SUB), turn_completed(MAIN, "completed")]).await;
    let events = events_until_terminal(&mut bridge, &mut cursor).await;
    assert!(
        matches!(terminal(&events), Some(BridgeEvent::Completed { .. })),
        "{events:?}"
    );
}

#[tokio::test]
async fn main_thread_fatal_error_still_fails_the_run() {
    let (mut bridge, mut cursor) = start(&[fatal_error(MAIN)]).await;
    let events = events_until_terminal(&mut bridge, &mut cursor).await;
    match terminal(&events) {
        Some(BridgeEvent::Failed { detail, .. }) => {
            assert_eq!(detail.as_deref(), Some("boom on th_main"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn main_thread_interrupted_turn_still_fails_the_run() {
    let (mut bridge, mut cursor) = start(&[turn_completed(MAIN, "interrupted")]).await;
    let events = events_until_terminal(&mut bridge, &mut cursor).await;
    assert!(
        matches!(terminal(&events), Some(BridgeEvent::Failed { .. })),
        "{events:?}"
    );
}

#[tokio::test]
async fn turn_completed_without_thread_id_is_attributed_to_the_run() {
    // Older codex builds omit `threadId`; keep treating those as terminal.
    let frame = r#"{"jsonrpc":"2.0","method":"turn/completed","params":{"conclusion":"success"}}"#;
    let (mut bridge, mut cursor) = start(&[frame.to_string()]).await;
    let events = events_until_terminal(&mut bridge, &mut cursor).await;
    assert!(
        matches!(terminal(&events), Some(BridgeEvent::Completed { .. })),
        "{events:?}"
    );
}
