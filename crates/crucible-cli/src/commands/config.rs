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
    Init {
        #[arg(long, help = "Include all configured agents in the review list")]
        full: bool,
        #[arg(
            long,
            help = "Write to ~/.config/crucible/config.toml instead of .crucible.toml"
        )]
        global: bool,
    },
    Validate,
}

pub fn run(args: ConfigArgs) -> Result<()> {
    match args.command {
        ConfigCommand::Init { full, global } => init(full, global),
        ConfigCommand::Validate => validate(),
    }
}

fn init(full: bool, global: bool) -> Result<()> {
    let path = if global {
        let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("HOME not set"))?;
        let dir = std::path::PathBuf::from(home).join(".config/crucible");
        fs::create_dir_all(&dir).context("create config dir")?;
        dir.join("config.toml")
    } else {
        std::env::current_dir()?.join(".crucible.toml")
    };
    if path.exists() {
        return Err(anyhow::anyhow!("{} already exists", path.display()));
    }
    let cfg = if full {
        CrucibleConfig::default_full()
    } else {
        CrucibleConfig::default()
    };
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
