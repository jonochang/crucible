use crate::log_helpers;
use anyhow::Result;
use clap::Args;
use git2::{DiffFormat, DiffOptions, Repository};
use libcrucible::config::CrucibleConfig;
use libcrucible::progress::ProgressEvent;
use libcrucible::report::{CanonicalIssue, ReviewReport, Severity, Verdict};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Args)]
pub struct ReviewArgs {
    #[arg(value_name = "PR", help = "GitHub PR number or URL to review")]
    pub pr: Option<String>,
    #[arg(long)]
    pub hook: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(
        long,
        help = "Export deduplicated issues list to a file (.json or .md)"
    )]
    pub export_issues: Option<PathBuf>,
    #[arg(long, help = "Write the full review report to a file (.json)")]
    pub output_report: Option<PathBuf>,
    #[arg(
        long,
        help = "Render the GitHub review payload without posting it (PR target only)"
    )]
    pub github_dry_run: bool,
    #[arg(
        long,
        help = "Publish the review to GitHub as a PR review (PR target only)"
    )]
    pub publish_github: bool,
    #[arg(long, help = "Enable verbose CLI agent logging")]
    pub verbose: bool,
    #[arg(long, help = "Enable debug logging to crucible.log")]
    pub debug: bool,
    #[arg(long, help = "Keep TUI open after completion; default is auto-exit")]
    pub interactive: bool,
    #[arg(long, help = "Run review with a single reviewer id (e.g. claude-code)")]
    pub reviewer: Option<String>,
    #[arg(long, help = "Override maximum review rounds")]
    pub max_rounds: Option<u8>,
    #[arg(long, help = "Review local uncommitted diff (git diff HEAD)")]
    pub local: bool,
    #[arg(
        long,
        help = "Review branch diff against remote default branch (e.g. origin/main...HEAD)"
    )]
    pub repo: bool,
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "main",
        help = "Review current branch against base branch (default: main)"
    )]
    pub branch: Option<String>,
    #[arg(long, help = "Review specific files (git diff HEAD -- <files...>)")]
    pub files: Vec<PathBuf>,
    #[arg(
        long,
        default_value = "origin",
        help = "Git remote to use for --repo base and PR checkout"
    )]
    pub git_remote: String,
}

