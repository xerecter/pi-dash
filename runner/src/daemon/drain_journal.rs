//! Crash-safe journal for terminal run signals the daemon failed to deliver.
//!
//! Every in-flight run is recorded before shutdown delivery is attempted.
//! Confirmed deliveries are removed, while survivors are replayed by the
//! next daemon process so the cloud does not leave the runner permanently
//! busy after a restart.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::cloud::protocol::FailureReason;
use crate::util::paths::Paths;

/// One undelivered `RunFailed` signal, serialized to
/// `data_dir/pending_run_failures.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingRunFailure {
    pub runner_id: Uuid,
    pub run_id: Uuid,
    pub reason: FailureReason,
    pub detail: String,
    /// When the run was failed (stamped at drain time, replayed verbatim).
    pub ended_at: DateTime<Utc>,
    /// When the entry was journaled. Kept for diagnostics; age alone is not
    /// proof that the cloud recovered the run.
    pub recorded_at: DateTime<Utc>,
}

/// Serialize each load-mutate-save sequence. Atomic rename protects an
/// individual write, while this mutex prevents concurrent drain and replay
/// mutations from overwriting each other.
static JOURNAL_MUTATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn journal_path(paths: &Paths) -> PathBuf {
    paths.drain_journal_path()
}

/// Missing and corrupt journals are treated as empty so they cannot prevent
/// the daemon from starting. Corrupt bytes are quarantined before a future
/// write can replace the journal path.
pub fn load(paths: &Paths) -> Vec<PendingRunFailure> {
    let path = journal_path(paths);
    match load_preserving_corrupt(&path) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!("failed to read drain journal {}: {error:#}", path.display());
            Vec::new()
        }
    }
}

/// Merge entries into the journal, replacing any existing entry for the same
/// run so a repeated shutdown drain cannot create duplicates.
pub fn record_all(paths: &Paths, entries: &[PendingRunFailure]) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let _guard = JOURNAL_MUTATION.lock().unwrap_or_else(|e| e.into_inner());
    let mut merged = load_preserving_corrupt(&journal_path(paths))?;
    for entry in entries {
        merged.retain(|current| current.run_id != entry.run_id);
        merged.push(entry.clone());
    }
    save(paths, &merged)
}

/// Remove entries for the supplied runs. The journal file is deleted when no
/// entries remain, keeping the healthy steady state free of stale files.
pub fn remove(paths: &Paths, run_ids: &[Uuid]) -> Result<()> {
    if run_ids.is_empty() {
        return Ok(());
    }
    let _guard = JOURNAL_MUTATION.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = load_preserving_corrupt(&journal_path(paths))?;
    let before = entries.len();
    entries.retain(|entry| !run_ids.contains(&entry.run_id));
    if entries.len() == before {
        return Ok(());
    }
    save(paths, &entries)
}

fn load_preserving_corrupt(path: &std::path::Path) -> Result<Vec<PendingRunFailure>> {
    match read_entries(path) {
        Ok(entries) => Ok(entries),
        Err(error) if error.downcast_ref::<serde_json::Error>().is_some() => {
            let preserved = quarantine_corrupt_journal(path)?;
            tracing::warn!(
                journal = %path.display(),
                preserved = %preserved.display(),
                error = %error,
                "corrupt drain journal was preserved; no pending runs could be recovered"
            );
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    }
}

fn read_entries(path: &std::path::Path) -> Result<Vec<PendingRunFailure>> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read journal {}", path.display()));
        }
    };
    serde_json::from_slice(&raw).context("parse drain journal")
}

fn quarantine_corrupt_journal(path: &std::path::Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("drain journal has no file name: {}", path.display()))?;
    let preserved = path.with_file_name(format!(
        "{}.corrupt-{}",
        name.to_string_lossy(),
        Uuid::new_v4()
    ));
    std::fs::rename(path, &preserved).with_context(|| {
        format!(
            "preserve corrupt drain journal {} as {}",
            path.display(),
            preserved.display()
        )
    })?;
    Ok(preserved)
}

