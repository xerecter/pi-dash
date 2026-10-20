// Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// See the LICENSE file for details.

//! `karma-team workpad …` subcommands.
//!
//! The workpad is the coding agent's durable per-issue scratchpad. It used
//! to live in a dedicated `## Agent Workpad` IssueComment; with the comment
//! thread now reserved for human ↔ agent conversation, the workpad is a
//! plain markdown field on the issue itself and these commands are the
//! agent's read/write channel.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde_json::json;
use uuid::Uuid;

use crate::api_client::{ApiClient, CliEnv, CliError, EXIT_INVALID, EXIT_UNKNOWN, report_error};

use super::resolve::resolve_issue;

#[derive(Debug, Args)]
pub struct WorkpadArgs {
    #[command(subcommand)]
    pub command: WorkpadCommand,
}

#[derive(Debug, Subcommand)]
pub enum WorkpadCommand {
    /// Fetch the agent workpad for a work item.
    ///
    /// Defaults the work-item identifier to `KARMA_TEAM_ISSUE_IDENTIFIER` when
    /// omitted, so the agent can call `karma-team workpad get` with no args.
    Get {
        /// Work item identifier, e.g. `ENG-42`. Defaults to
        /// `KARMA_TEAM_ISSUE_IDENTIFIER`.
        identifier: Option<String>,
    },
    /// Overwrite the agent workpad. An empty file clears it.
    ///
    /// A successful upload deletes the body file so a stale local copy cannot
    /// overwrite newer server content. Pass `--keep` to retain it.
    Update {
        /// Work item identifier, e.g. `ENG-42`. Defaults to
        /// `KARMA_TEAM_ISSUE_IDENTIFIER`.
        identifier: Option<String>,
        /// Path to a file containing the workpad body (markdown).
        #[arg(long = "body-file", value_name = "PATH")]
        body_file: PathBuf,
        /// Keep the body file after a successful upload.
        #[arg(long = "keep")]
        keep: bool,
    },
}

pub async fn run(args: WorkpadArgs, paths: &crate::util::paths::Paths) -> i32 {
    let env = match CliEnv::resolve(paths) {
        Ok(e) => e,
        Err(e) => return report_error(&e),
    };
    let client = match ApiClient::new(env) {
        Ok(c) => c,
        Err(e) => return report_error(&CliError::new(EXIT_UNKNOWN, format!("{e}"))),
    };

    let result = match args.command {
        WorkpadCommand::Get { identifier } => cmd_get(&client, identifier).await,
        WorkpadCommand::Update {
            identifier,
            body_file,
            keep,
        } => cmd_update(&client, identifier, body_file, keep).await,
    };
    match result {
        Ok(()) => 0,
        Err(e) => report_error(&e),
    }
}

async fn cmd_get(client: &ApiClient, identifier: Option<String>) -> Result<(), CliError> {
    let current_issue = std::env::var("KARMA_TEAM_ISSUE_IDENTIFIER").ok();
    let ident = resolve_identifier(identifier.as_deref(), current_issue.as_deref())?;
    let issue = resolve_issue(client, ident).await?;
    let path = format!(
        "workspaces/{}/projects/{}/work-items/{}/workpad/",
        client.env.workspace_slug, issue.project_id, issue.id
    );
    let resp = client.get(&path).await?;
    println!(
        "{}",
        serde_json::to_string(&resp).expect("serialize JSON value")
    );
    Ok(())
}

async fn cmd_update(
    client: &ApiClient,
    identifier: Option<String>,
    body_file: PathBuf,
    keep: bool,
) -> Result<(), CliError> {
    let body = load_workpad_body(&body_file)?;
    let current_issue = std::env::var("KARMA_TEAM_ISSUE_IDENTIFIER").ok();
    let ident = resolve_identifier(identifier.as_deref(), current_issue.as_deref())?;
    let issue = resolve_issue(client, ident).await?;
    let path = format!(
        "workspaces/{}/projects/{}/work-items/{}/workpad/",
        client.env.workspace_slug, issue.project_id, issue.id
    );
    let resp = client.patch(&path, &json!({ "body": &body })).await?;
    println!(
        "{}",
        serde_json::to_string(&resp).expect("serialize JSON value")
    );
    if let Some(warning) = cleanup_warning(cleanup_body_file(&body_file, keep, body.as_bytes())) {
        eprintln!("{warning}");
    }
    Ok(())
}

#[derive(Debug)]
enum Cleanup {
    Deleted,
    Kept,
    Skipped,
    Changed(String),
    Failed(String),
}