pub async fn run(args: ReviewArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let run_id = Uuid::new_v4();
    let artifacts = libcrucible::artifacts::RunArtifacts::create(&cwd, run_id)?;
    if args.debug {
        let path = artifacts.debug_log.clone();
        std::fs::write(&path, b"")?;
        libcrucible::plugins::set_debug_log(&path)?;
        eprintln!(
            "Debug logging enabled: {} (run {})",
            path.display(),
            artifacts.run_id
        );
    }
    if args.verbose {
        libcrucible::plugins::set_verbose(true);
    }
    let mut cfg = CrucibleConfig::load()?;
    if let Some(reviewer) = &args.reviewer {
        cfg.plugins.agents = vec![reviewer.clone()];
        cfg.plugins.analyzer = reviewer.clone();
        cfg.plugins.judge = reviewer.clone();
        if args.max_rounds.is_none() {
            cfg.coordinator.max_rounds = 1;
            cfg.coordinator.quorum_threshold = 1.0;
        }
    }
    if let Some(rounds) = args.max_rounds {
        cfg.coordinator.max_rounds = rounds.max(1);
    }
    if args.github_dry_run && args.publish_github {
        anyhow::bail!("choose only one GitHub action: --github-dry-run or --publish-github");
    }
    let mode_count = u8::from(args.local)
        + u8::from(args.repo)
        + u8::from(args.pr.is_some())
        + u8::from(args.branch.is_some())
        + u8::from(!args.files.is_empty());
    if mode_count > 1 {
        anyhow::bail!("choose only one target mode: <PR>, --local, --repo, --branch, or --files");
    }
    let target = if let Some(pr) = &args.pr {
        ReviewTarget::PullRequest(pr.clone())
    } else if let Some(base) = &args.branch {
        ReviewTarget::Branch(base.clone())
    } else if !args.files.is_empty() {
        ReviewTarget::Files(
            args.files
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
        )
    } else if args.local {
        ReviewTarget::Local
    } else if args.repo {
        ReviewTarget::Repo
    } else {
        ReviewTarget::CurrentBranchWithLocal
    };
    if (args.github_dry_run || args.publish_github)
        && !matches!(target, ReviewTarget::PullRequest(_))
    {
        anyhow::bail!("GitHub review actions require a PR target");
    }
    let diff_override = resolve_review_diff(&target, &args.git_remote)?;
    if matches!(
        target,
        ReviewTarget::Local | ReviewTarget::CurrentBranchWithLocal
    ) {
        let diff = diff_override.as_deref().unwrap_or_default();
        if diff.trim().is_empty() || count_changed_lines(diff) == 0 {
            println!("No branch/local code changes detected; skipping review.");
            return Ok(());
        }
    }

    let use_tui = !args.hook
        && args.export_issues.is_none()
        && !args.json
        && !args.github_dry_run
        && !args.publish_github
        && std::io::stdout().is_terminal();
    if use_tui {
        let scope_label = describe_review_scope(&target, &args.git_remote)
            .unwrap_or_else(|_| "scope unavailable".to_string());
        let exit_code = crate::tui::run_review_tui(
            &cfg,
            args.interactive,
            diff_override.clone(),
            scope_label,
            args.output_report.clone(),
            artifacts.clone(),
        )
        .await?;
        std::process::exit(exit_code);
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let log = open_review_log(&artifacts)?;
    let log = Arc::new(Mutex::new(log));
    let cfg_for_review = cfg.clone();
    let run_id_for_review = artifacts.run_id;
    let mut review_handle = tokio::spawn(async move {
        if let Some(diff) = diff_override {
            libcrucible::run_review_with_progress_diff_run_id(
                &cfg_for_review,
                tx,
                diff,
                run_id_for_review,
            )
            .await
        } else {
            libcrucible::run_review_with_progress_run_id(&cfg_for_review, tx, run_id_for_review)
                .await
        }
    });
    let log_for_progress = log.clone();
    let run_id_for_progress = artifacts.run_id;
    let progress_handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            emit_progress(run_id_for_progress, &event);
            let _ = write_log_event(&log_for_progress, run_id_for_progress, &event);
        }
    });

    let report = tokio::select! {
        res = &mut review_handle => {
            let report = res??;
            let _ = progress_handle.await;
            report
        }
        _ = tokio::signal::ctrl_c() => {
            emit_progress(artifacts.run_id, &ProgressEvent::Canceled);
            let _ = write_log_event(&log, artifacts.run_id, &ProgressEvent::Canceled);
            review_handle.abort();
            std::process::exit(130);
        }
    };

    if args.json {
        let json = render_report_json(&report);
        println!("{json}");
        write_report_targets(&artifacts, args.output_report.as_ref(), &json)?;
        write_log_json(&log, artifacts.run_id, &json);
        write_log_report_sections(&log, artifacts.run_id, &report);
        if let Some(path) = &args.export_issues {
            export_issues(path, &build_issue_list(&report))?;
        }
        if args.github_dry_run || args.publish_github {
            let pr = match &target {
                ReviewTarget::PullRequest(pr) => pr,
                _ => unreachable!("validated PR target above"),
            };
            handle_github_review_action(
                &report,
                pr,
                args.github_dry_run,
                args.publish_github,
                true,
            )?;
        }
        return Ok(());
    }

    let issues = build_issue_list(&report);
    print_report(&report, &issues);
    if let Some(path) = &args.export_issues {
        export_issues(path, &issues)?;
        println!("\nExported issues to {}", path.display());
    }
    let json = render_report_json(&report);
    write_report_targets(&artifacts, args.output_report.as_ref(), &json)?;
    if let Some(path) = &args.output_report {
        println!("Report written to {}", path.display());
    }
    println!("Run artifacts: {}", artifacts.run_dir.display());
    write_log_json(&log, artifacts.run_id, &json);
    write_log_report_sections(&log, artifacts.run_id, &report);
    if args.github_dry_run || args.publish_github {
        let pr = match &target {
            ReviewTarget::PullRequest(pr) => pr,
            _ => unreachable!("validated PR target above"),
        };
        handle_github_review_action(&report, pr, args.github_dry_run, args.publish_github, false)?;
    }

    if args.hook {
        let code = match report.verdict {
            Verdict::Block => 1,
            _ => 0,
        };
        std::process::exit(code);
    }

    Ok(())
}

#[derive(Debug, Clone)]
enum ReviewTarget {
    /// Review current branch delta (vs base branch) plus local worktree/index changes.
    CurrentBranchWithLocal,
    /// Review uncommitted local changes from working tree/index.
    Local,
    /// Review current branch against remote default branch.
    Repo,
    /// Review current branch against explicit base branch.
    Branch(String),
    /// Review only selected files.
    Files(Vec<String>),
    /// Review a GitHub pull request (number or URL).
    PullRequest(String),
}

#[derive(Debug, Deserialize)]
struct GhPrView {
    number: u64,
    url: String,
    #[serde(rename = "headRefOid")]
    head_ref_oid: String,
}

#[derive(Debug, Deserialize)]
struct GhRepoView {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug)]
struct GithubReviewContext {
    number: u64,
    url: String,
    repo: String,
    head_sha: String,
}

#[derive(Debug, Deserialize)]
struct ExistingGithubReview {
    body: Option<String>,
    #[serde(rename = "html_url")]
    html_url: Option<String>,
}

