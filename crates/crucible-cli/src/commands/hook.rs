use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use git2::Repository;
use std::fs;
use std::path::PathBuf;

const HEADER: &str = "# Managed by Crucible — do not edit manually (crucible hook uninstall to remove)";

#[derive(Args)]
pub struct HookArgs {
    #[command(subcommand)]
    pub command: HookCommand,
}

#[derive(Subcommand)]
pub enum HookCommand {
    Install { #[arg(long)] force: bool },
    Uninstall,
    Status,
}

pub fn run(args: HookArgs) -> Result<()> {
    match args.command {
        HookCommand::Install { force } => install(force),
        HookCommand::Uninstall => uninstall(),
        HookCommand::Status => status(),
    }
}

fn install(force: bool) -> Result<()> {
    let hook_path = hook_path()?;
    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path).unwrap_or_default();
        if !existing.contains(HEADER) && !force {
            return Err(anyhow!("pre-push hook exists; use --force to overwrite"));
        }
    }

    let contents = format!("#!/usr/bin/env bash\n{}\nset -euo pipefail\nexec crucible review --hook\n", HEADER);
    fs::write(&hook_path, contents).context("write pre-push hook")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms)?;
    }
    println!("Installed pre-push hook at {}", hook_path.display());
    Ok(())
}

fn uninstall() -> Result<()> {
    let hook_path = hook_path()?;
    if !hook_path.exists() {
        println!("No pre-push hook installed");
        return Ok(());
    }

    let existing = fs::read_to_string(&hook_path).unwrap_or_default();
    if !existing.contains(HEADER) {
        println!("pre-push hook not managed by Crucible; not removing");
        return Ok(());
    }

    fs::remove_file(&hook_path).context("remove pre-push hook")?;
    println!("Removed pre-push hook");
    Ok(())
}

fn status() -> Result<()> {
    let hook_path = hook_path()?;
    let installed = hook_path.exists() && fs::read_to_string(&hook_path).unwrap_or_default().contains(HEADER);
    let on_path = which::which("crucible").is_ok();
    println!("Hook installed: {}", if installed { "yes" } else { "no" });
    println!("crucible on PATH: {}", if on_path { "yes" } else { "no" });
    Ok(())
}

fn hook_path() -> Result<PathBuf> {
    let repo = Repository::discover(std::env::current_dir()?).context("discover repo")?;
    let git_dir = repo.path();
    Ok(git_dir.join("hooks/pre-push"))
}
