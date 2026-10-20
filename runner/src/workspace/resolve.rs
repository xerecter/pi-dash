use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

use crate::workspace::git;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("io error: {0}")]
    Io(#[source] std::io::Error),
}

#[derive(Debug, Clone)]
pub enum Resolution {
    /// Ordinary task folder: Git is optional, and existing files are retained.
    Directory(PathBuf),
    ExistingRepo(PathBuf),
    Cloned(PathBuf),
}

/// Resolve the directory the agent runs in.
///
/// Git is optional context rather than a prerequisite for an assignment. A
/// repository URL is used as a best-effort bootstrap only when the directory
/// contains no user files. Unsupported URLs, populated task folders, and clone
/// failures all leave the agent a usable ordinary directory. Only filesystem
/// failures that make the working directory unusable stop the assignment.
pub async fn resolve(
    working_dir: &Path,
    repo_url: Option<&str>,
) -> Result<Resolution, ResolveError> {
    if let Err(e) = std::fs::create_dir_all(working_dir) {
        return Err(ResolveError::Io(e));
    }
    let dir = working_dir.to_path_buf();
    // `create_dir_all` succeeds for an existing directory regardless of
    // whether it can be read. Probe it so a real filesystem failure does not
    // get mistaken for a populated task folder and passed to the agent.
    if let Err(e) = std::fs::read_dir(working_dir) {
        return Err(ResolveError::Io(e));
    }
    if git::is_git_repo(working_dir) {
        return Ok(Resolution::ExistingRepo(dir));
    }
    let Some(url) = repo_url.filter(|url| !url.trim().is_empty()) else {
        return Ok(Resolution::Directory(dir));
    };
    if !is_bootstrappable(working_dir).map_err(ResolveError::Io)? {
        tracing::info!(
            working_dir = ?dir,
            "working dir is not a git repo and contains user files; running it as a task folder without cloning"
        );
        return Ok(Resolution::Directory(dir));
    }
    if !is_supported_clone_url(url) {
        tracing::warn!(
            working_dir = ?dir,
            // Log only the shape because a URL can contain credentials.
            scheme = url_shape(url),
            "repo_url is not a supported clone URL; running as a task folder without cloning"
        );
        return Ok(Resolution::Directory(dir));
    }
    // Clone beside the destination, then publish the complete checkout. A
    // failed clone never writes into the directory users and agents can see,
    // so cleanup cannot race a newly-created workpad or other user file.
    let staging = clone_staging_path(working_dir).map_err(ResolveError::Io)?;
    match git::clone(url, &staging).await {
        Ok(()) => match publish_clone(&staging, working_dir) {
            Ok(()) => Ok(Resolution::Cloned(dir)),
            Err(PublishCloneError::DestinationChanged) => {
                if let Err(cleanup) = remove_entry(&staging) {
                    tracing::warn!(
                        staging_dir = ?staging,
                        error = %cleanup,
                        "could not remove an unpublished clone staging directory"
                    );
                }
                tracing::warn!(
                    working_dir = ?dir,
                    "working dir changed while clone bootstrap was in progress; retained those files and will run as a task folder"
                );
                Ok(Resolution::Directory(dir))
            }
            Err(PublishCloneError::Io(error)) => {
                let _ = remove_entry(&staging);
                Err(ResolveError::Io(error))
            }
        },
        Err(error) => {
            if let Err(cleanup) = remove_entry(&staging) {
                tracing::warn!(
                    staging_dir = ?staging,
                    error = %cleanup,
                    "could not remove failed clone staging directory"
                );
            }
            tracing::warn!(
                working_dir = ?dir,
                error = %sanitize_clone_error(&error),
                "clone bootstrap failed; running as a task folder so the agent can handle Git"
            );
            Ok(Resolution::Directory(dir))
        }
    }
}

/// Metadata written by the Runner does not make a directory user-owned. The
/// workpad staging file deliberately is not listed: until its upload succeeds,
/// it is user data and must block clone bootstrap.
const RUNNER_OWNED_ENTRIES: &[&str] = &[".karma-team"];

