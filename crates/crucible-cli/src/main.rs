use anyhow::Result;
use clap::{Parser, Subcommand};
use std::process::ExitCode;

mod commands;
mod log_helpers;
mod tui;

#[derive(Parser)]
#[command(name = "crucible", version, about = "Multi-agent code review CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Review(commands::review::ReviewArgs),
    PromptEval(commands::prompt_eval::PromptEvalArgs),
    Hook(commands::hook::HookArgs),
    Config(commands::config::ConfigArgs),
    Session(commands::session::SessionArgs),
    Version,
}

#[tokio::main]
async fn main() -> ExitCode {
    if let Err(err) = run().await {
        eprintln!("crucible error: {err}");
        return ExitCode::from(1);
    }
    ExitCode::from(0)
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Review(args) => commands::review::run(args).await,
        Command::PromptEval(args) => commands::prompt_eval::run(args).await,
        Command::Hook(args) => commands::hook::run(args),
        Command::Config(args) => commands::config::run(args),
        Command::Session(args) => commands::session::run(args),
        Command::Version => {
            println!("crucible {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
