use anyhow::Result;
use clap::Args;
use libcrucible::config::CrucibleConfig;
use libcrucible::progress::{ConvergenceVerdict, ProgressEvent, ReviewerState};
use libcrucible::report::{ReviewReport, Severity, Verdict};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Args)]
pub struct ReviewArgs {
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
    #[arg(long, help = "Keep TUI open after completion; default is auto-exit")]
    pub interactive: bool,
    #[arg(long, help = "Run review with a single reviewer id (e.g. claude-code)")]
    pub reviewer: Option<String>,
    #[arg(long, help = "Override maximum review rounds")]
    pub max_rounds: Option<u8>,
}

pub async fn run(args: ReviewArgs) -> Result<()> {
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
    let use_tui = !args.hook && args.export_issues.is_none() && std::io::stdout().is_terminal();
    if use_tui {
        let exit_code = crate::tui::run_review_tui(&cfg, args.interactive).await?;
        std::process::exit(exit_code);
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let log = open_review_log()?;
    let log = Arc::new(Mutex::new(log));
    let cfg_for_review = cfg.clone();
    let mut review_handle =
        tokio::spawn(
            async move { libcrucible::run_review_with_progress(&cfg_for_review, tx).await },
        );
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
            issue.message,
            issue.raised_by.join(", ")
        );
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
    file: Option<String>,
    line_start: Option<u32>,
    line_end: Option<u32>,
    location: String,
    message: String,
    raised_by: Vec<String>,
}

fn build_issue_list(report: &ReviewReport) -> Vec<IssueRow> {
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
                    file: group.file.clone(),
                    line_start: group.line_start,
                    line_end: group.line_end,
                    location,
                    message: group.message,
                    raised_by: group.raised_by.into_iter().collect(),
                }
            },
        )
        .collect::<Vec<_>>();

    issues.sort_by(|a, b| {
        let sa = severity_rank(&a.severity);
        let sb = severity_rank(&b.severity);
        sb.cmp(&sa)
            .then(a.location.cmp(&b.location))
            .then(a.message.cmp(&b.message))
    });
    issues
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
                i.message
            ));
            out.push_str(&format!("   - raised_by: {}\n", i.raised_by.join(", ")));
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
                    "file": i.file,
                    "line_start": i.line_start,
                    "line_end": i.line_end,
                    "location": i.location,
                    "message": i.message,
                    "raised_by": i.raised_by
                })
            })
            .collect::<Vec<_>>(),
    )?;
    std::fs::write(path, json)?;
    Ok(())
}

fn emit_progress(event: &ProgressEvent) {
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
        } => {
            eprintln!("[agent-review] round={} id={} {}", round, id, summary);
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
        } => {
            writeln!(file, "[agent-review] round={} id={} {}", round, id, summary)?;
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
                "consensus": consensus,
                "auto_fix": report.auto_fix,
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
                file: Some(PathBuf::from("src/main.rs")),
                span: Some(LineSpan { start: 10, end: 10 }),
                message: "  Missing   error handling ".to_string(),
                round: 1,
                raised_by: vec!["claude-code".to_string()],
            },
            Finding {
                agent: "codex".to_string(),
                severity: Severity::Warning,
                file: Some(PathBuf::from("SRC/main.rs")),
                span: Some(LineSpan { start: 10, end: 10 }),
                message: "missing error handling".to_string(),
                round: 1,
                raised_by: vec!["codex".to_string()],
            },
        ];
        let report = ReviewReport::from_findings(
            &findings,
            &VerdictConfig {
                block_on: "Critical".to_string(),
            },
            ConsensusMap::default(),
            None,
        );

        let issues = build_issue_list(&report);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].raised_by.len(), 2);
    }
}
