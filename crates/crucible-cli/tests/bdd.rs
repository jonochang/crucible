use assert_cmd::cargo::cargo_bin_cmd;
use cucumber::{World, given, then, when};
use git2::{IndexAddOption, Repository, Signature};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tempfile::TempDir;

#[derive(Debug, Default, World)]
struct CliWorld {
    output: Option<std::process::Output>,
    temp_dir: Option<TempDir>,
    repo_dir: Option<PathBuf>,
    export_path: Option<PathBuf>,
    report_path: Option<PathBuf>,
    interrupt_status: Option<i32>,
    interrupt_stderr: Option<String>,
}

fn run_cmd(args: &[&str], cwd: Option<&Path>) -> std::process::Output {
    let mut cmd = cargo_bin_cmd!("crucible");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args).output().expect("run crucible command")
}

#[given("an empty temp project")]
fn empty_temp_project(world: &mut CliWorld) {
    world.temp_dir = Some(TempDir::new().expect("temp dir"));
}

fn init_git_repo(world: &mut CliWorld) {
    let temp_dir = world
        .temp_dir
        .get_or_insert_with(|| TempDir::new().expect("temp dir"));
    let repo_dir = temp_dir.path();
    let repo = Repository::init(repo_dir).expect("init repo");
    repo.config()
        .and_then(|mut cfg| cfg.set_str("user.email", "bdd@example.com"))
        .expect("set git email");
    repo.config()
        .and_then(|mut cfg| cfg.set_str("user.name", "BDD Runner"))
        .expect("set git name");
    let readme = repo_dir.join("README.md");
    std::fs::write(&readme, "Hello\n").expect("write README");
    commit_all(&repo, "init");
    std::fs::write(&readme, "Hello\nWorld\n").expect("update README");
    world.repo_dir = Some(repo_dir.to_path_buf());
}

fn commit_all(repo: &Repository, message: &str) {
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"], IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("write index");
    let tree_id = index.write_tree().expect("write tree");
    let tree = repo.find_tree(tree_id).expect("find tree");
    let signature = Signature::now("BDD Runner", "bdd@example.com").expect("signature");

    let parent = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .and_then(|oid| repo.find_commit(oid).ok());
    let parents: Vec<&git2::Commit<'_>> = match parent.as_ref() {
        Some(p) => vec![p],
        None => Vec::new(),
    };

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )
    .expect("commit");
}

fn write_mock_agent_config(world: &mut CliWorld, sleep_secs: Option<u64>) {
    let repo_dir = world.repo_dir.as_ref().expect("repo dir");
    let mock_path = repo_dir.join("mock-agent.sh");
    let sleep_line = sleep_secs
        .map(|s| format!("sleep {s}\n"))
        .unwrap_or_default();
    let script = format!(
        r#"#!/usr/bin/env sh
cat >/dev/null
{sleep}cat <<'JSON'
{{"summary":"Mock summary","focus_items":[{{"area":"Mock","rationale":"Mock rationale"}}],"trade_offs":["none"],"findings":[{{"severity":"Info","file":"README.md","line_start":1,"line_end":1,"message":"Mock finding","confidence":"Low"}}],"unified_diff":"","explanation":""}}
JSON
"#,
        sleep = sleep_line
    );
    std::fs::write(&mock_path, script).expect("write mock agent");
    let mut perms = std::fs::metadata(&mock_path)
        .expect("metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_path, perms).expect("set permissions");
    }

    let config = format!(
        r#"[crucible]
version = "1"

[gate]
enabled = true
untangle_bin = "untangle"

[context]
reference_max_depth = 2
reference_max_files = 30
history_max_commits = 20
history_max_days = 30
docs_patterns = ["docs/**/*.md", "README.md", "ARCHITECTURE.md"]
docs_max_bytes = 50000

[coordinator]
max_rounds = 3
quorum_threshold = 0.75
agent_timeout_secs = 90
devil_advocate = false

[verdict]
block_on = "Critical"

[rate_limits]
anthropic_rpm = 50
google_rpm = 60
openai_rpm = 60

[plugins]
agents = ["claude-code"]
judge = "claude-code"
analyzer = "claude-code"
paths = []

[plugins.claude-code]
command = "{mock}"
args = []
persona = "Mock Reviewer"
role_weight = 1.0

[plugins.codex]
command = "{mock}"
args = []
persona = "Mock Reviewer"
role_weight = 1.0

[plugins.gemini]
command = "{mock}"
args = []
persona = "Mock Reviewer"
role_weight = 1.0

[plugins.open-code]
command = "{mock}"
args = []
persona = "Mock Reviewer"
role_weight = 1.0
"#,
        mock = mock_path.display()
    );
    std::fs::write(repo_dir.join(".crucible.toml"), config).expect("write config");
}

