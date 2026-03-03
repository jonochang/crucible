use anyhow::Result;
use clap::{Args, Subcommand};

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

pub fn run(_args: SessionArgs) -> Result<()> {
    println!("Session commands are not available in MVP");
    Ok(())
}
