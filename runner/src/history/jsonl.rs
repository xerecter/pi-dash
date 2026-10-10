use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

use crate::util::paths::RunnerPaths;

/// One record per line; lines are append-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryEntry {
    Header {
        run_id: Uuid,
        work_item_id: Option<Uuid>,
        prompt_preview: String,
        started_at: DateTime<Utc>,
        repo_url: Option<String>,
        /// Agent CLI that drove this run, as the stable wire label
        /// (`codex`, `claude_code`, `karma`, …). Written from the runner's
        /// config when the run starts, so the transcript self-documents which
        /// tool was invoked. Absent in logs written by older runners.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_kind: Option<String>,
    },
    Lifecycle {
        ts: DateTime<Utc>,
        state: String,
        detail: Option<String>,
    },
    CodexEvent {
        ts: DateTime<Utc>,
        method: String,
        params: serde_json::Value,
    },
    KarmaEvent {
        ts: DateTime<Utc>,
        method: String,
        params: serde_json::Value,
    },
    Approval {
        ts: DateTime<Utc>,
        approval_id: String,
        status: String,
        payload: serde_json::Value,
    },
    Footer {
        ts: DateTime<Utc>,
        final_status: String,
        done_payload: Option<serde_json::Value>,
        error: Option<String>,
    },
}

pub struct HistoryWriter {
    file: File,
    path: PathBuf,
}

impl HistoryWriter {
    pub async fn open(paths: &RunnerPaths, run_id: Uuid) -> Result<Self> {
        paths.ensure()?;
        let path = paths.runs_dir().join(format!("{run_id}.jsonl"));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .with_context(|| format!("opening {path:?}"))?;
        Ok(Self { file, path })
    }

    pub async fn append(&mut self, entry: &HistoryEntry) -> Result<()> {
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        self.file.write_all(&line).await?;
        // Flush so a daemon crash doesn't truncate the run transcript.
        // We use `flush` rather than `sync_data` to keep latency low; the
        // OS buffer survives a process crash, just not a kernel/host crash.
        self.file.flush().await?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub async fn read_all(path: &Path) -> Result<Vec<HistoryEntry>> {
    let file = File::open(path).await?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();
    let mut out = Vec::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).await?;
        if n == 0 {
            break;
        }
        let line = buf.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryEntry>(line) {
            Ok(e) => out.push(e),
            Err(err) => tracing::warn!("skipping bad history line in {path:?}: {err}"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_without_agent_kind_still_parses() {
        // Transcripts written before the field existed must keep loading.
        let line = r#"{"kind":"header","run_id":"00000000-0000-0000-0000-000000000000","work_item_id":null,"prompt_preview":"x","started_at":"2026-01-01T00:00:00Z","repo_url":null}"#;
        match serde_json::from_str::<HistoryEntry>(line).unwrap() {
            HistoryEntry::Header { agent_kind, .. } => assert!(agent_kind.is_none()),
            other => panic!("expected Header, got {other:?}"),
        }
    }

    #[test]
    fn header_serialises_agent_kind_when_present() {
        let entry = HistoryEntry::Header {
            run_id: Uuid::nil(),
            work_item_id: None,
            prompt_preview: "x".into(),
            started_at: Utc::now(),
            repo_url: None,
            agent_kind: Some("karma".into()),
        };
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            value.get("agent_kind").and_then(|v| v.as_str()),
            Some("karma")
        );
    }

    #[test]
    fn karma_events_use_the_karma_history_kind() {
        let entry = HistoryEntry::KarmaEvent {
            ts: Utc::now(),
            method: "system/status".into(),
            params: serde_json::json!({"status": "requesting"}),
        };

        let value = serde_json::to_value(entry).unwrap();
        assert_eq!(
            value.get("kind").and_then(|v| v.as_str()),
            Some("karma_event")
        );
    }
}