/// Resolve the concrete diff content for the selected target mode.
/// Returns `Ok(None)` only for empty local diffs so callers can skip gracefully.
fn resolve_review_diff(target: &ReviewTarget, git_remote: &str) -> Result<Option<String>> {
    let repo = Repository::discover(".")?;
    match target {
        ReviewTarget::CurrentBranchWithLocal => {
            if let Some(base_ref) = resolve_base_ref_for_current_branch(&repo, git_remote)? {
                let diff = diff_base_to_workdir_with_index(&repo, &base_ref, "HEAD")?;
                if diff.trim().is_empty() {
                    return Ok(None);
                }
                Ok(Some(diff))
            } else {
                let diff = diff_worktree_vs_head(&repo)?;
                if diff.trim().is_empty() {
                    return Ok(None);
                }
                Ok(Some(diff))
            }
        }
        ReviewTarget::Local => {
            let diff = diff_worktree_vs_head(&repo)?;
            if diff.trim().is_empty() {
                return Ok(None);
            }
            Ok(Some(diff))
        }
        ReviewTarget::Repo => {
            let base_ref = resolve_base_ref_for_current_branch(&repo, git_remote)?
                .ok_or_else(|| anyhow::anyhow!("unable to resolve base branch for --repo"))?;
            let diff = diff_three_dot_refs(&repo, &base_ref, "HEAD")?;
            if diff.trim().is_empty() {
                anyhow::bail!("no repo diff found for range {}...HEAD", base_ref);
            }
            Ok(Some(diff))
        }
        ReviewTarget::Branch(base) => {
            let diff = diff_three_dot_refs(&repo, base, "HEAD")?;
            if diff.trim().is_empty() {
                anyhow::bail!("no branch diff found for range {}...HEAD", base);
            }
            Ok(Some(diff))
        }
        ReviewTarget::Files(files) => {
            let diff = diff_worktree_vs_head_for_files(&repo, files)?;
            if diff.trim().is_empty() {
                anyhow::bail!("no diff found for requested files");
            }
            Ok(Some(diff))
        }
        ReviewTarget::PullRequest(pr) => {
            checkout_pr_branch(pr)?;
            let diff = run_cmd_capture("gh", &["pr", "diff", pr.as_str()])?;
            if diff.trim().is_empty() {
                anyhow::bail!("no diff returned for PR {}", pr);
            }
            Ok(Some(diff))
        }
    }
}

fn resolve_remote_default_branch(repo: &Repository, git_remote: &str) -> Result<String> {
    let ref_name = format!("refs/remotes/{git_remote}/HEAD");
    let reference = repo.find_reference(&ref_name)?;
    let target = reference
        .symbolic_target()
        .ok_or_else(|| anyhow::anyhow!("remote HEAD is not symbolic for {}", git_remote))?;
    let prefix = format!("refs/remotes/{git_remote}/");
    Ok(target.strip_prefix(&prefix).unwrap_or("main").to_string())
}

fn resolve_base_ref_for_current_branch(
    repo: &Repository,
    git_remote: &str,
) -> Result<Option<String>> {
    if let Ok(branch) = resolve_remote_default_branch(repo, git_remote) {
        return Ok(Some(format!("refs/remotes/{git_remote}/{branch}")));
    }
    if repo.find_reference("refs/heads/main").is_ok() {
        return Ok(Some("refs/heads/main".to_string()));
    }
    if repo.find_reference("refs/heads/master").is_ok() {
        return Ok(Some("refs/heads/master".to_string()));
    }
    Ok(None)
}

/// Checkout the PR branch into the current repository using GitHub CLI.
fn checkout_pr_branch(pr: &str) -> Result<()> {
    let status = std::process::Command::new("gh")
        .args(["pr", "checkout", pr])
        .status()?;
    if !status.success() {
        anyhow::bail!("failed to checkout PR branch via `gh pr checkout {}`", pr);
    }
    Ok(())
}

/// Run a command and capture stdout, surfacing stderr on failure.
fn run_cmd_capture(program: &str, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{} {:?} failed: {}", program, args, stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_cmd_capture_with_stdin(program: &str, args: &[&str], stdin: &str) -> Result<String> {
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(input) = child.stdin.as_mut() {
        input.write_all(stdin.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{} {:?} failed: {}", program, args, stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_cmd_capture_json<T: for<'de> Deserialize<'de>>(program: &str, args: &[&str]) -> Result<T> {
    let stdout = run_cmd_capture(program, args)?;
    Ok(serde_json::from_str(&stdout)?)
}

fn describe_review_scope(target: &ReviewTarget, git_remote: &str) -> Result<String> {
    let repo = Repository::discover(".")?;
    let head_branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(|s| s.to_string()))
        .unwrap_or_else(|| "detached".to_string());
    let label = match target {
        ReviewTarget::CurrentBranchWithLocal => {
            let base = resolve_base_ref_for_current_branch(&repo, git_remote)?
                .unwrap_or_else(|| "HEAD".to_string());
            format!("Mode: branch+local | head: {head_branch} | base: {base}")
        }
        ReviewTarget::Local => format!("Mode: local | head: {head_branch}"),
        ReviewTarget::Repo => {
            let base = resolve_base_ref_for_current_branch(&repo, git_remote)?
                .unwrap_or_else(|| format!("refs/remotes/{git_remote}/HEAD"));
            format!("Mode: repo | head: {head_branch} | base: {base}")
        }
        ReviewTarget::Branch(base) => {
            format!("Mode: branch | head: {head_branch} | base: {base}")
        }
        ReviewTarget::Files(files) => {
            format!(
                "Mode: files | head: {head_branch} | files: {}",
                files.join(", ")
            )
        }
        ReviewTarget::PullRequest(pr) => format!("Mode: pr | PR: {pr}"),
    };
    Ok(label)
}

fn diff_worktree_vs_head(repo: &Repository) -> Result<String> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut opts = DiffOptions::new();
    opts.include_untracked(true);
    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;
    render_diff_to_patch(&diff)
}

fn diff_worktree_vs_head_for_files(repo: &Repository, files: &[String]) -> Result<String> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut opts = DiffOptions::new();
    opts.include_untracked(true);
    for file in files {
        opts.pathspec(file);
    }
    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;
    render_diff_to_patch(&diff)
}

