use crate::config::CrucibleConfig;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecheckSignal {
    pub tool: String,
    pub status: PrecheckStatus,
    pub summary: String,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrecheckStatus {
    Pass,
    Warn,
    Fail,
    Skipped,
}

pub fn collect_precheck_signals(
    repo_root: &Path,
    cfg: &CrucibleConfig,
) -> Result<Vec<PrecheckSignal>> {
    if !cfg.prechecks.enabled {
        return Ok(vec![PrecheckSignal {
            tool: "prechecks".to_string(),
            status: PrecheckStatus::Skipped,
            summary: "Prechecks disabled by config".to_string(),
            command: "n/a".to_string(),
        }]);
    }

    let mut signals = Vec::new();
    let has_rust = repo_root.join("Cargo.toml").exists();

    if cfg.prechecks.include_untangle {
        signals.extend(run_untangle_tools(
            repo_root,
            &cfg.gate.untangle_bin,
            cfg.prechecks.timeout_secs,
        ));
    }
    if has_rust && cfg.prechecks.include_linters {
        signals.push(run_tool(
            repo_root,
            "cargo",
            &["fmt", "--all", "--", "--check"],
            cfg.prechecks.timeout_secs,
            "cargo-fmt",
        ));
    }
    if has_rust && cfg.prechecks.include_type_checks {
        signals.push(run_tool(
            repo_root,
            "cargo",
            &["check", "--quiet"],
            cfg.prechecks.timeout_secs,
            "cargo-check",
        ));
    }
    if has_rust && cfg.prechecks.include_tests {
        signals.push(run_tool(
            repo_root,
            "cargo",
            &["test", "--quiet", "--no-run"],
            cfg.prechecks.timeout_secs,
            "cargo-test",
        ));
    }

    Ok(signals)
}

fn run_untangle_tools(repo_root: &Path, program: &str, timeout_secs: u64) -> Vec<PrecheckSignal> {
    if program == "crucible" {
        return vec![PrecheckSignal {
            tool: "untangle".to_string(),
            status: PrecheckStatus::Warn,
            summary: "Skipped: gate.untangle_bin points to crucible; set it to untangle binary"
                .to_string(),
            command: "crucible analyze report . --format json --quiet".to_string(),
        }];
    }
    // Verified against `untangle --help`: both analyze and quality require a nested subcommand.
    // Run the default structural report plus the engineer-facing quality report.
    vec![
        run_tool(
            repo_root,
            program,
            &["analyze", "report", ".", "--format", "json", "--quiet"],
            timeout_secs,
            "untangle-analyze-report",
        ),
        run_tool(
            repo_root,
            program,
            &["quality", "report", ".", "--format", "json", "--quiet"],
            timeout_secs,
            "untangle-quality-report",
        ),
    ]
}

fn run_tool(
    repo_root: &Path,
    program: &str,
    args: &[&str],
    timeout_secs: u64,
    tool_name: &str,
) -> PrecheckSignal {
    let command = format!("{program} {}", args.join(" "));
    let mut cmd = Command::new(program);
    cmd.current_dir(repo_root)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = match wait_with_timeout(cmd, Duration::from_secs(timeout_secs)) {
        Ok(Some(out)) => out,
        Ok(None) => {
            return PrecheckSignal {
                tool: tool_name.to_string(),
                status: PrecheckStatus::Warn,
                summary: format!("Timed out after {}s", timeout_secs),
                command,
            };
        }
        Err(err) => {
            return PrecheckSignal {
                tool: tool_name.to_string(),
                status: PrecheckStatus::Warn,
                summary: format!("Execution error: {err}"),
                command,
            };
        }
    };

    if output.status.success() {
        PrecheckSignal {
            tool: tool_name.to_string(),
            status: PrecheckStatus::Pass,
            summary: "Passed".to_string(),
            command,
        }
    } else {
        let mut excerpt = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if excerpt.is_empty() {
            excerpt = String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
        if excerpt.len() > 300 {
            let end = (0..=300)
                .rev()
                .find(|&idx| excerpt.is_char_boundary(idx))
                .unwrap_or(0);
            excerpt.truncate(end);
            excerpt.push('…');
        }
        PrecheckSignal {
            tool: tool_name.to_string(),
            status: PrecheckStatus::Fail,
            summary: if excerpt.is_empty() {
                "Failed".to_string()
            } else {
                excerpt
            },
            command,
        }
    }
}

fn wait_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Option<std::process::Output>> {
    use std::thread;
    use std::time::Instant;

    let mut child = cmd.spawn()?;
    let stdout_handle = child.stdout.take().map(spawn_reader);
    let stderr_handle = child.stderr.take().map(spawn_reader);
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(std::process::Output {
                status,
                stdout: join_reader(stdout_handle)?,
                stderr: join_reader(stderr_handle)?,
            }));
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = join_reader(stdout_handle);
            let _ = join_reader(stderr_handle);
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_reader<T>(mut stream: T) -> std::thread::JoinHandle<Vec<u8>>
where
    T: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        buf
    })
}

fn join_reader(
    handle: Option<std::thread::JoinHandle<Vec<u8>>>,
) -> Result<Vec<u8>> {
    match handle {
        Some(handle) => handle
            .join()
            .map_err(|_| anyhow::anyhow!("reader thread panicked")),
        None => Ok(Vec::new()),
    }
}
