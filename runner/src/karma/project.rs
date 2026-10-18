//! Karma project discovery and per-run workspace layout.
//!
//! The Karma CLI owns the project directory (and its metadata).  Runner
//! workspaces must therefore be children of the path returned by
//! `karma project info`, never paths derived directly from cloud-controlled
//! names.

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;
use uuid::Uuid;

use crate::util::shell::{is_benign_login_shell_warning, login_shell_command};

/// Convert an externally supplied name into one safe path component.
pub fn slugify_component(value: &str, fallback: &str) -> String {
    let mut slug = String::with_capacity(value.len());
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() || slug == "." || slug == ".." {
        let fallback = fallback.trim();
        if fallback.is_empty() || fallback == "." || fallback == ".." {
            return "item".to_string();
        }
        return slugify_component(fallback, "item");
    }
    slug
}

/// Return `<project-root>/<runner>/<work-item>` using only safe components.
pub fn operation_dir(project_root: &Path, runner_name: &str, work_item: &str) -> PathBuf {
    project_root
        .join(slugify_component(runner_name, "runner"))
        .join(slugify_component(work_item, "run"))
}

#[derive(Debug, Clone)]
struct ProjectRecord {
    name: String,
    id: String,
}

/// Ensure that the Karma project for a runner exists and return the isolated
/// directory for this work item. Karma owns project metadata and chooses the
/// project root, so this function always confirms the root through `project
/// info` before creating runner-owned children below it. Projects and operation
/// directories are durable user-visible workspaces; this module never removes
/// them when a work item completes.
pub async fn prepare_operation_dir(
    binary: &str,
    project_name: &str,
    runner_name: &str,
    work_item_identifier: Option<&str>,
    run_id: Uuid,
) -> Result<PathBuf> {
    if project_name.chars().any(char::is_control) {
        bail!("Karma project name must not contain control characters");
    }
    let project_name = project_name.trim();
    if project_name.is_empty() {
        bail!("Karma project name is empty; configure the runner's project_slug");
    }

    // Karma allows duplicate display names. Serialize discovery and creation
    // across runner tasks and daemon processes sharing this user's projects.
    let _project_lock = acquire_project_lock(project_name).await?;
    let records = list_projects(binary).await?;
    let project_id = if let Some(existing) = records
        .iter()
        .find(|project| project.name.trim() == project_name)
    {
        existing.id.clone()
    } else {
        create_project(binary, project_name, runner_name, run_id).await?
    };

    let info = run_cli(binary, &["project", "info", &project_id]).await?;
    if !info.status.success() {
        bail!("karma project info failed: {}", output_error(&info));
    }
    if parse_project_name(&info.stdout).as_deref() != Some(project_name) {
        bail!("karma project info did not report the requested project name unambiguously");
    }
    let project_root = parse_project_path(&info.stdout)
        .context("karma project info did not include an absolute Path")?;
    let root = tokio::fs::canonicalize(&project_root)
        .await
        .with_context(|| format!("resolving Karma project directory {project_root:?}"))?;
    let work_item = work_item_identifier
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| run_id.to_string());
    let operation = operation_dir(&root, runner_name, &work_item);
    let runner_dir = operation
        .parent()
        .context("Karma operation directory has no runner parent")?;
    for path in [runner_dir, operation.as_path()] {
        if let Ok(metadata) = tokio::fs::symlink_metadata(path).await
            && metadata.file_type().is_symlink()
        {
            bail!("refusing symlinked Karma operation path {}", path.display());
        }
    }
    tokio::fs::create_dir_all(&operation)
        .await
        .with_context(|| format!("creating Karma operation directory {operation:?}"))?;

    // `operation_dir` is lexical and safe for normal directories. Refuse a
    // pre-existing symlinked child so cloud-controlled names cannot redirect
    // an agent outside the Karma project root.
    let operation_real = tokio::fs::canonicalize(&operation)
        .await
        .with_context(|| format!("resolving Karma operation directory {operation:?}"))?;
    if !operation_real.starts_with(&root) || operation_real == root {
        bail!(
            "Karma operation directory escaped project root: {}",
            operation_real.display()
        );
    }
    Ok(operation_real)
}