fn diff_three_dot_refs(repo: &Repository, base_ref: &str, head_ref: &str) -> Result<String> {
    let base_commit = peel_commit(repo, base_ref)?;
    let head_commit = peel_commit(repo, head_ref)?;
    let merge_base = repo.merge_base(base_commit.id(), head_commit.id())?;
    let merge_base_tree = repo.find_commit(merge_base)?.tree()?;
    let head_tree = head_commit.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&head_tree), None)?;
    render_diff_to_patch(&diff)
}

fn diff_base_to_workdir_with_index(
    repo: &Repository,
    base_ref: &str,
    head_ref: &str,
) -> Result<String> {
    let base_commit = peel_commit(repo, base_ref)?;
    let head_commit = peel_commit(repo, head_ref)?;
    let merge_base = repo.merge_base(base_commit.id(), head_commit.id())?;
    let merge_base_tree = repo.find_commit(merge_base)?.tree()?;
    let mut opts = DiffOptions::new();
    opts.include_untracked(true);
    let diff = repo.diff_tree_to_workdir_with_index(Some(&merge_base_tree), Some(&mut opts))?;
    render_diff_to_patch(&diff)
}

fn peel_commit<'a>(repo: &'a Repository, reference: &str) -> Result<git2::Commit<'a>> {
    let obj = repo.revparse_single(reference)?;
    let commit = obj.peel_to_commit()?;
    Ok(commit)
}

fn render_diff_to_patch(diff: &git2::Diff<'_>) -> Result<String> {
    let mut out = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        if let Ok(content) = std::str::from_utf8(line.content()) {
            push_diff_line(&mut out, line.origin(), content);
        }
        true
    })?;
    Ok(out)
}

fn resolve_github_review_context(pr: &str) -> Result<GithubReviewContext> {
    let pr_view: GhPrView =
        run_cmd_capture_json("gh", &["pr", "view", pr, "--json", "number,url,headRefOid"])?;
    let repo_view: GhRepoView =
        run_cmd_capture_json("gh", &["repo", "view", "--json", "nameWithOwner"])?;
    Ok(GithubReviewContext {
        number: pr_view.number,
        url: pr_view.url,
        repo: repo_view.name_with_owner,
        head_sha: pr_view.head_ref_oid,
    })
}

fn handle_github_review_action(
    report: &ReviewReport,
    pr: &str,
    github_dry_run: bool,
    publish_github: bool,
    use_stderr: bool,
) -> Result<()> {
    let ctx = resolve_github_review_context(pr)?;
    if github_dry_run {
        let rendered = render_github_dry_run(report, &ctx)?;
        if use_stderr {
            eprintln!("{rendered}");
        } else {
            println!("\n{rendered}");
        }
        return Ok(());
    }
    if publish_github {
        match publish_github_review(report, &ctx)? {
            Some(url) => {
                if use_stderr {
                    eprintln!("Published GitHub review: {url}");
                } else {
                    println!("\nPublished GitHub review: {url}");
                }
            }
            None => {
                if use_stderr {
                    eprintln!(
                        "Skipped GitHub review publish; matching review already exists for {}",
                        ctx.url
                    );
                } else {
                    println!(
                        "\nSkipped GitHub review publish; matching review already exists for {}",
                        ctx.url
                    );
                }
            }
        }
    }
    Ok(())
}

fn render_github_dry_run(report: &ReviewReport, ctx: &GithubReviewContext) -> Result<String> {
    let draft = report
        .pr_review_draft
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("report did not include a PR review draft"))?;
    let mut out = format!(
        "GitHub review dry run\nPR: {}\nRepo: {}\nInline comments: {}\nOverview-only comments: {}\n",
        ctx.url,
        ctx.repo,
        draft.inline_comments.len(),
        draft.overview_only_comments.len()
    );
    out.push_str("\nOverview:\n");
    out.push_str(&draft.overview_comment.body);
    out.push_str("\n\nInline comments:\n");
    for comment in &draft.inline_comments {
        let path = comment
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let line = comment
            .line
            .map(|line| line.to_string())
            .unwrap_or_else(|| "?".to_string());
        out.push_str(&format!(
            "- {}:{} {}\n{}\n",
            path, line, comment.title, comment.body
        ));
    }
    if !draft.overview_only_comments.is_empty() {
        out.push_str("\nOverview-only comments:\n");
        for comment in &draft.overview_only_comments {
            out.push_str(&format!(
                "- {} ({})\n",
                comment.title,
                comment.mapping_note.clone().unwrap_or_default()
            ));
        }
    }
    Ok(out)
}