/// Delete only the same plain regular staging file contents the server accepted.
/// Symlinks, directories, special files, stdin, missing paths, and a file that
/// changed while the request was in flight are never removed. Cleanup errors
/// remain warnings because the upload already won.
fn cleanup_body_file(path: &Path, keep: bool, uploaded: &[u8]) -> Cleanup {
    if keep {
        return Cleanup::Kept;
    }
    if path.as_os_str() == "-" {
        return Cleanup::Skipped;
    }
    let _metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) | Err(_) => return Cleanup::Skipped,
    };
    let Some(parent) = path.parent() else {
        return Cleanup::Skipped;
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let quarantine = parent.join(format!(".{name}.karma-team-uploaded-{}", Uuid::new_v4()));
    if let Err(error) = std::fs::rename(path, &quarantine) {
        return Cleanup::Failed(format!(
            "could not isolate workpad body file {} before cleanup: {error}",
            path.display()
        ));
    }

    match std::fs::read(&quarantine) {
        Ok(current) if current == uploaded => match std::fs::remove_file(&quarantine) {
            Ok(()) => Cleanup::Deleted,
            Err(error) => Cleanup::Failed(format!(
                "could not delete uploaded workpad body file {}: {error}",
                quarantine.display()
            )),
        },
        Ok(_) => {
            let restored = !path.exists() && std::fs::rename(&quarantine, path).is_ok();
            let location = if restored { path } else { &quarantine };
            Cleanup::Changed(format!(
                "workpad body file {} changed while the upload was in flight; retained the newer local contents at {}",
                path.display(),
                location.display()
            ))
        }
        Err(error) => {
            // The rename already detached the file from its familiar path. Put
            // it back when possible; otherwise report the quarantine location.
            let restored = !path.exists() && std::fs::rename(&quarantine, path).is_ok();
            let location = if restored { path } else { &quarantine };
            Cleanup::Failed(format!(
                "could not verify workpad body file {} before cleanup: {error}; retained it at {}",
                path.display(),
                location.display()
            ))
        }
    }
}

fn cleanup_warning(outcome: Cleanup) -> Option<String> {
    match outcome {
        Cleanup::Changed(message) | Cleanup::Failed(message) => Some(format!("warning: {message}")),
        Cleanup::Deleted | Cleanup::Kept | Cleanup::Skipped => None,
    }
}

fn resolve_identifier<'a>(
    explicit: Option<&'a str>,
    current_issue: Option<&'a str>,
) -> Result<&'a str, CliError> {
    explicit.or(current_issue).ok_or_else(|| {
        CliError::new(
            EXIT_INVALID,
            "workpad requires an issue identifier or KARMA_TEAM_ISSUE_IDENTIFIER",
        )
    })
}

fn load_workpad_body(path: &Path) -> Result<String, CliError> {
    std::fs::read_to_string(path).map_err(|e| {
        CliError::new(
            EXIT_UNKNOWN,
            format!("failed reading workpad body file {}: {e}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        Cleanup, cleanup_body_file, cleanup_warning, load_workpad_body, resolve_identifier,
    };

    #[test]
    fn resolve_prefers_explicit_arg() {
        let target = resolve_identifier(Some("ENG-42"), Some("ENG-7")).expect("target");
        assert_eq!(target, "ENG-42");
    }

    #[test]
    fn resolve_falls_back_to_env() {
        let target = resolve_identifier(None, Some("ENG-7")).expect("target");
        assert_eq!(target, "ENG-7");
    }

    #[test]
    fn resolve_requires_context() {
        let err = resolve_identifier(None, None).expect_err("missing target");
        assert_eq!(
            err.message,
            "workpad requires an issue identifier or KARMA_TEAM_ISSUE_IDENTIFIER"
        );
    }

    #[test]
    fn load_workpad_body_reads_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pad.md");
        std::fs::write(&path, "## Agent Workpad\n\nphase: implementing\n").expect("write file");

        let body = load_workpad_body(&path).expect("body");
        assert!(body.contains("phase: implementing"));
    }

    #[test]
    fn cleanup_deletes_regular_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pad.md");
        std::fs::write(&path, "body").expect("write file");

        let outcome = cleanup_body_file(&path, false, b"body");

        assert!(matches!(outcome, Cleanup::Deleted));
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_keep_preserves_regular_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pad.md");
        std::fs::write(&path, "body").expect("write file");

        let outcome = cleanup_body_file(&path, true, b"body");

        assert!(matches!(outcome, Cleanup::Kept));
        assert!(path.exists());
    }

    #[test]
    fn cleanup_skips_stdin_directories_and_missing_paths() {
        let dir = tempfile::tempdir().expect("tempdir");

        assert!(matches!(
            cleanup_body_file(Path::new("-"), false, b"body"),
            Cleanup::Skipped
        ));
        assert!(matches!(
            cleanup_body_file(dir.path(), false, b"body"),
            Cleanup::Skipped
        ));
        assert!(matches!(
            cleanup_body_file(&dir.path().join("missing.md"), false, b"body"),
            Cleanup::Skipped
        ));
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_skips_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target.md");
        let link = dir.path().join("link.md");
        std::fs::write(&target, "body").expect("write target");
        symlink(&target, &link).expect("create symlink");

        assert!(matches!(
            cleanup_body_file(&link, false, b"body"),
            Cleanup::Skipped
        ));
        assert!(link.exists());
        assert!(target.exists());
    }

    #[test]
    fn cleanup_failure_becomes_warning() {
        let warning = cleanup_warning(Cleanup::Failed("permission denied".to_string()))
            .expect("failure should warn");

        assert_eq!(warning, "warning: permission denied");
        assert!(cleanup_warning(Cleanup::Skipped).is_none());
    }

    #[test]
    fn cleanup_retains_file_changed_during_upload() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pad.md");
        std::fs::write(&path, "newer local body").expect("write file");

        let outcome = cleanup_body_file(&path, false, b"uploaded body");

        let warning = cleanup_warning(outcome).expect("changed file should warn");
        assert!(warning.contains("changed while the upload was in flight"));
        assert_eq!(
            std::fs::read_to_string(path).expect("changed file must remain"),
            "newer local body"
        );
    }
}
