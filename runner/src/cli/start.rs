//! `karma-team start` — start the installed service.
//!
//! Delegates to systemd (`systemctl --user start karma-team.service`) on Linux and
//! launchd (`launchctl kickstart -k gui/<uid>/so.karma-team.daemon`) on macOS. The
//! foreground daemon entry point moved to the hidden `karma-team __run`
//! subcommand (`cli/run.rs`), which is what the generated unit files exec.

use anyhow::Result;
use clap::Args as ClapArgs;

use crate::util::paths::Paths;

#[derive(Debug, ClapArgs)]
pub struct Args {}

pub async fn run(_args: Args, _paths: &Paths) -> Result<()> {
    validate_agent_config(_paths)?;
    crate::service::detect().start().await
}

/// Do not touch the service manager when an existing config selects a
/// retired agent. A missing config is still allowed here so the service can
/// retain its historical first-run error message.
fn validate_agent_config(paths: &Paths) -> Result<()> {
    if paths.config_path().exists() {
        crate::config::file::load_config(paths)?.validate_agents()?;
    }
    Ok(())
}