fn is_bootstrappable(path: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(path)? {
        let name = entry?.file_name();
        if !RUNNER_OWNED_ENTRIES.iter().any(|owned| name == *owned) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn clone_staging_path(path: &Path) -> std::io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "clone destination has no parent directory",
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "clone destination has no file name",
        )
    })?;
    Ok(parent.join(format!(
        ".{}.karma-team-clone-{}",
        file_name.to_string_lossy(),
        Uuid::new_v4()
    )))
}

#[derive(Debug)]
enum PublishCloneError {
    DestinationChanged,
    Io(std::io::Error),
}

fn publish_clone(staging: &Path, destination: &Path) -> Result<(), PublishCloneError> {
    if !is_bootstrappable(destination).map_err(PublishCloneError::Io)? {
        return Err(PublishCloneError::DestinationChanged);
    }

    // Move only known Runner-owned metadata aside, then remove the destination
    // with `remove_dir`. The latter is the race-safe guard: it fails instead of
    // deleting anything if a user or another process created a file since the
    // preflight above.
    let metadata = destination.join(RUNNER_OWNED_ENTRIES[0]);
    let metadata_backup = staging.with_extension("runner-metadata");
    let metadata_was_moved = match std::fs::symlink_metadata(&metadata) {
        Ok(_) => {
            std::fs::rename(&metadata, &metadata_backup).map_err(PublishCloneError::Io)?;
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(PublishCloneError::Io(error)),
    };

    if let Err(error) = std::fs::remove_dir(destination) {
        restore_runner_metadata(destination, &metadata, &metadata_backup, metadata_was_moved);
        if matches!(
            error.kind(),
            std::io::ErrorKind::DirectoryNotEmpty | std::io::ErrorKind::AlreadyExists
        ) {
            return Err(PublishCloneError::DestinationChanged);
        }
        return Err(PublishCloneError::Io(error));
    }

    if let Err(error) = std::fs::rename(staging, destination) {
        if !destination.exists() {
            let _ = std::fs::create_dir(destination);
        }
        restore_runner_metadata(destination, &metadata, &metadata_backup, metadata_was_moved);
        if destination.exists() {
            return Err(PublishCloneError::DestinationChanged);
        }
        return Err(PublishCloneError::Io(error));
    }

    if metadata_was_moved {
        if let Err(error) = merge_runner_metadata(&metadata_backup, &metadata) {
            tracing::warn!(
                metadata_backup = ?metadata_backup,
                working_dir = ?destination,
                error = %error,
                "could not merge prior runner metadata after clone publication; preserved it at the backup path"
            );
        }
    }
    Ok(())
}

fn restore_runner_metadata(
    destination: &Path,
    metadata: &Path,
    metadata_backup: &Path,
    metadata_was_moved: bool,
) {
    if !metadata_was_moved {
        return;
    }
    let result = match std::fs::symlink_metadata(metadata) {
        Ok(_) => merge_runner_metadata(metadata_backup, metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::rename(metadata_backup, metadata)
        }
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        tracing::warn!(
            working_dir = ?destination,
            metadata_backup = ?metadata_backup,
            error = %error,
            "could not restore runner metadata after clone publication was abandoned"
        );
    }
}

fn merge_runner_metadata(source: &Path, destination: &Path) -> std::io::Result<()> {
    let source_metadata = std::fs::symlink_metadata(source)?;
    let destination_metadata = match std::fs::symlink_metadata(destination) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return std::fs::rename(source, destination);
        }
        Err(error) => return Err(error),
    };
    if !source_metadata.file_type().is_dir() || !destination_metadata.file_type().is_dir() {
        return preserve_metadata_conflict(source, destination);
    }

    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_entry = entry.path();
        let destination_entry = destination.join(entry.file_name());
        let source_entry_metadata = std::fs::symlink_metadata(&source_entry)?;
        let destination_entry_metadata = match std::fs::symlink_metadata(&destination_entry) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if destination_entry_metadata.is_none() {
            std::fs::rename(&source_entry, &destination_entry)?;
        } else if entry.file_name() == "context.md"
            && source_entry_metadata.file_type().is_file()
            && destination_entry_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.file_type().is_file())
        {
            let local = std::fs::read_to_string(&source_entry)?;
            let cloned = std::fs::read_to_string(&destination_entry)?;
            if let Some(merged) = crate::cli::context::merge_context_documents(&local, &cloned) {
                std::fs::write(&destination_entry, merged)?;
                std::fs::remove_file(source_entry)?;
            } else {
                preserve_metadata_conflict(&source_entry, &destination_entry)?;
            }
        } else if source_entry_metadata.file_type().is_dir()
            && destination_entry_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.file_type().is_dir())
        {
            merge_runner_metadata(&source_entry, &destination_entry)?;
        } else {
            preserve_metadata_conflict(&source_entry, &destination_entry)?;
        }
    }

    std::fs::remove_dir(source)
}