fn publish_github_review(
    report: &ReviewReport,
    ctx: &GithubReviewContext,
) -> Result<Option<String>> {
    let draft = report
        .pr_review_draft
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("report did not include a PR review draft"))?;
    let marker = format!("<!-- crucible-head:{} -->", ctx.head_sha);
    let endpoint = format!("repos/{}/pulls/{}/reviews", ctx.repo, ctx.number);
    let existing: Vec<ExistingGithubReview> =
        run_cmd_capture_json("gh", &["api", endpoint.as_str()])?;
    if existing
        .iter()
        .any(|review| review.body.as_deref().unwrap_or("").contains(&marker))
    {
        let _ = existing
            .iter()
            .find_map(|review| review.html_url.as_deref());
        return Ok(None);
    }

    let mut body = draft.overview_comment.body.clone();
    if !draft.overview_only_comments.is_empty() {
        body.push_str("\n\n### Additional Issues Not Mapped To Diff Hunks\n");
        for comment in &draft.overview_only_comments {
            body.push_str(&format!(
                "- **{}**: {}\n",
                comment.title, comment.description
            ));
        }
    }
    body.push_str(&format!(
        "\n\n<!-- crucible-run:{} -->\n{}\n",
        report.run_id, marker
    ));

    let comments = draft
        .inline_comments
        .iter()
        .filter_map(|comment| {
            Some(serde_json::json!({
                "path": comment.path.as_ref()?.display().to_string(),
                "body": comment.body,
                "line": comment.line?,
                "side": match comment.side? {
                    libcrucible::report::PullRequestCommentSide::Left => "LEFT",
                    libcrucible::report::PullRequestCommentSide::Right => "RIGHT",
                },
                "start_line": comment.start_line,
                "start_side": comment.start_side.map(|side| match side {
                    libcrucible::report::PullRequestCommentSide::Left => "LEFT",
                    libcrucible::report::PullRequestCommentSide::Right => "RIGHT",
                }),
            }))
        })
        .collect::<Vec<_>>();
    let payload = serde_json::to_string_pretty(&serde_json::json!({
        "body": body,
        "event": "COMMENT",
        "commit_id": ctx.head_sha,
        "comments": comments,
    }))?;
    let response = run_cmd_capture_with_stdin(
        "gh",
        &["api", "--method", "POST", endpoint.as_str(), "--input", "-"],
        &payload,
    )?;
    let response_json: serde_json::Value = serde_json::from_str(&response)?;
    Ok(Some(
        response_json
            .get("html_url")
            .and_then(|value| value.as_str())
            .unwrap_or(&ctx.url)
            .to_string(),
    ))
}

fn count_changed_lines(diff: &str) -> usize {
    diff.lines()
        .filter(|line| line.starts_with('+') || line.starts_with('-'))
        .filter(|line| !line.starts_with("+++ ") && !line.starts_with("--- "))
        .count()
}

fn print_report(report: &ReviewReport, issues: &[IssueRow]) {
    let mut critical = 0;
    let mut warning = 0;
    let mut info = 0;
    for f in &report.findings {
        match f.severity {
            libcrucible::report::Severity::Critical => critical += 1,
            libcrucible::report::Severity::Warning => warning += 1,
            libcrucible::report::Severity::Info => info += 1,
        }
    }

    println!();
    println!(
        "Issues Found ({} unique, {} total across reviewers)",
        issues.len(),
        report.findings.len()
    );
    for (idx, issue) in issues.iter().enumerate() {
        println!(
            "  {:>2}. [{:8}] {} {} [{}]",
            idx + 1,
            format_severity(&issue.severity),
            issue.location,
            issue.title,
            issue.raised_by.join(", ")
        );
        println!("      {}", issue.description);
        if let Some(fix) = &issue.suggested_fix {
            println!("      fix: {}", fix);
        }
    }

    println!(
        "Crucible Review — {} findings ({} Critical, {} Warning, {} Info)",
        report.findings.len(),
        critical,
        warning,
        info
    );
    println!();
    for f in &report.findings {
        let loc = match (&f.file, &f.span) {
            (Some(file), Some(span)) => format!("{}:{}", file.display(), span.start),
            (Some(file), None) => file.display().to_string(),
            _ => "<unknown>".to_string(),
        };
        println!(
            "  [{:8}]  {:20}  {}",
            format_severity(&f.severity),
            loc,
            f.message
        );
    }

    if let Some(final_analysis) = &report.final_analysis_markdown {
        println!("\nFinal Analysis:\n{}", final_analysis);
    }

    if let Some(plan) = &report.final_action_plan {
        println!("\nAction Plan:");
        for step in &plan.prioritized_steps {
            println!("  - {}", step);
        }
        if !plan.quick_wins.is_empty() {
            println!("Quick Wins:");
            for step in &plan.quick_wins {
                println!("  - {}", step);
            }
        }
    }

    if let Some(comment) = &report.pr_comment_markdown {
        println!("\nPR Comment Artifact:\n{}", comment);
    }

    if report.auto_fix.is_some() {
        println!();
        println!("Auto-fix available. Run with TUI to apply: crucible review");
    }

    match report.verdict {
        Verdict::Block => println!("\nVerdict: BLOCK"),
        Verdict::Warn => println!("\nVerdict: WARN"),
        Verdict::Pass => println!("\nVerdict: PASS"),
    }
}

fn format_severity(sev: &libcrucible::report::Severity) -> &'static str {
    match sev {
        libcrucible::report::Severity::Critical => "CRITICAL",
        libcrucible::report::Severity::Warning => "WARNING",
        libcrucible::report::Severity::Info => "INFO",
    }
}

#[derive(Debug, Clone)]
struct IssueRow {
    severity: Severity,
    category: Option<String>,
    file: Option<String>,
    line_start: Option<u32>,
    line_end: Option<u32>,
    location: String,
    title: String,
    description: String,
    suggested_fix: Option<String>,
    raised_by: Vec<String>,
    evidence: Vec<serde_json::Value>,
}

