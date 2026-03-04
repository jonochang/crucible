use assert_cmd::cargo::cargo_bin_cmd;
use cucumber::{given, then, when, World};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[derive(Debug, Default, World)]
struct CliWorld {
    output: Option<std::process::Output>,
    temp_dir: Option<TempDir>,
    repo_dir: Option<PathBuf>,
}

fn run_cmd(args: &[&str], cwd: Option<&Path>) -> std::process::Output {
    let mut cmd = cargo_bin_cmd!("crucible-cli");
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.args(args).output().expect("run crucible-cli command")
}

#[given("an empty temp project")]
fn empty_temp_project(world: &mut CliWorld) {
    world.temp_dir = Some(TempDir::new().expect("temp dir"));
}

fn init_git_repo(world: &mut CliWorld) {
    let temp_dir = world.temp_dir.get_or_insert_with(|| TempDir::new().expect("temp dir"));
    let repo_dir = temp_dir.path();
    run_git(&["init"], repo_dir);
    run_git(&["config", "user.email", "bdd@example.com"], repo_dir);
    run_git(&["config", "user.name", "BDD Runner"], repo_dir);
    let readme = repo_dir.join("README.md");
    std::fs::write(&readme, "Hello\n").expect("write README");
    run_git(&["add", "README.md"], repo_dir);
    run_git(&["commit", "-m", "init"], repo_dir);
    std::fs::write(&readme, "Hello\nWorld\n").expect("update README");
    world.repo_dir = Some(repo_dir.to_path_buf());
}

fn run_git(args: &[&str], cwd: &Path) {
    let status = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git command failed: {:?}", args);
}

fn write_mock_agent_config(world: &mut CliWorld) {
    let repo_dir = world.repo_dir.as_ref().expect("repo dir");
    let mock_path = repo_dir.join("mock-agent.sh");
    std::fs::write(
        &mock_path,
        r#"#!/usr/bin/env sh
cat >/dev/null
cat <<'JSON'
{"summary":"Mock summary","focus_items":[{"area":"Mock","rationale":"Mock rationale"}],"trade_offs":["none"],"findings":[{"severity":"Info","file":"README.md","line_start":1,"line_end":1,"message":"Mock finding","confidence":"Low"}],"unified_diff":"","explanation":""}
JSON
"#,
    )
    .expect("write mock agent");
    let mut perms = std::fs::metadata(&mock_path).expect("metadata").permissions();
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
    write_mock_agent_config(world);
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
    assert!(stdout.contains("Usage: crucible-cli review"));
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
        futures::executor::block_on(CliWorld::cucumber().with_default_cli().run("tests/features"));
    } else {
        futures::executor::block_on(CliWorld::cucumber().with_default_cli().filter_run(
            "tests/features",
            |_, _, sc| !sc.tags.iter().any(|t| t == "real-agents"),
        ));
    }
}
