use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use std::fs;

#[derive(Args)]
pub struct SessionArgs {
    #[command(subcommand)]
    pub command: SessionCommand,
}

#[derive(Subcommand)]
pub enum SessionCommand {
    List,
    Resume { id: String },
    Delete { id: String },
}

pub fn run(args: SessionArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = cwd.join(".crucible/sessions");
    match args.command {
        SessionCommand::List => {
            if !root.exists() {
                return Ok(());
            }
            for entry in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
                let entry = entry?;
                if entry.path().is_dir() {
                    println!("{}", entry.file_name().to_string_lossy());
                }
            }
            Ok(())
        }
        SessionCommand::Resume { id } => {
            let path = root.join(&id).join("report.json");
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("read session report {}", path.display()))?;
            println!("{raw}");
            Ok(())
        }
        SessionCommand::Delete { id } => {
            let dir = root.join(&id);
            if !dir.exists() {
                return Err(anyhow!("session '{}' not found", id));
            }
            fs::remove_dir_all(&dir)
                .with_context(|| format!("remove session {}", dir.display()))?;
            Ok(())
        }
    }
}