fn build_issue_list(report: &ReviewReport) -> Vec<IssueRow> {
    if !report.issues.is_empty() {
        return build_issue_rows_from_canonical(&report.issues);
    }

    struct Group {
        raised_by: BTreeSet<String>,
        severity: Severity,
        file: Option<String>,
        line_start: Option<u32>,
        line_end: Option<u32>,
        message: String,
    }
    let mut grouped: BTreeMap<(String, Option<String>, Option<u32>, Option<u32>, String), Group> =
        BTreeMap::new();
    for finding in &report.findings {
        let file = finding.file.as_ref().map(|p| p.display().to_string());
        let line_start = finding.span.as_ref().map(|s| s.start);
        let line_end = finding.span.as_ref().map(|s| s.end);
        let loc = match (&file, line_start, line_end) {
            (Some(f), Some(s), Some(e)) if s != e => format!("{f}:{s}-{e}"),
            (Some(f), Some(s), _) => format!("{f}:{s}"),
            (Some(f), None, _) => f.clone(),
            _ => "<unknown>".to_string(),
        };
        let key = (
            format_severity(&finding.severity).to_string(),
            file.clone().map(|f| normalize_dedup_text(&f)),
            line_start,
            line_end,
            normalize_dedup_text(&finding.message),
        );
        grouped
            .entry(key)
            .and_modify(|group| {
                group.raised_by.insert(finding.agent.clone());
            })
            .or_insert_with(|| {
                let mut raised_by = BTreeSet::new();
                raised_by.insert(finding.agent.clone());
                Group {
                    raised_by,
                    severity: finding.severity.clone(),
                    file: file.clone(),
                    line_start,
                    line_end,
                    message: finding.message.clone(),
                }
            });
        let _ = loc;
    }

    let mut issues = grouped
        .into_iter()
        .map(
            |((_severity, _file_key, _line_start, _line_end, _message_key), group)| {
                let location = match (&group.file, group.line_start, group.line_end) {
                    (Some(f), Some(s), Some(e)) if s != e => format!("{f}:{s}-{e}"),
                    (Some(f), Some(s), _) => format!("{f}:{s}"),
                    (Some(f), None, _) => f.clone(),
                    _ => "<unknown>".to_string(),
                };
                IssueRow {
                    severity: group.severity,
                    category: None,
                    file: group.file.clone(),
                    line_start: group.line_start,
                    line_end: group.line_end,
                    location,
                    title: group.message.clone(),
                    description: group.message,
                    suggested_fix: None,
                    raised_by: group.raised_by.into_iter().collect(),
                    evidence: Vec::new(),
                }
            },
        )
        .collect::<Vec<_>>();

    issues.sort_by(|a, b| {
        let sa = severity_rank(&a.severity);
        let sb = severity_rank(&b.severity);
        sb.cmp(&sa)
            .then(a.location.cmp(&b.location))
            .then(a.title.cmp(&b.title))
    });
    issues
}