fn save(paths: &Paths, entries: &[PendingRunFailure]) -> Result<()> {
    let path = journal_path(paths);
    if entries.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("remove drain journal {}", path.display()))
            }
        };
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create journal dir {}", parent.display()))?;
    }
    let raw = serde_json::to_vec_pretty(entries).context("serialize drain journal")?;
    let temporary = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)
            .with_context(|| format!("create {}", temporary.display()))?;
        file.write_all(&raw)
            .with_context(|| format!("write {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", temporary.display()))?;
    }
    std::fs::rename(&temporary, &path)
        .with_context(|| format!("rename {} -> {}", temporary.display(), path.display()))?;
    // `sync_all` above makes the file contents durable. Sync the directory as
    // well so the rename itself survives a sudden power loss.
    if let Some(parent) = path.parent() {
        let directory = std::fs::File::open(parent)
            .with_context(|| format!("open journal dir {}", parent.display()))?;
        directory
            .sync_all()
            .with_context(|| format!("sync journal dir {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_for(root: &std::path::Path) -> Paths {
        Paths {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
            runtime_dir: root.join("runtime"),
        }
    }

    fn entry(run_id: Uuid, detail: &str) -> PendingRunFailure {
        PendingRunFailure {
            runner_id: Uuid::new_v4(),
            run_id,
            reason: FailureReason::DaemonRestart,
            detail: detail.to_string(),
            ended_at: Utc::now(),
            recorded_at: Utc::now(),
        }
    }

    #[test]
    fn missing_file_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load(&paths_for(tmp.path())).is_empty());
    }

    #[test]
    fn record_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let expected = entry(Uuid::new_v4(), "daemon shutdown requested");
        record_all(&paths, std::slice::from_ref(&expected)).unwrap();
        assert_eq!(load(&paths), vec![expected]);
    }

    #[test]
    fn record_replaces_same_run_id() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let run_id = Uuid::new_v4();
        record_all(&paths, &[entry(run_id, "first")]).unwrap();
        record_all(&paths, &[entry(run_id, "second")]).unwrap();
        let loaded = load(&paths);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].detail, "second");
    }

    #[test]
    fn remove_to_empty_deletes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        let expected = entry(Uuid::new_v4(), "x");
        record_all(&paths, std::slice::from_ref(&expected)).unwrap();
        assert!(journal_path(&paths).exists());
        remove(&paths, &[expected.run_id]).unwrap();
        assert!(!journal_path(&paths).exists());
        assert!(load(&paths).is_empty());
    }

    #[test]
    fn corrupt_file_loads_empty_and_preserves_original_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        std::fs::create_dir_all(paths.data_dir.clone()).unwrap();
        let corrupt = b"not json{{{";
        std::fs::write(journal_path(&paths), corrupt).unwrap();

        assert!(load(&paths).is_empty());

        let preserved = preserved_corrupt_files(&paths);
        assert_eq!(preserved.len(), 1);
        assert_eq!(std::fs::read(&preserved[0]).unwrap(), corrupt);
        assert!(!journal_path(&paths).exists());
    }

    #[test]
    fn record_all_quarantines_corruption_before_writing_new_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        std::fs::create_dir_all(paths.data_dir.clone()).unwrap();
        let corrupt = b"not json{{{";
        std::fs::write(journal_path(&paths), corrupt).unwrap();
        let expected = entry(Uuid::new_v4(), "new failure");

        record_all(&paths, std::slice::from_ref(&expected)).unwrap();

        assert_eq!(load(&paths), vec![expected]);
        let preserved = preserved_corrupt_files(&paths);
        assert_eq!(preserved.len(), 1);
        assert_eq!(std::fs::read(&preserved[0]).unwrap(), corrupt);
    }

    #[test]
    fn remove_does_not_overwrite_corrupt_journal() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_for(tmp.path());
        std::fs::create_dir_all(paths.data_dir.clone()).unwrap();
        let corrupt = b"not json{{{";
        std::fs::write(journal_path(&paths), corrupt).unwrap();

        remove(&paths, &[Uuid::new_v4()]).unwrap();

        let preserved = preserved_corrupt_files(&paths);
        assert_eq!(preserved.len(), 1);
        assert_eq!(std::fs::read(&preserved[0]).unwrap(), corrupt);
        assert!(!journal_path(&paths).exists());
    }

    fn preserved_corrupt_files(paths: &Paths) -> Vec<PathBuf> {
        std::fs::read_dir(&paths.data_dir)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().contains(".corrupt-"))
            })
            .collect()
    }
}
