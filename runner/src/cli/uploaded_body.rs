// Copyright (c) 2023-present Pi Dash Software, Inc. and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// See the LICENSE file for details.

use std::path::Path;

use uuid::Uuid;

#[derive(Debug)]
pub(super) enum Cleanup {
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
pub(super) fn cleanup_body_file(path: &Path, keep: bool, uploaded: &[u8]) -> Cleanup {
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
            "could not isolate body file {} before cleanup: {error}",
            path.display()
        ));
    }

    match std::fs::read(&quarantine) {
        Ok(current) if current == uploaded => match std::fs::remove_file(&quarantine) {
            Ok(()) => Cleanup::Deleted,
            Err(error) => Cleanup::Failed(format!(
                "could not delete uploaded body file {}: {error}",
                quarantine.display()
            )),
        },
        Ok(_) => {
            let restored = !path.exists() && std::fs::rename(&quarantine, path).is_ok();
            let location = if restored { path } else { &quarantine };
            Cleanup::Changed(format!(
                "body file {} changed while the upload was in flight; retained the newer local contents at {}",
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
                "could not verify body file {} before cleanup: {error}; retained it at {}",
                path.display(),
                location.display()
            ))
        }
    }
}

pub(super) fn cleanup_warning(outcome: Cleanup) -> Option<String> {
    match outcome {
        Cleanup::Changed(message) | Cleanup::Failed(message) => Some(format!("warning: {message}")),
        Cleanup::Deleted | Cleanup::Kept | Cleanup::Skipped => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Cleanup, cleanup_body_file, cleanup_warning};
    use std::path::Path;

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