fn build_issue_rows_from_canonical(issues: &[CanonicalIssue]) -> Vec<IssueRow> {
    let mut rows = issues
        .iter()
        .map(|issue| {
            let file = issue.file.as_ref().map(|p| p.display().to_string());
            let location = match (&file, issue.line_start, issue.line_end) {
                (Some(f), Some(s), Some(e)) if s != e => format!("{f}:{s}-{e}"),
                (Some(f), Some(s), _) => format!("{f}:{s}"),
                (Some(f), None, _) => f.clone(),
                _ => "<unknown>".to_string(),
            };
            IssueRow {
                severity: issue.severity.clone(),
                category: Some(issue.category.clone()),
                file,
                line_start: issue.line_start,
                line_end: issue.line_end,
                location,
                title: issue.title.clone(),
                description: issue.description.clone(),
                suggested_fix: issue.suggested_fix.clone(),
                raised_by: issue.raised_by.clone(),
                evidence: issue
                    .evidence
                    .iter()
                    .map(|e| serde_json::json!({"location": e.location, "quote": e.quote}))
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        let sa = severity_rank(&a.severity);
        let sb = severity_rank(&b.severity);
        sb.cmp(&sa)
            .then(a.location.cmp(&b.location))
            .then(a.title.cmp(&b.title))
    });
    rows
}

fn normalize_dedup_text(value: &str) -> String {
    value
        .trim()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn severity_rank(sev: &Severity) -> u8 {
    match sev {
        Severity::Critical => 3,
        Severity::Warning => 2,
        Severity::Info => 1,
    }
}

fn export_issues(path: &std::path::Path, issues: &[IssueRow]) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    if ext.eq_ignore_ascii_case("md") {
        let mut out = String::new();
        out.push_str("# Crucible Issues\n\n");
        for (idx, i) in issues.iter().enumerate() {
            out.push_str(&format!(
                "{}. [{}] `{}` {}\n",
                idx + 1,
                format_severity(&i.severity),
                i.location,
                i.title
            ));
            out.push_str(&format!("   - raised_by: {}\n", i.raised_by.join(", ")));
            out.push_str(&format!("   - description: {}\n", i.description));
            if let Some(category) = &i.category {
                out.push_str(&format!("   - category: {}\n", category));
            }
            if let Some(fix) = &i.suggested_fix {
                out.push_str(&format!("   - suggested_fix: {}\n", fix));
            }
        }
        std::fs::write(path, out)?;
        return Ok(());
    }

    let json = serde_json::to_string_pretty(
        &issues
            .iter()
            .map(|i| {
                serde_json::json!({
                    "severity": format_severity(&i.severity),
                    "category": i.category,
                    "file": i.file,
                    "line_start": i.line_start,
                    "line_end": i.line_end,
                    "location": i.location,
                    "title": i.title,
                    "description": i.description,
                    "suggested_fix": i.suggested_fix,
                    "raised_by": i.raised_by,
                    "evidence": i.evidence,
                })
            })
            .collect::<Vec<_>>(),
    )?;
    std::fs::write(path, json)?;
    Ok(())
}

fn emit_progress(run_id: Uuid, event: &ProgressEvent) {
    render_spinner_status(event);
    eprint!("[run:{}] ", run_id);
    let _ = std::io::stderr().flush();
    match event {
        ProgressEvent::RunHeader {
            reviewers,
            max_rounds,
            changed_lines,
            ..
        } => {
            eprintln!("Configuration loaded");
            eprintln!("Found local changes ({} lines)", changed_lines);
            eprintln!("Reviewers: {}", reviewers.join(", "));
            eprintln!("Max rounds: {}", max_rounds);
        }
        ProgressEvent::PhaseStart { phase } => eprintln!("[progress] phase:start {}", phase),
        ProgressEvent::PhaseDone { phase } => eprintln!("[progress] phase:done {}", phase),
        ProgressEvent::AnalyzerStart => eprintln!("[progress] analyzer:start"),
        ProgressEvent::AnalyzerDone => eprintln!("[progress] analyzer:done"),
        ProgressEvent::StartupPhase {
            phase,
            status,
            count,
            duration_secs,
            detail,
        } => {
            let count_suffix = count
                .map(|value| format!(" count={value}"))
                .unwrap_or_default();
            let duration_suffix = duration_secs
                .map(|value| format!(" duration={}", log_helpers::format_duration(value)))
                .unwrap_or_default();
            eprintln!(
                "[progress] startup:{} {}{}{} {}",
                log_helpers::format_startup_phase(*phase),
                log_helpers::format_startup_status(*status),
                count_suffix,
                duration_suffix,
                detail
            );
        }
        ProgressEvent::AnalysisReady { markdown } => {
            eprintln!("\n--- Analysis ---\n{}\n", markdown);
        }
        ProgressEvent::SystemContextReady { markdown } => {
            eprintln!("--- System Context ---\n{}\n", markdown);
        }
        ProgressEvent::RoundStart { round, agents, .. } => {
            eprintln!(
                "[progress] round:{} start (agents: {})",
                round,
                agents.join(",")
            );
        }
        ProgressEvent::ParallelStatus { round, statuses } => {
            eprintln!(
                "[progress] round:{} status {}",
                round,
                log_helpers::format_parallel_status(statuses)
            );
        }
        ProgressEvent::AgentStart { round, id } => {
            eprintln!("[progress] agent:start round={} id={}", round, id);
        }
        ProgressEvent::AgentReview {
            round,
            id,
            summary,
            highlights,
            details,
        } => {
            eprintln!("[agent-review] round={} id={} {}", round, id, summary);
            eprintln!("[agent-review] details:\n{}", details);
            for h in highlights {
                eprintln!(
                    "[agent-review]   [{}] {} {}",
                    h.severity, h.location, h.message
                );
            }
        }
        ProgressEvent::AgentDone { round, id } => {
            eprintln!("[progress] agent:done round={} id={}", round, id);
        }
        ProgressEvent::AgentError { round, id, message } => {
            eprintln!(
                "[progress] agent:error round={} id={} msg={}",
                round, id, message
            );
        }
        ProgressEvent::RoundDone { round } => eprintln!("[progress] round:{} done", round),
        ProgressEvent::ConvergenceJudgment {
            round,
            verdict,
            rationale,
        } => {
            eprintln!(
                "[progress] convergence round={} verdict={} rationale={}",
                round,
                log_helpers::format_convergence(*verdict),
                rationale
            );
        }
        ProgressEvent::RoundComplete {
            round,
            total_rounds,
        } => {
            eprintln!("-- Round {}/{} complete --", round, total_rounds);
        }
        ProgressEvent::AutoFixReady => eprintln!("[progress] autofix:ready"),
        ProgressEvent::Completed(_) => {}
        ProgressEvent::Canceled => eprintln!("[progress] canceled"),
    }
}

#[derive(Default)]
struct SpinnerState {
    frame: usize,
    phase: String,
    round: Option<u8>,
    statuses: String,
}

fn spinner_state() -> &'static Mutex<SpinnerState> {
    static STATE: OnceLock<Mutex<SpinnerState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(SpinnerState::default()))
}