async fn acquire_project_lock(project_name: &str) -> Result<File> {
    let karma_dir = std::env::var_os("KARMA_DATA_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".karma")))
        .context("resolving Karma data directory for project lock")?;
    let lock_dir = karma_dir.join("runner-project-locks");
    tokio::fs::create_dir_all(&lock_dir)
        .await
        .context("creating Karma project lock directory")?;
    let key = Uuid::new_v5(&Uuid::NAMESPACE_URL, project_name.as_bytes());
    let lock_path = lock_dir.join(format!("{key}.lock"));
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&lock_path)
        .with_context(|| format!("opening Karma project lock {lock_path:?}"))?;
    // A nonblocking flock keeps the async runtime responsive and releases
    // automatically when the file is dropped, including on cancellation.
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(file),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(error).context("locking Karma project"),
            }
        }
    })
    .await
    .context("timed out waiting for Karma project lock")?
}

async fn list_projects(binary: &str) -> Result<Vec<ProjectRecord>> {
    let out = run_cli(binary, &["project", "list"]).await?;
    if !out.status.success() {
        bail!("karma project list failed: {}", output_error(&out));
    }
    Ok(parse_project_list(&out.stdout))
}

async fn create_project(
    binary: &str,
    project_name: &str,
    runner_name: &str,
    run_id: Uuid,
) -> Result<String> {
    let create = run_cli(
        binary,
        &[
            "project",
            "create",
            project_name,
            "--no-session",
            "--no-git",
        ],
    )
    .await?;
    if create.status.success() {
        return project_id_from_create_or_list(binary, project_name, &create).await;
    }

    // Another runner may have created the same project between list and
    // create. Re-list before attempting recovery so concurrent assignments
    // converge on one project record.
    if let Some(existing) = find_project(binary, project_name).await? {
        return Ok(existing.id);
    }

    // Older Karma installations reserve the default project directory even
    // when the project metadata was removed. Retry in a runner/run-specific
    // directory instead of failing every assignment on that stale checkout.
    if is_directory_exists_error(&create) {
        let path = fallback_project_path(project_name, runner_name, run_id);
        let retry = run_cli(
            binary,
            &[
                "project",
                "create",
                project_name,
                "--path",
                &path,
                "--no-session",
                "--no-git",
            ],
        )
        .await?;
        if retry.status.success() {
            return project_id_from_create_or_list(binary, project_name, &retry).await;
        }
        if let Some(existing) = find_project(binary, project_name).await? {
            return Ok(existing.id);
        }
        bail!(
            "karma project create failed in the fallback directory {path:?}: {}",
            output_error(&retry)
        );
    }

    bail!("karma project create failed: {}", output_error(&create));
}

async fn project_id_from_create_or_list(
    binary: &str,
    project_name: &str,
    create: &Output,
) -> Result<String> {
    if let Some(id) = parse_project_id(&create.stdout) {
        return Ok(id);
    }
    if let Some(existing) = find_project(binary, project_name).await? {
        return Ok(existing.id);
    }
    bail!(
        "karma project create succeeded but did not report a project id: {}",
        output_text(&create.stdout)
    );
}

async fn find_project(binary: &str, project_name: &str) -> Result<Option<ProjectRecord>> {
    Ok(list_projects(binary)
        .await?
        .into_iter()
        .find(|project| project.name.trim() == project_name))
}

fn fallback_project_path(project_name: &str, runner_name: &str, run_id: Uuid) -> String {
    let mut project = slugify_component(project_name, "project");
    let mut runner = slugify_component(runner_name, "runner");
    // Slugs contain only ASCII. Leave room for both separators and the UUID
    // under the usual 255-byte filesystem component limit.
    project.truncate(96);
    runner.truncate(96);
    format!("{}-{}-{}", project, runner, run_id.simple())
}

fn is_directory_exists_error(output: &Output) -> bool {
    output
        .stderr
        .windows("Directory already exists:".len())
        .any(|window| window == b"Directory already exists:")
}

async fn run_cli(binary: &str, args: &[&str]) -> Result<Output> {
    let mut command = login_shell_command(binary, args, None);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .with_context(|| format!("timed out running {binary} {}", args.join(" ")))?
        .with_context(|| format!("running {binary} {}", args.join(" ")))
}

