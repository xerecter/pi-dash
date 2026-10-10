use karma_team::agent::AgentBridge;
use karma_team::config::{
    file,
    schema::{AgentKind, Config, RunnerConfig},
};
use karma_team::util::paths::Paths;
use serde_json::json;

const DISABLED: [AgentKind; 6] = [
    AgentKind::Codex,
    AgentKind::ClaudeCode,
    AgentKind::CursorAgent,
    AgentKind::OpenClaw,
    AgentKind::Grok,
    AgentKind::MuseCode,
];

fn config() -> Config {
    serde_json::from_value(json!({
        "version": 2,
        "daemon": { "cloud_url": "http://127.0.0.1:1" },
        "runner": [{
            "name": "test",
            "runner_id": "00000000-0000-4000-8000-000000000001",
            "project_slug": "TEST",
            "workspace": { "working_dir": "/unused/karma-only-test" }
        }]
    }))
    .unwrap()
}

fn assert_unsupported(error: impl std::fmt::Display, kind: AgentKind) {
    let message = error.to_string();
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
fn omitted_agent_defaults_to_karma() {
    assert_eq!(AgentKind::default(), AgentKind::Karma);
    assert_eq!(config().runners[0].agent.kind, AgentKind::Karma);
    let mut cfg = config();
    cfg.runners[0].agent.kind = AgentKind::Karma;
    cfg.validate().unwrap();
}

#[test]
fn config_validation_rejects_each_disabled_agent() {
    for kind in DISABLED {
        let mut cfg = config();
        cfg.runners[0].agent.kind = kind;
        assert_unsupported(
            cfg.validate()
                .expect_err("disabled agent must fail validation"),
            kind,
        );
    }
}

#[test]
fn config_write_rejects_disabled_agents_without_overwriting_existing_config() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::resolve(
        Some(root.path().join("config")),
        Some(root.path().join("data")),
    )
    .unwrap();
    let mut cfg = config();
    cfg.runners[0].agent.kind = AgentKind::Karma;
    file::write_config(&paths, &cfg).unwrap();
    let original = std::fs::read(paths.config_path()).unwrap();
    for kind in DISABLED {
        cfg.runners[0].agent.kind = kind;
        assert_unsupported(
            file::write_config(&paths, &cfg).expect_err("disabled agent must not be saved"),
            kind,
        );
        assert_eq!(std::fs::read(paths.config_path()).unwrap(), original);
    }
}

#[test]
fn legacy_agent_configs_remain_readable_and_are_not_silently_migrated() {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::resolve(
        Some(root.path().join("config")),
        Some(root.path().join("data")),
    )
    .unwrap();
    paths.ensure().unwrap();
    for kind in DISABLED {
        let mut cfg = config();
        cfg.runners[0].agent.kind = kind;
        let original = toml::to_string_pretty(&cfg).unwrap();
        std::fs::write(paths.config_path(), &original).unwrap();
        let loaded = file::load_config(&paths).unwrap();
        assert_eq!(loaded.runners[0].agent.kind, kind);
        assert_unsupported(
            loaded.validate().expect_err("legacy agent must not run"),
            kind,
        );
        assert_eq!(
            std::fs::read_to_string(paths.config_path()).unwrap(),
            original
        );
    }
}

#[tokio::test]
async fn runtime_entry_rejects_disabled_agents_before_launching_a_binary() {
    let root = tempfile::tempdir().unwrap();
    for kind in DISABLED {
        let mut runner: RunnerConfig = config().runners.remove(0);
        runner.agent.kind = kind;
        let missing = root.path().join("must-not-launch").display().to_string();
        runner.codex.binary = missing.clone();
        runner.claude_code.binary = missing.clone();
        runner.cursor_agent.binary = missing.clone();
        runner.openclaw.binary = missing.clone();
        runner.grok.binary = missing.clone();
        runner.muse_code.binary = missing;
        let result = AgentBridge::spawn_from_config(&runner, root.path(), None).await;
        let error = match result {
            Err(error) => error,
            Ok(bridge) => {
                let _ = bridge.shutdown(std::time::Duration::from_secs(1)).await;
                panic!("disabled agent must be rejected before launch: {kind:?}");
            }
        };
        assert_unsupported(error, kind);
    }
}