fn render_spinner_status(event: &ProgressEvent) {
    if !std::io::stderr().is_terminal() {
        return;
    }
    let mut state = spinner_state().lock().expect("spinner lock");
    match event {
        ProgressEvent::PhaseStart { phase } => state.phase = phase.clone(),
        ProgressEvent::StartupPhase { phase, status, .. } => {
            state.phase = format!(
                "startup:{} {}",
                log_helpers::format_startup_phase(*phase),
                log_helpers::format_startup_status(*status)
            );
        }
        ProgressEvent::RoundStart { round, .. } => state.round = Some(*round),
        ProgressEvent::ParallelStatus { statuses, .. } => {
            state.statuses = log_helpers::format_parallel_status(statuses);
        }
        ProgressEvent::PhaseDone { phase } => state.phase = format!("{phase} done"),
        ProgressEvent::Completed(_) | ProgressEvent::Canceled => {
            eprintln!();
            return;
        }
        _ => {}
    }
    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let frame = FRAMES[state.frame % FRAMES.len()];
    state.frame = state.frame.wrapping_add(1);

    let round = state
        .round
        .map(|r| format!("round {r}"))
        .unwrap_or_else(|| "startup".to_string());
    let phase = if state.phase.is_empty() {
        "initializing".to_string()
    } else {
        state.phase.clone()
    };
    let status = if state.statuses.is_empty() {
        String::new()
    } else {
        format!(" | {}", state.statuses)
    };
    let line = format!("\r\x1b[36m{frame}\x1b[0m \x1b[33m{phase}\x1b[0m ({round}){status}");
    eprint!("{line}");
    let _ = std::io::stderr().flush();
}

struct ReviewLog {
    sinks: Vec<File>,
}

fn open_review_log(artifacts: &libcrucible::artifacts::RunArtifacts) -> Result<ReviewLog> {
    let legacy_path = std::env::current_dir()?.join("review_report.log");
    let legacy = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(legacy_path)?;
    let scoped = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&artifacts.progress_log)?;
    Ok(ReviewLog {
        sinks: vec![legacy, scoped],
    })
}

fn write_log_event(log: &Arc<Mutex<ReviewLog>>, run_id: Uuid, event: &ProgressEvent) -> Result<()> {
    let mut file = log.lock().expect("log lock");
    for sink in &mut file.sinks {
        let _ = write!(sink, "[run:{}] ", run_id);
        log_helpers::write_log_event(sink, event);
    }
    Ok(())
}

fn write_log_json(log: &Arc<Mutex<ReviewLog>>, run_id: Uuid, json: &str) {
    if let Ok(mut file) = log.lock() {
        for sink in &mut file.sinks {
            let _ = writeln!(sink, "[run:{}]", run_id);
            log_helpers::write_log_json(sink, json);
        }
    }
}

fn write_log_report_sections(log: &Arc<Mutex<ReviewLog>>, run_id: Uuid, report: &ReviewReport) {
    if let Ok(mut file) = log.lock() {
        for sink in &mut file.sinks {
            let _ = writeln!(sink, "[run:{}]", run_id);
            log_helpers::write_log_report_sections(sink, report);
        }
    }
}

fn render_report_json(report: &ReviewReport) -> String {
    log_helpers::render_report_json(report)
}

fn write_report(path: &std::path::Path, json: &str) -> Result<()> {
    std::fs::write(path, json)?;
    Ok(())
}

fn write_report_targets(
    artifacts: &libcrucible::artifacts::RunArtifacts,
    explicit: Option<&PathBuf>,
    json: &str,
) -> Result<()> {
    write_report(&artifacts.report_json, json)?;
    if let Some(path) = explicit {
        if path != &artifacts.report_json {
            write_report(path, json)?;
        }
    }
    Ok(())
}

fn push_diff_line(buf: &mut String, origin: char, content: &str) {
    match origin {
        '+' | '-' | ' ' => {
            buf.push(origin);
            buf.push_str(content);
        }
        _ => buf.push_str(content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libcrucible::config::VerdictConfig;
    use libcrucible::report::{ConsensusMap, Finding, LineSpan, ReviewReport};
    use std::path::PathBuf;

    #[test]
    fn format_duration_uses_one_decimal_second() {
        assert_eq!(log_helpers::format_duration(0.04), "0.0s");
        assert_eq!(log_helpers::format_duration(1.06), "1.1s");
        assert_eq!(log_helpers::format_duration(12.34), "12.3s");
    }

    #[test]
    fn dedup_normalization_collapses_whitespace_and_case() {
        let findings = vec![
            Finding {
                agent: "claude-code".to_string(),
                severity: Severity::Warning,
                confidence: libcrucible::report::Confidence::High,
                file: Some(PathBuf::from("src/main.rs")),
                span: Some(LineSpan { start: 10, end: 10 }),
                message: "  Missing   error handling ".to_string(),
                category: None,
                title: None,
                description: None,
                suggested_fix: None,
                evidence: Vec::new(),
                round: 1,
                raised_by: vec!["claude-code".to_string()],
            },
            Finding {
                agent: "codex".to_string(),
                severity: Severity::Warning,
                confidence: libcrucible::report::Confidence::High,
                file: Some(PathBuf::from("SRC/main.rs")),
                span: Some(LineSpan { start: 10, end: 10 }),
                message: "missing error handling".to_string(),
                category: None,
                title: None,
                description: None,
                suggested_fix: None,
                evidence: Vec::new(),
                round: 1,
                raised_by: vec!["codex".to_string()],
            },
        ];
        let report = ReviewReport::from_findings(
            uuid::Uuid::nil(),
            &findings,
            Vec::new(),
            None,
            None,
            None,
            &VerdictConfig {
                block_on: "Critical".to_string(),
            },
            ConsensusMap::default(),
            None,
            None,
            None,
            None,
        );

        let issues = build_issue_list(&report);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].raised_by.len(), 2);
    }
}