fn parse_project_list(stdout: &[u8]) -> Vec<ProjectRecord> {
    let mut projects = Vec::new();
    let mut current: Option<(ProjectRecord, usize)> = None;
    for line in String::from_utf8_lossy(stdout).lines() {
        let line = line.trim();
        if let Some(name) = project_header_name(line) {
            if let Some((record, 1)) = current.take()
                && !record.id.is_empty()
            {
                projects.push(record);
            }
            current = Some((
                ProjectRecord {
                    name: name.to_string(),
                    id: String::new(),
                },
                0,
            ));
        } else if let Some(id) = line.strip_prefix("ID:")
            && let Some((record, count)) = current.as_mut()
        {
            record.id = id.trim().to_string();
            *count += 1;
        }
    }
    if let Some((record, 1)) = current
        && !record.id.is_empty()
    {
        projects.push(record);
    }
    projects
}

fn project_header_name(line: &str) -> Option<&str> {
    ["📁", "💻", "📚", "⭐", "❤️", "🚀", "⚡", "🔥"]
        .iter()
        .find_map(|icon| line.trim().strip_prefix(icon))
        .map(str::trim)
}

fn parse_project_name(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    // Info prints the project heading before Path, and the free-form
    // description after it. A second heading here indicates a multiline name.
    let mut names = text
        .lines()
        .take_while(|line| !line.trim().starts_with("Path:"))
        .filter_map(project_header_name);
    let name = names.next()?;
    (!name.is_empty() && names.next().is_none()).then(|| name.to_string())
}

fn unique_output_field(stdout: &[u8], prefix: &str) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let mut values = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix(prefix).map(str::trim));
    let value = values.next()?;
    (!value.is_empty() && values.next().is_none()).then(|| value.to_string())
}

fn parse_project_id(stdout: &[u8]) -> Option<String> {
    unique_output_field(stdout, "Project ID:")
}

