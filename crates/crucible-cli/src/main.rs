use anyhow::Result;
use clap::{Parser, Subcommand};
use std::process::ExitCode;

mod commands;
mod log_helpers;
mod tui;

#[derive(Parser)]
#[command(name = "crucible", version, about = "Multi-agent consensus CLI", after_long_help = concat!(
    "EXAMPLES:\n",
    "  Code review on current branch vs main (default: deep review):\n",
    "      crucible review --branch\n\n",
    "  Fast review with fewer agents:\n",
    "      crucible review --branch --short\n\n",
    "  Design review with architecture doc:\n",
    "      crucible consensus run --pack design-review --prompt \"Evaluate this architecture\" --attach ARCHITECTURE.md\n\n",
    "  Requirements review with inline text:\n",
    "      crucible consensus run --pack requirements-review --prompt \"Review these requirements\" --attach-text \"Users can upload files. Admins can delete any file.\"\n\n",
    "  Test plan review:\n",
    "      crucible consensus run --pack test-plan-review --prompt \"Review for coverage gaps\" --attach test-plan.md\n\n",
    "  List available task packs:\n",
    "      crucible consensus packs\n\n",
    "  Full list: crucible <subcommand> --help\n",
))]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Consensus(commands::consensus::ConsensusArgs),
    Review(commands::review::ReviewArgs),
    PromptEval(commands::prompt_eval::PromptEvalArgs),
    Hook(commands::hook::HookArgs),
    Config(commands::config::ConfigArgs),
    Doctor(commands::doctor::DoctorArgs),
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
        Command::Consensus(args) => commands::consensus::run(args).await,
        Command::Review(args) => commands::review::run(args).await,
        Command::PromptEval(args) => commands::prompt_eval::run(args).await,
        Command::Hook(args) => commands::hook::run(args),
        Command::Config(args) => commands::config::run(args),
        Command::Doctor(args) => commands::doctor::run(args),
        Command::Session(args) => commands::session::run(args),
        Command::Version => {
            println!("crucible {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
