use clap::Parser;
use karma_team::cli::{Cli, Command, connect, doctor, runner};
use karma_team::config::schema::AgentKind;
use karma_team::util::paths::Paths;

fn paths(root: &std::path::Path) -> Paths {
    Paths {
        config_dir: root.join("config"),
        data_dir: root.join("data"),
        runtime_dir: root.join("runtime"),
    }
}

fn disabled_agents() -> [AgentKind; 6] {
    [
        AgentKind::Codex,
        AgentKind::ClaudeCode,
        AgentKind::CursorAgent,
        AgentKind::OpenClaw,
        AgentKind::Grok,
        AgentKind::MuseCode,
    ]
}

fn assert_unsupported(error: &anyhow::Error, kind: AgentKind) {
    let message = format!("{error:#}");
    assert!(
        message.contains(&format!("unsupported agent '{}'", kind.wire_name())),
        "{message}"
    );
    assert!(
        message.contains("only the karma agent is supported"),
        "{message}"
    );
}

#[test]
fn runner_add_defaults_to_karma() {
    let cli = Cli::try_parse_from(["karma-team", "runner", "add", "--project", "TEST"]).unwrap();
    let Some(Command::Runner(runner::RunnerArgs {
        command: runner::RunnerCommand::Add(args),
    })) = cli.command
    else {
        panic!("runner add expected");
    };
    assert_eq!(args.agent, AgentKind::Karma);
}

#[test]
fn service_runtime_override_is_parsed_for_hidden_daemon_entrypoint() {
    let runtime = "/run/user/1000/karma-team";
    let cli = Cli::try_parse_from(["karma-team", "--runtime-dir", runtime, "__run", "--offline"])
        .unwrap();

    assert_eq!(
        cli.runtime_dir.as_deref(),
        Some(std::path::Path::new(runtime))
    );
    let Some(Command::Run(args)) = cli.command else {
        panic!("hidden daemon entrypoint expected");
    };
    assert!(args.offline);
}

#[tokio::test]
async fn runner_add_rejects_other_agents_before_authentication() {
    for kind in disabled_agents() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let error = runner::add(
            runner::AddArgs {
                url: Some("not-a-cloud-url".into()),
                name: None,
                project: "TEST".into(),
                workspace: None,
                pod: None,
                working_dir: None,
                agent: kind,
                model: None,
                reasoning_effort: None,
            },
            &paths,
        )
        .await
        .unwrap_err();
        assert_unsupported(&error, kind);
        assert!(!paths.config_path().exists());
        assert!(!paths.data_dir.exists());
    }
}

#[tokio::test]
async fn connect_rejects_other_agents_before_enrollment() {
    for kind in disabled_agents() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let error = connect::run(
            connect::Args {
                url: "not-a-cloud-url".into(),
                token: "unused".into(),
                host_label: None,
                working_dir: None,
                agent: Some(kind),
                model: None,
                reasoning_effort: None,
                skip_service: true,
                skip_linger: true,
            },
            &paths,
        )
        .await
        .unwrap_err();
        assert_unsupported(&error, kind);
        assert!(!paths.config_path().exists());
        assert!(!paths.data_dir.exists());
    }
}

#[tokio::test]
async fn doctor_rejects_other_agents_without_running_their_binary() {
    for kind in disabled_agents() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        let config = format!(
            "version = 2\n[daemon]\ncloud_url = 'not-a-cloud-url'\n[[runner]]\nname = 'test'\nrunner_id = '{}'\nproject_slug = 'TEST'\n[runner.workspace]\nworking_dir = '/unused'\n[runner.agent]\nkind = '{}'\n",
            uuid::Uuid::new_v4(),
            serde_json::to_value(kind).unwrap().as_str().unwrap(),
        );
        std::fs::write(paths.config_path(), &config).unwrap();
        let error = doctor::execute(&paths, None).await.unwrap_err();
        assert_unsupported(&error, kind);
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            config
        );
    }
}

#[test]
fn managed_codex_provisioning_is_rejected_before_loading_config() {
    for verb in ["enroll", "rebind"] {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_karma-team"));
        command
            .env("KARMA_TEAM_CONFIG_DIR", &paths.config_dir)
            .env("KARMA_TEAM_DATA_DIR", &paths.data_dir)
            .args(["__managed", verb]);
        if verb == "enroll" {
            command.args([
                "--workspace",
                "test",
                "--project",
                "TEST",
                "--working-dir",
                "/unused",
            ]);
        }
        let output = command
            .args([
                "--engine",
                "/unused/engine",
                "--codex-home",
                "/unused/home",
                "--path-prepend",
                "/unused/bin",
                "--model-token-file",
                "/unused/token",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("unsupported agent 'codex'"), "{stderr}");
        assert!(
            stderr.contains("only the karma agent is supported"),
            "{stderr}"
        );
        assert!(!paths.config_path().exists());
        assert!(!paths.data_dir.exists());
    }
}