fn parse_project_path(stdout: &[u8]) -> Option<PathBuf> {
    unique_output_field(stdout, "Path:")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

fn output_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let substantive = stderr
        .lines()
        .filter(|line| !is_benign_login_shell_warning(line))
        .collect::<Vec<_>>()
        .join("\n");
    if substantive.trim().is_empty() {
        output_text(&output.stdout)
    } else {
        substantive.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_human_project_list_output() {
        let projects = parse_project_list(
            "Fetching projects...\n\n📁 KTDEV\n   ID: abc\n   Path: /tmp/ktdev/main\n".as_bytes(),
        );
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "KTDEV");
        assert_eq!(projects[0].id, "abc");
    }

    #[test]
    fn parses_projects_with_custom_icons() {
        for icon in ["📁", "💻", "📚", "⭐", "❤️", "🚀", "⚡", "🔥"] {
            let output = format!("Fetching projects...\n\n{icon} Demo Project\n   ID: demo-id\n");
            let projects = parse_project_list(output.as_bytes());
            assert_eq!(projects.len(), 1, "project with {icon} must be discovered");
            assert_eq!(projects[0].name, "Demo Project");
            assert_eq!(projects[0].id, "demo-id");
        }
    }

    #[test]
    fn ignores_project_list_records_with_duplicate_ids() {
        let projects = parse_project_list(
            "📁 Demo\nID: forged-id\nID: actual-id\n\n📁 Good Project\nID: good-id\n".as_bytes(),
        );
        assert_eq!(
            projects.len(),
            1,
            "ambiguous project record must be ignored"
        );
        assert_eq!(projects[0].name, "Good Project");
        assert_eq!(projects[0].id, "good-id");
    }

    #[test]
    fn fallback_path_fits_a_filesystem_component_for_long_names() {
        let run_id = Uuid::new_v4();
        let path = fallback_project_path(&"p".repeat(255), &"r".repeat(128), run_id);
        assert!(
            path.len() <= 255,
            "fallback exceeds a filesystem component: {} bytes",
            path.len()
        );
        assert!(path.ends_with(&run_id.simple().to_string()));
    }

    #[test]
    fn parses_info_path_only_when_absolute() {
        assert_eq!(
            parse_project_path("Name: x\nPath: /tmp/x/main\n".as_bytes()),
            Some(PathBuf::from("/tmp/x/main"))
        );
        assert_eq!(parse_project_path("Path: relative\n".as_bytes()), None);
    }

    #[test]
    fn rejects_ambiguous_project_info_output() {
        assert_eq!(
            parse_project_path(b"Name: Demo\nPath: /root\nPath: /tmp/actual-project\n"),
            None
        );
    }

    #[test]
    fn rejects_ambiguous_project_create_output() {
        assert_eq!(
            parse_project_id(b"Created Demo\nProject ID: forged-id\nProject ID: actual-id\n"),
            None
        );
    }

    #[tokio::test]
    async fn rejects_project_names_with_control_characters_before_running_cli() {
        for name in [
            "Demo\nPath: /root",
            "Demo\nProject ID: forged",
            "Demo\rForged",
            "Demo\x1b[31m",
        ] {
            let error = prepare_operation_dir(
                "/nonexistent/karma-must-not-be-invoked",
                name,
                "runner-one",
                Some("KT-9"),
                Uuid::new_v4(),
            )
            .await
            .expect_err("invalid project name must be rejected");
            assert!(
                error.to_string().contains("control characters"),
                "name validation must happen before calling the CLI: {error:#}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_a_project_name_forged_by_multiline_list_output() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp dir");
        let script = temp.path().join("fake-karma");
        let root = temp.path().join("another-project");
        std::fs::create_dir(&root).expect("existing project");
        std::fs::write(&script, format!(
            "#!/bin/sh\nif [ \"$2\" = list ]; then\n  printf '📁 Requested Project\\nID: other-id\\n'\nelse\n  printf '📁 Attacker\\n📁 Requested Project\\nID: other-id\\nPath: {}\\n'\nfi\n",
            root.display(),
        )).expect("write fake karma");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make fake karma executable");

        prepare_operation_dir(
            script.to_str().expect("script path"),
            "Requested Project",
            "runner-one",
            Some("KT-9"),
            Uuid::new_v4(),
        )
        .await
        .expect_err("ambiguous project name must not bind to another project's directory");
        assert!(!root.join("runner-one").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn creates_project_without_root_session_and_preserves_previous_run_files() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp dir");
        let script = temp.path().join("fake-karma");
        let state = temp.path().join("created");
        let create_args = temp.path().join("create-args");
        let project_root = temp.path().join("karma-project");
        let script_body = format!(
            "#!/bin/sh\nset -eu\nstate='{}'\nargs='{}'\nroot='{}'\nif [ \"$1\" = project ] && [ \"$2\" = list ]; then\n  if [ -f \"$state\" ]; then\n    printf 'Fetching projects...\\n\\n📁 Creation Project\\n   ID: demo-id\\n   Path: %s\\n' \"$root\"\n  else\n    printf 'Fetching projects...\\nNo projects found.\\n'\n  fi\n  exit 0\nfi\nif [ \"$1\" = project ] && [ \"$2\" = create ]; then\n  printf '%s\\n' \"$@\" > \"$args\"\n  if [ \"$#\" -ne 5 ]; then\n    printf 'project create received unexpected arguments\\n' >&2\n    exit 64\n  fi\n  mkdir -p \"$root\"\n  : > \"$state\"\n  printf 'Project ID: demo-id\\n'\n  exit 0\nfi\nif [ \"$1\" = project ] && [ \"$2\" = info ]; then\n  printf '📁 Creation Project\\nPath: %s\\n' \"$root\"\n  exit 0\nfi\nexit 2\n",
            state.display(),
            create_args.display(),
            project_root.display()
        );
        std::fs::write(&script, script_body).expect("write fake karma");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make fake karma executable");

        let first = prepare_operation_dir(
            script.to_str().expect("script path"),
            "Creation Project",
            "runner/one",
            Some("KT-10"),
            Uuid::new_v4(),
        )
        .await
        .expect("first project preparation");
        assert_eq!(first, project_root.join("runner-one").join("KT-10"));
        assert_eq!(
            std::fs::read_to_string(&create_args).expect("recorded project create arguments"),
            "project\ncreate\nCreation Project\n--no-session\n--no-git\n"
        );

        let result_file = first.join("result.txt");
        std::fs::write(&result_file, "completed work").expect("write first run result");

        let second = prepare_operation_dir(
            script.to_str().expect("script path"),
            "Creation Project",
            "runner/one",
            Some("KT-11"),
            Uuid::new_v4(),
        )
        .await
        .expect("project reuse");
        assert_eq!(second, project_root.join("runner-one").join("KT-11"));
        assert!(state.is_file(), "fake project must have been created");
        assert_eq!(
            std::fs::read_to_string(result_file).expect("previous work item result is preserved"),
            "completed work"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recovers_when_default_project_directory_already_exists() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp dir");
        let script = temp.path().join("fake-karma");
        let project_root = temp.path().join("existing-project");
        let retry_args = temp.path().join("retry-args");
        std::fs::create_dir_all(project_root.join(".git")).expect("create existing project root");
        let script_body = format!(
            "#!/bin/sh\nset -eu\nroot='{}'\nretry_args='{}'\nif [ \"$1\" = project ] && [ \"$2\" = list ]; then\n  printf 'Fetching projects...\\nNo projects found.\\n'\n  exit 0\nfi\nif [ \"$1\" = project ] && [ \"$2\" = create ]; then\n  if [ \"$#\" -eq 5 ]; then\n    printf 'bash: /root/.local/share/karma-cli/env.sh: No such file or directory\\n' >&2\n    printf 'Error: Directory already exists: %s\\n' \"$root\" >&2\n    exit 1\n  fi\n  printf '%s\\n' \"$@\" > \"$retry_args\"\n  printf 'Project ID: recovered-id\\n'\n  exit 0\nfi\nif [ \"$1\" = project ] && [ \"$2\" = info ]; then\n  printf '📁 Recovery Project\\nPath: %s\\n' \"$root\"\n  exit 0\nfi\nprintf 'unexpected command: %s %s\\n' \"$1\" \"$2\" >&2\nexit 2\n",
            project_root.display(),
            retry_args.display()
        );
        std::fs::write(&script, script_body).expect("write fake karma");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make fake karma executable");

        let operation = prepare_operation_dir(
            script.to_str().expect("script path"),
            "Recovery Project",
            "runner-01",
            Some("KTDEV4-9"),
            Uuid::new_v4(),
        )
        .await
        .expect("existing default directory should trigger fallback recovery");

        assert_eq!(operation, project_root.join("runner-01").join("KTDEV4-9"));
        assert!(operation.is_dir());
        let retry = std::fs::read_to_string(retry_args).expect("fallback create arguments");
        assert!(retry.starts_with("project\ncreate\nRecovery Project\n--path\n"));
        assert!(retry.ends_with("\n--no-session\n--no-git\n"));
        assert_eq!(
            retry
                .lines()
                .filter(|argument| *argument == "--no-git")
                .count(),
            1,
            "fallback project creation must pass --no-git exactly once"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_recovery_creates_one_project_and_reuses_its_root() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temp dir");
        let script = temp.path().join("fake-karma");
        let project_name = format!("Concurrent-{}", Uuid::new_v4());
        let script_body = format!(
            r#"#!/bin/sh
set -eu
root='{}'
state="$root/project-id"
if [ "$1" = project ] && [ "$2" = list ]; then
  if [ -f "$state" ]; then
    printf '📁 {}\n   ID: %s\n' "$(cat "$state")"
  fi
  exit 0
fi
if [ "$1" = project ] && [ "$2" = create ]; then
  if [ "$#" -eq 5 ]; then
    printf 'Error: Directory already exists: %s/default/main\n' "$root" >&2
    exit 1
  fi
  test "$4" = --path
  id="$5"
  printf '%s\n' "$id" >> "$root/creations"
  mkdir -p "$root/$id"
  sleep 1
  printf '%s\n' "$id" > "$state"
  printf 'Project ID: %s\n' "$id"
  exit 0
fi
if [ "$1" = project ] && [ "$2" = info ]; then
  printf '📁 {}\nPath: %s/%s\n' "$root" "$3"
  exit 0
fi
exit 2
"#,
            temp.path().display(),
            project_name,
            project_name,
        );
        std::fs::write(&script, script_body).expect("write fake karma");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make fake karma executable");

        let binary = script.to_str().expect("script path");
        let (first, second) = tokio::join!(
            prepare_operation_dir(
                binary,
                &project_name,
                "runner-one",
                Some("KT-9"),
                Uuid::new_v4()
            ),
            prepare_operation_dir(
                binary,
                &project_name,
                "runner-two",
                Some("KT-10"),
                Uuid::new_v4()
            ),
        );
        let first = first.expect("first runner prepares its workspace");
        let second = second.expect("second runner prepares its workspace");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("creations"))
                .expect("project creations")
                .lines()
                .count(),
            1,
            "concurrent recovery must not create duplicate project records"
        );
        assert_eq!(
            first.parent().unwrap().parent(),
            second.parent().unwrap().parent()
        );
        assert_ne!(
            first, second,
            "runners retain isolated operation directories"
        );
    }
}