fn write_real_agent_config(world: &mut CliWorld) {
    let repo_dir = world.repo_dir.as_ref().expect("repo dir");
    let config = r#"[crucible]
version = "1"

[gate]
enabled = true
untangle_bin = "untangle"

[context]
reference_max_depth = 2
reference_max_files = 30
history_max_commits = 20
history_max_days = 30
docs_patterns = ["docs/**/*.md", "README.md", "ARCHITECTURE.md"]
docs_max_bytes = 50000

[coordinator]
max_rounds = 3
quorum_threshold = 0.75
agent_timeout_secs = 90
devil_advocate = false

[verdict]
block_on = "Critical"

[rate_limits]
anthropic_rpm = 50
google_rpm = 60
openai_rpm = 60

[plugins]
agents = ["claude-code", "codex", "gemini"]
judge = "claude-code"
analyzer = "claude-code"
paths = []

[plugins.claude-code]
command = "claude"
args = ["-p", "--output-format", "json"]
persona = "Security Auditor"
role_weight = 2.0

[plugins.codex]
command = "codex"
args = ["exec", "-", "--color", "never"]
persona = "Architecture Lead"
role_weight = 1.5

[plugins.gemini]
command = "gemini"
args = ["-y", "-o", "json"]
persona = "Performance Optimizer"
role_weight = 1.5

[plugins.open-code]
command = "opencode"
args = []
persona = "Correctness Reviewer"
role_weight = 1.0
"#;
    std::fs::write(repo_dir.join(".crucible.toml"), config).expect("write config");
}

#[when("I run config init")]
fn run_config_init(world: &mut CliWorld) {
    let temp_dir = world.temp_dir.as_ref().expect("temp dir");
    let output = run_cmd(&["config", "init"], Some(temp_dir.path()));
    world.output = Some(output);
}

#[when("I run review help")]
fn run_review_help(world: &mut CliWorld) {
    let output = run_cmd(&["review", "--help"], None);
    world.output = Some(output);
}

#[given("a git repo with a diff")]
fn git_repo_with_diff(world: &mut CliWorld) {
    init_git_repo(world);
}

#[given("a mock crucible config")]
fn mock_crucible_config(world: &mut CliWorld) {
    write_mock_agent_config(world, None);
}

#[given("a slow mock crucible config")]
fn slow_mock_crucible_config(world: &mut CliWorld) {
    write_mock_agent_config(world, Some(2));
}

#[given("a real agent crucible config")]
fn real_agent_config(world: &mut CliWorld) {
    write_real_agent_config(world);
}

#[when("I run review")]
fn run_review(world: &mut CliWorld) {
    let repo_dir = world.repo_dir.as_ref().expect("repo dir");
    let output = run_cmd(&["review"], Some(repo_dir));
    world.output = Some(output);
}

#[when("I run review with issue export")]
fn run_review_with_export(world: &mut CliWorld) {
    let repo_dir = world.repo_dir.as_ref().expect("repo dir");
    let export_path = repo_dir.join("issues.json");
    let export_arg = export_path.display().to_string();
    let output = run_cmd(
        &["review", "--export-issues", export_arg.as_str()],
        Some(repo_dir),
    );
    world.export_path = Some(export_path);
    world.output = Some(output);
}

#[when("I run review with report export")]
fn run_review_with_report_export(world: &mut CliWorld) {
    let repo_dir = world.repo_dir.as_ref().expect("repo dir");
    let report_path = repo_dir.join("report.json");
    let report_arg = report_path.display().to_string();
    let output = run_cmd(
        &["review", "--output-report", report_arg.as_str()],
        Some(repo_dir),
    );
    world.report_path = Some(report_path);
    world.output = Some(output);
}

#[when("I interrupt review")]
fn interrupt_review(world: &mut CliWorld) {
    let repo_dir = world.repo_dir.as_ref().expect("repo dir");
    let bin = assert_cmd::cargo::cargo_bin!("crucible");
    let mut cmd = std::process::Command::new(bin);
    cmd.current_dir(repo_dir);
    cmd.arg("review");
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::piped());
    let child = cmd.spawn().expect("spawn crucible");
    std::thread::sleep(Duration::from_millis(200));
    #[cfg(unix)]
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let output = child.wait_with_output().expect("wait on crucible");
    world.interrupt_status = output.status.code();
    world.interrupt_stderr = Some(String::from_utf8_lossy(&output.stderr).to_string());
}

