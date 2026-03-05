use anyhow::Result;
use clap::Args;
use libcrucible::config::CrucibleConfig;
use libcrucible::progress::{ConvergenceVerdict, ProgressEvent, ReviewerState};
use libcrucible::report::{CanonicalIssue, ReviewReport, Severity, Verdict};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::OnceLock;
use tokio::sync::mpsc;

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
    #[arg(long, default_value = "origin", help = "Git remote to use for --repo base and PR checkout")]
    pub git_remote: String,
}

pub async fn run(args: ReviewArgs) -> Result<()> {
    if args.debug {
        let path = std::env::current_dir()?.join("crucible.log");
        std::fs::write(&path, b"")?;
        libcrucible::plugins::set_debug_log(&path)?;
        eprintln!("Debug logging enabled: {}", path.display());
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
    } else if args.repo {
        ReviewTarget::Repo
    } else {
        ReviewTarget::Local
    };
    let diff_override = resolve_review_diff(&target, &args.git_remote)?;
    if matches!(target, ReviewTarget::Local) {
        let diff = diff_override.as_deref().unwrap_or_default();
        if diff.trim().is_empty() || count_changed_lines(diff) == 0 {
            println!("No local code changes detected; skipping review.");
            return Ok(());
        }
    }

    let use_tui = matches!(target, ReviewTarget::Local)
        && !args.hook
        && args.export_issues.is_none()
        && std::io::stdout().is_terminal();
    if use_tui {
        let exit_code = crate::tui::run_review_tui(&cfg, args.interactive).await?;
        std::process::exit(exit_code);
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let log = open_review_log()?;
    let log = Arc::new(Mutex::new(log));
    let cfg_for_review = cfg.clone();
    let mut review_handle = tokio::spawn(async move {
        if let Some(diff) = diff_override {
            libcrucible::run_review_with_progress_diff(&cfg_for_review, tx, diff).await
        } else {
            libcrucible::run_review_with_progress(&cfg_for_review, tx).await
        }
    });
    let log_for_progress = log.clone();
    let progress_handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            emit_progress(&event);
            let _ = write_log_event(&log_for_progress, &event);
        }
    });

    let report = tokio::select! {
        res = &mut review_handle => {
            let report = res??;
            let _ = progress_handle.await;
            report
        }
        _ = tokio::signal::ctrl_c() => {
            emit_progress(&ProgressEvent::Canceled);
            let _ = write_log_event(&log, &ProgressEvent::Canceled);
            review_handle.abort();
            std::process::exit(130);
        }
    };

    if args.json {
        let json = render_report_json(&report);
        println!("{json}");
        write_log_json(&log, &json);
        if let Some(path) = &args.export_issues {
            export_issues(path, &build_issue_list(&report))?;
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
    write_log_json(&log, &json);

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

/// Resolve the concrete diff content for the selected target mode.
/// Returns `Ok(None)` only for empty local diffs so callers can skip gracefully.
fn resolve_review_diff(target: &ReviewTarget, git_remote: &str) -> Result<Option<String>> {
    match target {
        ReviewTarget::Local => {
            let diff = run_git_capture(&["diff", "HEAD"])?;
            if diff.trim().is_empty() {
                return Ok(None);
            }
            Ok(Some(diff))
        }
        ReviewTarget::Repo => {
            let base_branch = resolve_remote_default_branch(git_remote)?;
            let range = format!("{git_remote}/{base_branch}...HEAD");
            let diff = run_git_capture(&["diff", range.as_str()])?;
            if diff.trim().is_empty() {
                anyhow::bail!("no repo diff found for range {}", range);
            }
            Ok(Some(diff))
        }
        ReviewTarget::Branch(base) => {
            let range = format!("{base}...HEAD");
            let diff = run_git_capture(&["diff", range.as_str()])?;
            if diff.trim().is_empty() {
                anyhow::bail!("no branch diff found for range {}", range);
            }
            Ok(Some(diff))
        }
        ReviewTarget::Files(files) => {
            let mut args = vec!["diff", "HEAD", "--"];
            let owned = files.iter().map(String::as_str).collect::<Vec<_>>();
            args.extend(owned);
            let diff = run_git_capture(&args)?;
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

fn resolve_remote_default_branch(git_remote: &str) -> Result<String> {
    let ref_name = format!("refs/remotes/{git_remote}/HEAD");
    let symref = run_git_capture(&["symbolic-ref", ref_name.as_str()])?;
    let prefix = format!("refs/remotes/{git_remote}/");
    let branch = symref.trim().strip_prefix(&prefix).unwrap_or("main");
    Ok(branch.to_string())
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

/// Run a git command and capture stdout as UTF-8.
fn run_git_capture(args: &[&str]) -> Result<String> {
    run_cmd_capture("git", args)
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

fn emit_progress(event: &ProgressEvent) {
    render_spinner_status(event);
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
                format_parallel_status(statuses)
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
                format_convergence(*verdict),
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
        ProgressEvent::RoundStart { round, .. } => state.round = Some(*round),
        ProgressEvent::ParallelStatus { statuses, .. } => {
            state.statuses = format_parallel_status(statuses);
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

    let round = state.round.map(|r| format!("round {r}")).unwrap_or_else(|| "startup".to_string());
    let phase = if state.phase.is_empty() { "initializing".to_string() } else { state.phase.clone() };
    let status = if state.statuses.is_empty() { String::new() } else { format!(" | {}", state.statuses) };
    let line = format!(
        "\r\x1b[36m{frame}\x1b[0m \x1b[33m{phase}\x1b[0m ({round}){status}"
    );
    eprint!("{line}");
    let _ = std::io::stderr().flush();
}

fn open_review_log() -> Result<std::fs::File> {
    let path = std::env::current_dir()?.join("review_report.log");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(file)
}

fn write_log_event(log: &Arc<Mutex<std::fs::File>>, event: &ProgressEvent) -> Result<()> {
    let mut file = log.lock().expect("log lock");
    match event {
        ProgressEvent::RunHeader {
            reviewers,
            max_rounds,
            changed_files,
            changed_lines,
            convergence_enabled,
            context_enabled,
        } => writeln!(
            file,
            "[progress] run:header reviewers={} max_rounds={} changed_files={} changed_lines={} convergence_enabled={} context_enabled={}",
            reviewers.join(","),
            max_rounds,
            changed_files,
            changed_lines,
            convergence_enabled,
            context_enabled
        )?,
        ProgressEvent::PhaseStart { phase } => writeln!(file, "[progress] phase:start {}", phase)?,
        ProgressEvent::PhaseDone { phase } => writeln!(file, "[progress] phase:done {}", phase)?,
        ProgressEvent::AnalyzerStart => writeln!(file, "[progress] analyzer:start")?,
        ProgressEvent::AnalyzerDone => writeln!(file, "[progress] analyzer:done")?,
        ProgressEvent::AnalysisReady { markdown } => {
            writeln!(file, "[analysis]")?;
            writeln!(file, "{}", markdown)?;
        }
        ProgressEvent::SystemContextReady { markdown } => {
            writeln!(file, "[system-context]")?;
            writeln!(file, "{}", markdown)?;
        }
        ProgressEvent::RoundStart { round, agents, .. } => writeln!(
            file,
            "[progress] round:{} start (agents: {})",
            round,
            agents.join(",")
        )?,
        ProgressEvent::ParallelStatus { round, statuses } => writeln!(
            file,
            "[progress] round:{} status {}",
            round,
            format_parallel_status(statuses)
        )?,
        ProgressEvent::AgentStart { round, id } => {
            writeln!(file, "[progress] agent:start round={} id={}", round, id)?
        }
        ProgressEvent::AgentReview {
            round,
            id,
            summary,
            highlights,
            details,
        } => {
            writeln!(file, "[agent-review] round={} id={} {}", round, id, summary)?;
            writeln!(file, "[agent-review] details:\n{}", details)?;
            for h in highlights {
                writeln!(
                    file,
                    "[agent-review]   [{}] {} {}",
                    h.severity, h.location, h.message
                )?;
            }
        }
        ProgressEvent::AgentDone { round, id } => {
            writeln!(file, "[progress] agent:done round={} id={}", round, id)?
        }
        ProgressEvent::AgentError { round, id, message } => writeln!(
            file,
            "[progress] agent:error round={} id={} msg={}",
            round, id, message
        )?,
        ProgressEvent::RoundDone { round } => writeln!(file, "[progress] round:{} done", round)?,
        ProgressEvent::ConvergenceJudgment {
            round,
            verdict,
            rationale,
        } => writeln!(
            file,
            "[progress] convergence round={} verdict={} rationale={}",
            round,
            format_convergence(*verdict),
            rationale
        )?,
        ProgressEvent::RoundComplete {
            round,
            total_rounds,
        } => writeln!(file, "[progress] round:{}/{} complete", round, total_rounds)?,
        ProgressEvent::AutoFixReady => writeln!(file, "[progress] autofix:ready")?,
        ProgressEvent::Completed(_) => {}
        ProgressEvent::Canceled => writeln!(file, "[progress] canceled")?,
    }
    let _ = file.flush();
    Ok(())
}

fn write_log_json(log: &Arc<Mutex<std::fs::File>>, json: &str) {
    if let Ok(mut file) = log.lock() {
        let _ = writeln!(file, "[report]");
        let _ = writeln!(file, "{}", json);
        let _ = file.flush();
    }
}

fn render_report_json(report: &ReviewReport) -> String {
    match serde_json::to_string_pretty(report) {
        Ok(s) => s,
        Err(_) => {
            let consensus = report
                .consensus_map
                .0
                .iter()
                .map(|(key, status)| {
                    serde_json::json!({
                        "file": key.file,
                        "span": key.span,
                        "agreed_count": status.agreed_count,
                        "total_agents": status.total_agents,
                        "severity": status.severity,
                        "reached_quorum": status.reached_quorum
                    })
                })
                .collect::<Vec<_>>();
            serde_json::to_string_pretty(&serde_json::json!({
                "verdict": report.verdict,
                "findings": report.findings,
                "issues": report.issues,
                "consensus": consensus,
                "auto_fix": report.auto_fix,
                "final_action_plan": report.final_action_plan,
                "pr_comment_markdown": report.pr_comment_markdown,
                "session_id": report.session_id
            }))
            .unwrap_or_else(|_| "{}".to_string())
        }
    }
}

fn format_parallel_status(statuses: &[libcrucible::progress::ReviewerStatus]) -> String {
    let mut parts = Vec::with_capacity(statuses.len());
    for status in statuses {
        let marker = match status.state {
            ReviewerState::Queued => "...",
            ReviewerState::Running => "RUN",
            ReviewerState::Done => "OK",
            ReviewerState::Error => "ERR",
        };
        let part = match status.duration_secs {
            Some(secs) => format!("{marker} {} ({})", status.id, format_duration(secs)),
            None => format!("{marker} {}", status.id),
        };
        parts.push(part);
    }
    format!("[{}]", parts.join(" | "))
}

fn format_duration(seconds: f32) -> String {
    format!("{seconds:.1}s")
}

fn format_convergence(verdict: ConvergenceVerdict) -> &'static str {
    match verdict {
        ConvergenceVerdict::Converged => "CONVERGED",
        ConvergenceVerdict::NotConverged => "NOT_CONVERGED",
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
        assert_eq!(format_duration(0.04), "0.0s");
        assert_eq!(format_duration(1.06), "1.1s");
        assert_eq!(format_duration(12.34), "12.3s");
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
        );

        let issues = build_issue_list(&report);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].raised_by.len(), 2);
    }
}