fn preserve_metadata_conflict(source: &Path, destination: &Path) -> std::io::Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| std::io::Error::other("runner metadata has no parent directory"))?;
    let preserved = parent.join(format!("preserved-runner-metadata-{}", Uuid::new_v4()));
    std::fs::create_dir(&preserved)?;
    let name = source.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "runner metadata entry has no file name",
        )
    })?;
    std::fs::rename(source, preserved.join(name))
}

fn remove_entry(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn sanitize_clone_error(error: &anyhow::Error) -> String {
    let text = error.to_string();
    let mut sanitized = String::with_capacity(text.len());
    let mut rest = text.as_str();
    loop {
        let start = match (rest.find("http://"), rest.find("https://")) {
            (Some(http), Some(https)) => http.min(https),
            (Some(http), None) => http,
            (None, Some(https)) => https,
            (None, None) => break,
        };
        let (prefix, url_and_tail) = rest.split_at(start);
        sanitized.push_str(prefix);
        let end = url_and_tail
            .find(char::is_whitespace)
            .unwrap_or(url_and_tail.len());
        let (url, tail) = url_and_tail.split_at(end);
        let parsed = reqwest::Url::parse(url.trim_matches(['\'', '"']));
        if let Ok(parsed) = parsed {
            sanitized.push_str(parsed.scheme());
            sanitized.push_str("://");
            sanitized.push_str(parsed.host_str().unwrap_or("<host>"));
            if let Some(port) = parsed.port() {
                sanitized.push(':');
                sanitized.push_str(&port.to_string());
            }
            sanitized.push_str("/<redacted>");
            if url.ends_with('\'') {
                sanitized.push('\'');
            } else if url.ends_with('"') {
                sanitized.push('"');
            }
        } else {
            sanitized.push_str("<redacted-url>");
        }
        rest = tail;
    }
    sanitized.push_str(rest);
    sanitized
}

/// Non-secret discriminator for rejected URLs. Never log the URL itself
/// because HTTP userinfo may contain a token.
fn url_shape(url: &str) -> &'static str {
    match url.split_once("://") {
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("https") => "https",
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("http") => "http",
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("ssh") => "ssh",
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("git") => "git",
        Some(_) => "other-scheme",
        None if url.contains('@') => "scp-like",
        None => "no-scheme",
    }
}

/// Defense-in-depth: only hand `git clone` URL forms we expect from the cloud.
/// Rejected forms skip bootstrap instead of failing the assignment.
fn is_supported_clone_url(url: &str) -> bool {
    if url.is_empty() || url.starts_with('-') {
        return false;
    }
    if url.chars().any(|c| c.is_control()) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("ssh://")
        || lower.starts_with("git://")
        || is_scp_clone_url(url)
}

