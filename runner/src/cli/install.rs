//! `karma-team install` — write or refresh the OS service unit.
//!
//! Machine enrollment now lives in `karma-team auth login`; project runners
//! are reconciled automatically by the daemon. `karma-team runner add` remains
//! an optional manual path. `install` writes the systemd / launchd unit so the
//! daemon survives reboots. Safe to run repeatedly (e.g. after an upgrade).

use anyhow::Result;
use clap::Args as ClapArgs;

use crate::util::paths::Paths;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Skip the `sudo loginctl enable-linger` step (Linux only). Without
    /// linger the daemon only starts at login, not at boot. Set this in
    /// CI / unattended installs where a sudo password prompt would hang.
    #[arg(long)]
    pub skip_linger: bool,
}

pub async fn run(args: Args, paths: &Paths) -> Result<()> {
    validate_agent_config(paths)?;
    let svc = crate::service::detect();
    let enrolled = machine_is_configured(paths);

    svc.write_unit(paths).await?;
    if !enrolled {
        println!("Service unit written.");
        println!();
        println!("This machine is not configured yet. Next:");
        println!("  karma-team auth login --url <URL>");
        println!();
        return Ok(());
    }

    svc.enable_and_start().await?;
    if !args.skip_linger {
        let _ = svc.ensure_boot_start().await;
    }
    println!();
    println!("Service unit refreshed and daemon restarted.");
    println!();
    Ok(())
}

/// Validate before writing or activating the service unit. Existing configs
/// selecting a retired agent must not leave a stopped/replaced service behind.
fn validate_agent_config(paths: &Paths) -> Result<()> {
    if paths.config_path().exists() {
        crate::config::file::load_config(paths)?.validate_agents()?;
    }
    Ok(())
}

fn machine_is_configured(paths: &Paths) -> bool {
    let Ok(cfg) = crate::config::file::load_config(paths) else {
        return false;
    };
    let has_shared_machine_token = cfg
        .cli
        .as_ref()
        .and_then(|cli| cli.token.as_deref())
        .map(|token| token.starts_with("mt_"))
        .unwrap_or(false);
    // A machine-token binding is a complete daemon enrollment even before
    // the first project runner exists. ProjectSync will populate runners
    // after the service starts. Keep the dev-machine identity requirement so
    // a partially written/legacy config cannot launch an idle service.
    if has_shared_machine_token && cfg.daemon.dev_machine_id.is_some() {
        return true;
    }
    if cfg.runners.is_empty() {
        return false;
    }
    has_shared_machine_token || paths.credentials_path().exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{CliSection, Config, DaemonConfig};

    #[test]
    fn machine_token_binding_is_configured_without_any_runner() {
        let temp = tempfile::tempdir().expect("temp dir");
        let paths = Paths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            runtime_dir: temp.path().join("runtime"),
        };
        let config = Config {
            version: 2,
            daemon: DaemonConfig {
                cloud_url: "https://example.test".into(),
                dev_machine_id: Some(uuid::Uuid::new_v4()),
                ..DaemonConfig::default()
            },
            runners: Vec::new(),
            cli: Some(CliSection {
                token: Some("mt_bound_machine".into()),
                workspace_slug: Some("acme".into()),
                default_project: None,
                ..CliSection::default()
            }),
        };
        crate::config::file::write_config(&paths, &config).expect("write config");

        assert!(machine_is_configured(&paths));
    }
}