#[then("the config file is created")]
fn config_file_created(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "command failed");
    let temp_dir = world.temp_dir.as_ref().expect("temp dir");
    let config_path = temp_dir.path().join(".crucible.toml");
    assert!(config_path.exists(), "config file missing");
    let contents = std::fs::read_to_string(config_path).expect("read config");
    assert!(contents.contains("[crucible]"));
}

#[then("the review help shows usage")]
fn review_help_shows_usage(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "command failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: crucible review"));
}

#[then("the review verdict is pass")]
fn review_verdict_pass(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "command failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Verdict: PASS"));
}

#[then("the review findings include the mock finding")]
fn review_has_mock_finding(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "command failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Mock finding"));
}

#[then("progress output is emitted")]
fn progress_output_emitted(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "command failed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[progress] analyzer:start"));
    assert!(stderr.contains("[progress] analyzer:done"));
    assert!(stderr.contains("[progress] round:1 start"));
    assert!(stderr.contains("[progress] round:1 status"));
    assert!(stderr.contains("[progress] agent:start round=1"));
    assert!(stderr.contains("[agent-review] round=1 id=claude-code"));
    assert!(stderr.contains("[progress] agent:done round=1"));
    assert!(stderr.contains("[progress] round:1 done"));
}

#[then("startup header is shown")]
fn startup_header_shown(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Configuration loaded"));
    assert!(stderr.contains("Found local changes"));
    assert!(stderr.contains("Reviewers:"));
    assert!(stderr.contains("Max rounds:"));
}

#[then("startup phase output is shown")]
fn startup_phase_output_shown(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[progress] startup:references"));
    assert!(stderr.contains("[progress] startup:history"));
    assert!(stderr.contains("[progress] startup:docs"));
    assert!(stderr.contains("[progress] startup:prechecks"));
}

#[then("round status output includes durations")]
fn round_status_with_durations(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[progress] round:1 status ["));
    assert!(stderr.contains("OK claude-code"));
    assert!(stderr.contains("s)"));
}

#[then("analysis section is shown")]
fn analysis_section_shown(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--- Analysis ---"));
}

#[then("system context section is shown")]
fn system_context_section_shown(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--- System Context ---"));
}

#[then("convergence output is shown")]
fn convergence_output_shown(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[progress] convergence round="));
    assert!(stderr.contains("-- Round"));
}

#[then("issues are exported with code locations")]
fn issues_exported_with_locations(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "command failed");
    let export_path = world.export_path.as_ref().expect("export path");
    assert!(export_path.exists(), "issues export missing");
    let raw = std::fs::read_to_string(export_path).expect("read issues export");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("parse issues export json");
    let arr = json.as_array().expect("issues array");
    assert!(!arr.is_empty(), "issues array is empty");
    let first = &arr[0];
    assert!(
        first
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("README.md:1")
    );
    assert!(first.get("raised_by").is_some(), "raised_by missing");
}

#[then("the full report artifact is written")]
fn full_report_artifact_written(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "command failed");
    let report_path = world.report_path.as_ref().expect("report path");
    assert!(report_path.exists(), "report export missing");
    let raw = std::fs::read_to_string(report_path).expect("read report export");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("parse report export json");
    assert!(json.get("run_id").is_some(), "run_id missing");
    assert!(json.get("verdict").is_some(), "verdict missing");
    assert!(
        json.get("analysis_markdown").is_some(),
        "analysis_markdown missing"
    );
}

#[then("the review exits with code 130")]
fn review_exits_130(world: &mut CliWorld) {
    let status = world.interrupt_status.expect("interrupt status");
    assert_eq!(status, 130);
    let stderr = world.interrupt_stderr.as_deref().unwrap_or("");
    assert!(stderr.contains("[progress] canceled"));
}

#[then("the review process completes successfully")]
fn review_process_completes_successfully(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "review should complete and exit");
}

#[then("the review output is valid")]
fn review_output_is_valid(world: &mut CliWorld) {
    let output = world.output.as_ref().expect("output available");
    assert!(output.status.success(), "command failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Crucible Review"));
}

fn main() {
    let include_real = std::env::var("CRUCIBLE_BDD_REAL_AGENTS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if include_real {
        futures::executor::block_on(
            CliWorld::cucumber()
                .with_default_cli()
                .run("tests/features"),
        );
    } else {
        futures::executor::block_on(
            CliWorld::cucumber()
                .with_default_cli()
                .filter_run("tests/features", |_, _, sc| {
                    !sc.tags.iter().any(|t| t == "real-agents")
                }),
        );
    }
}