/// Git's scp-like transport accepts any user name (`git@host:path` is only
/// the common convention). Keep the shape narrow enough that a value cannot
/// be interpreted as a command-line option or a local path.
fn is_scp_clone_url(url: &str) -> bool {
    let Some((user_host, path)) = url.split_once(':') else {
        return false;
    };
    let Some((user, host)) = user_host.rsplit_once('@') else {
        return false;
    };
    !user.is_empty()
        && !host.is_empty()
        && !path.is_empty()
        && !host.contains('/')
        && !host.contains('\\')
        && user
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || "._-".contains(char))
        && host
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || "._-".contains(char))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn detects_existing_repo() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        let r = resolve(tmp.path(), None).await.unwrap();
        assert!(matches!(r, Resolution::ExistingRepo(_)));
    }

    #[tokio::test]
    async fn runs_as_task_folder_when_non_empty_and_not_a_repo() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("junk"), b"keep").unwrap();
        let resolution = resolve(tmp.path(), Some("https://example.com/repo.git"))
            .await
            .unwrap();
        assert!(matches!(resolution, Resolution::Directory(_)));
        assert_eq!(std::fs::read(tmp.path().join("junk")).unwrap(), b"keep");
        assert!(!tmp.path().join(".git").exists());
    }

    #[tokio::test]
    async fn runs_as_task_folder_when_working_dir_contains_sibling_repos() {
        let tmp = tempdir().unwrap();
        for repo in ["karma-team", "another-project"] {
            std::fs::create_dir_all(tmp.path().join(repo).join(".git")).unwrap();
        }

        let resolution = resolve(tmp.path(), Some("https://example.com/repo.git"))
            .await
            .unwrap();

        assert!(matches!(resolution, Resolution::Directory(p) if p == tmp.path()));
        assert!(tmp.path().join("karma-team/.git").is_dir());
        assert!(tmp.path().join("another-project/.git").is_dir());
        assert!(!tmp.path().join(".git").exists());
    }

    #[tokio::test]
    async fn accepts_empty_directory_without_repo_url() {
        let tmp = tempdir().unwrap();
        let resolution = resolve(tmp.path(), None).await.unwrap();
        assert!(matches!(resolution, Resolution::Directory(_)));
        assert!(!tmp.path().join(".git").exists());
    }

    #[tokio::test]
    async fn preserves_existing_files_without_repo_url() {
        let tmp = tempdir().unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "user notes").unwrap();
        for url in [None, Some(""), Some("  ")] {
            assert!(matches!(
                resolve(tmp.path(), url).await.unwrap(),
                Resolution::Directory(_)
            ));
            assert_eq!(
                std::fs::read_to_string(tmp.path().join("notes.txt")).unwrap(),
                "user notes"
            );
            assert!(!tmp.path().join(".git").exists());
        }
    }

    #[tokio::test]
    async fn creates_missing_task_directory() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("tasks/new");
        assert!(
            matches!(resolve(&path, None).await.unwrap(), Resolution::Directory(p) if p == path)
        );
        assert!(path.is_dir());
    }

    #[tokio::test]
    async fn rejects_file_as_working_directory() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("notes.txt");
        std::fs::write(&path, "keep").unwrap();
        assert!(matches!(
            resolve(&path, None).await.unwrap_err(),
            ResolveError::Io(_)
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "keep");
    }

    #[tokio::test]
    async fn skips_clone_bootstrap_for_unsupported_repo_url() {
        for url in [
            "--upload-pack=evil",
            "https://x.test/a.git\nrm -rf",
            "ext::sh -c evil",
            "file:///etc",
        ] {
            let tmp = tempdir().unwrap();
            let resolution = resolve(tmp.path(), Some(url)).await.unwrap();
            assert!(
                matches!(resolution, Resolution::Directory(_)),
                "expected task folder for {url:?}"
            );
            assert!(!tmp.path().join(".git").exists(), "cloned for {url:?}");
        }
    }

    #[tokio::test]
    async fn falls_back_to_task_folder_when_clone_fails() {
        let tmp = tempdir().unwrap();
        // Port 1 on loopback refuses immediately without external network I/O.
        let resolution = resolve(tmp.path(), Some("https://127.0.0.1:1/x.git"))
            .await
            .unwrap();
        assert!(matches!(resolution, Resolution::Directory(p) if p == tmp.path()));
        assert!(tmp.path().is_dir());
        assert!(!tmp.path().join(".git").exists());
        assert!(is_bootstrappable(tmp.path()).unwrap());
    }

    #[tokio::test]
    async fn completed_clone_preserves_runner_metadata_when_repository_has_none() {
        let tmp = tempdir().unwrap();
        let source = tmp.path().join("source.git");
        let target = tmp.path().join("target");
        assert!(
            std::process::Command::new("git")
                .args(["init", "--bare", source.to_str().unwrap()])
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::create_dir_all(target.join(".karma-team")).unwrap();
        std::fs::write(target.join(".karma-team/context.md"), "# context").unwrap();

        let staging = clone_staging_path(&target).unwrap();
        git::clone(source.to_str().unwrap(), &staging)
            .await
            .unwrap();
        publish_clone(&staging, &target).unwrap();

        assert!(git::is_git_repo(&target));
        assert_eq!(
            std::fs::read_to_string(target.join(".karma-team/context.md")).unwrap(),
            "# context"
        );
    }

    #[test]
    fn completed_clone_merges_existing_and_cloned_project_context() {
        let tmp = tempdir().unwrap();
        let target = tmp.path().join("target");
        let staging = clone_staging_path(&target).unwrap();
        std::fs::create_dir_all(target.join(".karma-team")).unwrap();
        std::fs::write(
            target.join(".karma-team/context.md"),
            "---\nworkspace_slug: acme\ndefault_project_id: p1\nprojects:\n  - id: p1\n    identifier: WEB\n    name: Website\n    description: \"\"\n    is_default: true\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(staging.join(".git")).unwrap();
        std::fs::create_dir_all(staging.join(".karma-team")).unwrap();
        std::fs::write(
            staging.join(".karma-team/context.md"),
            "---\nworkspace_slug: acme\ndefault_project_id: p2\nprojects:\n  - id: p2\n    identifier: API\n    name: API\n    description: \"\"\n    is_default: true\n---\n",
        )
        .unwrap();

        publish_clone(&staging, &target).unwrap();

        let merged = std::fs::read_to_string(target.join(".karma-team/context.md")).unwrap();
        assert!(merged.contains("identifier: \"WEB\""));
        assert!(merged.contains("identifier: \"API\""));
        assert!(!tmp.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with("runner-metadata")
        }));
    }

    #[test]
    fn failed_clone_publication_merges_metadata_recreated_concurrently() {
        let tmp = tempdir().unwrap();
        let destination = tmp.path().join("target");
        let metadata = destination.join(".karma-team");
        let backup = tmp.path().join("metadata-backup");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(
            backup.join("context.md"),
            "---\nworkspace_slug: acme\ndefault_project_id: p1\nprojects:\n  - id: p1\n    identifier: WEB\n    name: Website\n    description: \"\"\n    is_default: true\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(&metadata).unwrap();
        std::fs::write(
            metadata.join("context.md"),
            "---\nworkspace_slug: acme\ndefault_project_id: p2\nprojects:\n  - id: p2\n    identifier: API\n    name: API\n    description: \"\"\n    is_default: true\n---\n",
        )
        .unwrap();

        restore_runner_metadata(&destination, &metadata, &backup, true);

        let merged = std::fs::read_to_string(metadata.join("context.md")).unwrap();
        assert!(merged.contains("identifier: \"WEB\""));
        assert!(merged.contains("identifier: \"API\""));
        assert!(!backup.exists());
    }

    #[test]
    fn runner_metadata_is_not_removed_when_user_files_exist() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".karma-team")).unwrap();
        std::fs::write(tmp.path().join(".karma-team/context.md"), "# context").unwrap();

        std::fs::write(tmp.path().join("notes.txt"), "user content").unwrap();
        assert!(!is_bootstrappable(tmp.path()).unwrap());
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(".karma-team/context.md")).unwrap(),
            "# context"
        );
    }

    #[test]
    fn unpublished_clone_cleanup_is_isolated_from_destination_files() {
        let tmp = tempdir().unwrap();
        let destination = tmp.path().join("target");
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("user.txt"), "keep").unwrap();
        let staging = clone_staging_path(&destination).unwrap();
        std::fs::create_dir_all(staging.join(".git/objects")).unwrap();
        std::fs::write(staging.join("partial.txt"), "from clone").unwrap();

        remove_entry(&staging).unwrap();

        assert_eq!(
            std::fs::read_to_string(destination.join("user.txt")).unwrap(),
            "keep"
        );
        assert!(!staging.exists());
    }

    #[tokio::test]
    async fn unuploaded_workpad_blocks_clone_and_is_never_cleaned_up() {
        let tmp = tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".karma-team")).unwrap();
        std::fs::write(tmp.path().join(".karma-team/context.md"), "# context").unwrap();
        let workpad = tmp.path().join(".karma-team-workpad.md");
        std::fs::write(&workpad, "not uploaded").unwrap();

        assert!(!is_bootstrappable(tmp.path()).unwrap());
        let resolution = resolve(tmp.path(), Some("https://example.com/repo.git"))
            .await
            .unwrap();
        assert!(matches!(resolution, Resolution::Directory(_)));
        assert_eq!(std::fs::read_to_string(&workpad).unwrap(), "not uploaded");
    }

    #[test]
    fn completed_clone_is_not_published_over_new_user_files() {
        let tmp = tempdir().unwrap();
        let destination = tmp.path().join("target");
        let staging = clone_staging_path(&destination).unwrap();
        std::fs::create_dir_all(staging.join(".git")).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::create_dir_all(destination.join(".karma-team")).unwrap();
        std::fs::write(destination.join(".karma-team/context.md"), "# context").unwrap();
        std::fs::write(destination.join("workpad.md"), "new local contents").unwrap();

        let result = publish_clone(&staging, &destination);

        assert!(matches!(result, Err(PublishCloneError::DestinationChanged)));
        assert_eq!(
            std::fs::read_to_string(destination.join("workpad.md")).unwrap(),
            "new local contents"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join(".karma-team/context.md")).unwrap(),
            "# context"
        );
        assert!(staging.join(".git").is_dir());
    }

    #[test]
    fn clone_error_sanitizer_removes_http_credentials_and_query() {
        let error = anyhow::anyhow!(
            "git clone failed: unable to access 'https://user:token@example.test/repo.git?access_token=secret': denied"
        );

        let sanitized = sanitize_clone_error(&error);

        assert!(sanitized.contains("https://example.test/<redacted>"));
        for secret in ["user", "token", "repo.git", "access_token", "secret"] {
            assert!(!sanitized.contains(secret), "leaked {secret}: {sanitized}");
        }
    }

    #[test]
    fn url_shape_never_exposes_credentials() {
        assert_eq!(url_shape("https://user:token@host/repo.git"), "https");
        assert_eq!(
            url_shape("deploy@git.example.com:team/repo.git"),
            "scp-like"
        );
        assert_eq!(url_shape("ext::sh -c evil"), "no-scheme");
        assert_eq!(url_shape("ftp://host/repo.git"), "other-scheme");
        assert!(!url_shape("https://user:token@host/repo.git").contains("token"));
    }

    #[test]
    fn accepts_scp_style_urls_with_non_git_users() {
        assert!(is_scp_clone_url("deploy@git.example.com:team/repo.git"));
        assert!(!is_scp_clone_url("--upload-pack=evil"));
        assert!(!is_scp_clone_url("/tmp/repo"));
    }
}
