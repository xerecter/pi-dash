//! `karma-team restart` — restart the installed service and verify daemon health.

use anyhow::Result;
use clap::Args as ClapArgs;

use crate::util::paths::Paths;

#[derive(Debug, ClapArgs)]
pub struct Args {}

pub async fn run(_args: Args, paths: &Paths) -> Result<()> {
    validate_agent_config(paths)?;
    println!("restarting daemon...");
    let outcome =
        crate::service::reload::restart_and_verify_with_progress(paths, |msg| eprintln!("{msg}"))
            .await;
    if outcome.ok {
        println!("daemon restarted ({}).", outcome.summary);
        return Ok(());
    }
    anyhow::bail!(
        "daemon restart did not complete cleanly: {}\n{}",
        outcome.summary,
        outcome.detail.unwrap_or_default()
    )
}

/// Validate before `restart_and_verify_with_progress` can rewrite a unit or
/// start the service. Missing config remains the service helper's concern.
fn validate_agent_config(paths: &Paths) -> Result<()> {
    if paths.config_path().exists() {
        crate::config::file::load_config(paths)?.validate_agents()?;
    }
    Ok(())
}
