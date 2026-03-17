use anyhow::{Context, Result, anyhow};
use clap::{Args, Subcommand};
use git2::Repository;
use std::fs;
use std::path::PathBuf;

const HEADER: &str =
    "# Managed by Crucible — do not edit manually (crucible hook uninstall to remove)";

#[derive(Args)]
pub struct HookArgs {
    #[command(subcommand)]
    pub command: HookCommand,
}

#[derive(Subcommand)]
pub enum HookCommand {
    Install {
        #[arg(long)]
        force: bool,
    },
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
    ensure_prerequisites()?;
    let hook_path = hook_path()?;
    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path).unwrap_or_default();
        if !existing.contains(HEADER) && !force {
            return Err(anyhow!("pre-push hook exists; use --force to overwrite"));
        }
    }

    let contents = format!(
        "#!/usr/bin/env bash\n{}\nset -euo pipefail\nexec just crucible-pre-push\n",
        HEADER
    );
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
    let installed = hook_path.exists()
        && fs::read_to_string(&hook_path)
            .unwrap_or_default()
            .contains(HEADER);
    let on_path = which::which("crucible").is_ok();
    let just_on_path = which::which("just").is_ok();
    let has_recipe = std::env::current_dir()
        .ok()
        .map(|dir| dir.join("Justfile").exists() || dir.join("justfile").exists())
        .unwrap_or(false);
    println!("Hook installed: {}", if installed { "yes" } else { "no" });
    println!("crucible on PATH: {}", if on_path { "yes" } else { "no" });
    println!("just on PATH: {}", if just_on_path { "yes" } else { "no" });
    println!(
        "crucible-pre-push recipe available: {}",
        if has_recipe { "yes" } else { "no" }
    );
    Ok(())
}

fn ensure_prerequisites() -> Result<()> {
    if which::which("just").is_err() {
        return Err(anyhow!("`just` is required for the managed pre-push hook"));
    }
    let cwd = std::env::current_dir().context("get current dir")?;
    let has_justfile = cwd.join("Justfile").exists() || cwd.join("justfile").exists();
    if !has_justfile {
        return Err(anyhow!(
            "managed hook expects a Justfile with a `crucible-pre-push` recipe"
        ));
    }
    Ok(())
}

fn hook_path() -> Result<PathBuf> {
    let repo = Repository::discover(std::env::current_dir()?).context("discover repo")?;
    let git_dir = repo.path();
    Ok(git_dir.join("hooks/pre-push"))
}
