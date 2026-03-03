use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use libcrucible::config::CrucibleConfig;
use std::fs;

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    Init,
    Validate,
}

pub fn run(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommand::Init => init(),
        ConfigCommand::Validate => validate(),
    }
}

fn init() -> Result<()> {
    let path = std::env::current_dir()?.join(".crucible.toml");
    if path.exists() {
        return Err(anyhow::anyhow!(".crucible.toml already exists"));
    }
    let cfg = CrucibleConfig::default();
    let content = toml::to_string_pretty(&cfg).context("serialize config")?;
    fs::write(&path, content).context("write config")?;
    println!("Wrote {}", path.display());
    Ok(())
}

fn validate() -> Result<()> {
    let _cfg = CrucibleConfig::load()?;
    println!("Config OK");
    Ok(())
}
