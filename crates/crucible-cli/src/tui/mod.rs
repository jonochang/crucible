use anyhow::{Context, Result};
use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use libcrucible::config::CrucibleConfig;
use libcrucible::progress::{ConvergenceVerdict, ProgressEvent, ReviewerState};
use libcrucible::report::{ReviewReport, Verdict};
use ratatui::Terminal;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::io::{Write, stdout};
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Analyzing,
    Reviewing,
    Review,
    DiffView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentStatus {
    Queued,
    Running,
    Done,
    Error,
}

#[derive(Debug, Default, Clone)]
struct ProgressState {
    phase: Option<String>,
    analyzer_done: bool,
    round: Option<u8>,
    total_rounds: u8,
    run_header: Option<String>,
    analysis: Option<String>,
    system_context: Option<String>,
    convergence: Option<String>,
    parallel_status: Option<String>,
    agents: Vec<String>,
    statuses: std::collections::HashMap<String, AgentStatus>,
    reviews: std::collections::HashMap<String, AgentReviewState>,
}

#[derive(Debug, Clone, Default)]
struct AgentReviewState {
    summary: String,
    highlights: Vec<String>,
    details: String,
}

pub async fn run_review_tui(
    cfg: &CrucibleConfig,
    interactive: bool,
    diff_override: Option<String>,
    scope_label: String,
) -> Result<i32> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut log = open_review_log()?;
    let cfg = cfg.clone();
    let handle = tokio::spawn(async move {
        if let Some(diff) = diff_override {
            libcrucible::run_review_with_progress_diff(&cfg, tx, diff).await
        } else {
            libcrucible::run_review_with_progress(&cfg, tx).await
        }
    });

    let mut screen = Screen::Analyzing;
    let mut report: Option<ReviewReport> = None;
    let mut diff_scroll: u16 = 0;
    let mut status_line: Option<String> = None;
    let mut progress = ProgressState::default();
    progress.run_header = Some(scope_label);
    let mut last_tick = Instant::now();
    let mut spinner_idx: usize = 0;
    let mut exit_code: Option<i32> = None;

    loop {
        while let Ok(event) = rx.try_recv() {
            match &event {
                ProgressEvent::RunHeader {
                    reviewers,
                    max_rounds,
                    changed_lines,
                    ..
                } => {
                    let runtime = format!(
                        "Reviewers: {} | Max rounds: {} | Changed lines: {}",
                        reviewers.join(", "),
                        max_rounds,
                        changed_lines
                    );
                    progress.run_header = Some(match progress.run_header.take() {
                        Some(scope) => format!("{scope}\n{runtime}"),
                        None => runtime,
                    });
                }
                ProgressEvent::PhaseStart { phase } => {
                    progress.phase = Some(format!("{} (running)", phase))
                }
                ProgressEvent::PhaseDone { phase } => {
                    progress.phase = Some(format!("{} (done)", phase))
                }
                ProgressEvent::AnalyzerStart => screen = Screen::Analyzing,
                ProgressEvent::AnalyzerDone => progress.analyzer_done = true,
                ProgressEvent::AnalysisReady { markdown } => {
                    progress.analysis = Some(markdown.clone())
                }
                ProgressEvent::SystemContextReady { markdown } => {
                    progress.system_context = Some(markdown.clone())
                }
                ProgressEvent::RoundStart {
                    round,
                    total_rounds,
                    agents,
                } => {
                    screen = Screen::Reviewing;
                    progress.round = Some(*round);
                    progress.total_rounds = *total_rounds;
                    progress.agents = agents.clone();
                    progress.statuses = agents
                        .iter()
                        .cloned()
                        .map(|id| (id, AgentStatus::Queued))
                        .collect();
                }
                ProgressEvent::ParallelStatus { statuses, .. } => {
                    progress.parallel_status = Some(format_parallel_status(statuses));
                    for status in statuses {
                        let mapped = match status.state {
                            ReviewerState::Queued => AgentStatus::Queued,
                            ReviewerState::Running => AgentStatus::Running,
                            ReviewerState::Done => AgentStatus::Done,
                            ReviewerState::Error => AgentStatus::Error,
                        };
                        progress.statuses.insert(status.id.clone(), mapped);
                    }
                }
                ProgressEvent::AgentStart { id, .. } => {
                    progress.statuses.insert(id.clone(), AgentStatus::Running);
                }
                ProgressEvent::AgentReview {
                    id,
                    summary,
                    highlights,
                    details,
                    ..
                } => {
                    progress.reviews.insert(
                        id.clone(),
                        AgentReviewState {
                            summary: summary.clone(),
                            highlights: highlights
                                .iter()
                                .map(|h| format!("[{}] {} {}", h.severity, h.location, h.message))
                                .collect(),
                            details: details.clone(),
                        },
                    );
                }
                ProgressEvent::AgentDone { id, .. } => {
                    progress.statuses.insert(id.clone(), AgentStatus::Done);
                }
                ProgressEvent::AgentError { id, .. } => {
                    progress.statuses.insert(id.clone(), AgentStatus::Error);
                }
                ProgressEvent::ConvergenceJudgment {
                    round,
                    verdict,
                    rationale,
                } => {
                    progress.convergence = Some(format!(
                        "Round {} convergence: {} - {}",
                        round,
                        format_convergence(*verdict),
                        rationale
                    ));
                }
                ProgressEvent::RoundComplete {
                    round,
                    total_rounds,
                } => {
                    status_line = Some(format!("Round {}/{} complete", round, total_rounds));
                }
                ProgressEvent::Completed(rep) => {
                    let json = render_report_json(rep);
                    write_log_json(&mut log, &json);
                    write_log_report_sections(&mut log, rep);
                    report = Some(rep.clone());
                    screen = Screen::Review;
                    if !interactive {
                        exit_code = Some(match rep.verdict {
                            Verdict::Block => 1,
                            _ => 0,
                        });
                    }
                }
                ProgressEvent::Canceled => {}
                _ => {}
            }
            write_log_event(&mut log, &event);
        }

        terminal.draw(|f| {
            let size = f.area();
            let block = Block::default().borders(Borders::ALL).title("Crucible");
            let inner = block.inner(size);
            f.render_widget(block, size);

            let content = match screen {
                Screen::Analyzing => render_analyzing(&progress, spinner_frame(spinner_idx)),
                Screen::Reviewing => render_reviewing(&progress, spinner_frame(spinner_idx)),
                Screen::Review => render_review(report.as_ref(), status_line.as_deref()),
                Screen::DiffView => render_diff(report.as_ref(), diff_scroll),
            };
            f.render_widget(content, inner);
        })?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match screen {
                        Screen::Review => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') => break,
                            KeyCode::Char('d') | KeyCode::Char('D') => screen = Screen::DiffView,
                            KeyCode::Enter => {
                                if let Some(rep) = &report {
                                    if let Some(fix) = &rep.auto_fix {
                                        match apply_patch(&fix.unified_diff) {
                                            Ok(_) => break,
                                            Err(err) => {
                                                status_line = Some(format!("Patch failed: {err}"))
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        },
                        Screen::DiffView => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                                screen = Screen::Review
                            }
                            KeyCode::Down => diff_scroll = diff_scroll.saturating_add(1),
                            KeyCode::Up => diff_scroll = diff_scroll.saturating_sub(1),
                            _ => {}
                        },
                        _ => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => break,
                            _ => {}
                        },
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
                    {
                        write_log_event(&mut log, &ProgressEvent::Canceled);
                        exit_code = Some(130);
                        break;
                    }
                }
            }
        }

        if exit_code.is_some() {
            break;
        }

        if last_tick.elapsed() > Duration::from_millis(250) {
            last_tick = Instant::now();
            spinner_idx = spinner_idx.wrapping_add(1);
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    if let Some(code) = exit_code {
        handle.abort();
        return Ok(code);
    }

    let report = match handle.await.context("review task join")? {
        Ok(rep) => rep,
        Err(err) => {
            eprintln!("Review failed: {err}");
            return Ok(1);
        }
    };

    let code = match report.verdict {
        Verdict::Block => 1,
        _ => 0,
    };
    Ok(code)
}

fn render_analyzing<'a>(progress: &'a ProgressState, spinner: &'static str) -> Paragraph<'a> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!("{spinner} "), Style::default().fg(Color::Cyan)),
        Span::styled("Analyzing changes...", Style::default().fg(Color::Yellow)),
    ]));
    if let Some(header) = &progress.run_header {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            header.clone(),
            Style::default().fg(Color::Gray),
        )));
    }
    if let Some(phase) = &progress.phase {
        lines.push(Line::from(Span::styled(
            format!("Phase: {phase}"),
            Style::default().fg(Color::Green),
        )));
    }
    Paragraph::new(Text::from(lines))
        .alignment(Alignment::Left)
        .wrap(Wrap { trim: true })
}

fn render_review<'a>(report: Option<&'a ReviewReport>, status: Option<&'a str>) -> Paragraph<'a> {
    let mut lines = Vec::new();
    if let Some(report) = report {
        for f in &report.findings {
            let sev = match f.severity {
                libcrucible::report::Severity::Critical => "CRITICAL",
                libcrucible::report::Severity::Warning => "WARNING",
                libcrucible::report::Severity::Info => "INFO",
            };
            let loc = match (&f.file, &f.span) {
                (Some(file), Some(span)) => format!("{}:{}", file.display(), span.start),
                (Some(file), None) => file.display().to_string(),
                _ => "<unknown>".to_string(),
            };
            lines.push(Line::from(vec![Span::styled(
                format!("[{}] {:20} {}", sev, loc, f.message),
                Style::default().fg(Color::White),
            )]));
        }

        if report.auto_fix.is_some() {
            lines.push(Line::from(""));
            lines.push(Line::from("Auto-fix ready."));
            lines.push(Line::from(
                "[Enter] Apply patch    [D] View diff    [Q] Skip",
            ));
        } else {
            lines.push(Line::from(""));
            lines.push(Line::from("[Q] Quit"));
        }
        if let Some(final_analysis) = &report.final_analysis_markdown {
            lines.push(Line::from(""));
            lines.push(Line::from("Final Analysis:"));
            for line in final_analysis.lines() {
                lines.push(Line::from(line.to_string()));
            }
        }
    } else {
        lines.push(Line::from("No report yet"));
    }

    if let Some(status) = status {
        lines.push(Line::from(""));
        lines.push(Line::from(status.to_string()));
    }

    Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false })
}

fn open_review_log() -> Result<std::fs::File> {
    let path = std::env::current_dir()?.join("review_report.log");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    Ok(file)
}

fn write_log_event(log: &mut std::fs::File, event: &ProgressEvent) {
    let _ = write!(log, "[{}] ", log_timestamp());
    match event {
        ProgressEvent::RunHeader {
            reviewers,
            max_rounds,
            changed_files,
            changed_lines,
            convergence_enabled,
            context_enabled,
        } => {
            let _ = writeln!(
                log,
                "[progress] run:header reviewers={} max_rounds={} changed_files={} changed_lines={} convergence_enabled={} context_enabled={}",
                reviewers.join(","),
                max_rounds,
                changed_files,
                changed_lines,
                convergence_enabled,
                context_enabled
            );
        }
        ProgressEvent::PhaseStart { phase } => {
            let _ = writeln!(log, "[progress] phase:start {}", phase);
        }
        ProgressEvent::PhaseDone { phase } => {
            let _ = writeln!(log, "[progress] phase:done {}", phase);
        }
        ProgressEvent::AnalyzerStart => {
            let _ = writeln!(log, "[progress] analyzer:start");
        }
        ProgressEvent::AnalyzerDone => {
            let _ = writeln!(log, "[progress] analyzer:done");
        }
        ProgressEvent::AnalysisReady { markdown } => {
            let _ = writeln!(log, "[analysis]");
            let _ = writeln!(log, "{}", markdown);
        }
        ProgressEvent::SystemContextReady { markdown } => {
            let _ = writeln!(log, "[system-context]");
            let _ = writeln!(log, "{}", markdown);
        }
        ProgressEvent::RoundStart { round, agents, .. } => {
            let _ = writeln!(
                log,
                "[progress] round:{} start (agents: {})",
                round,
                agents.join(",")
            );
        }
        ProgressEvent::ParallelStatus { round, statuses } => {
            let _ = writeln!(
                log,
                "[progress] round:{} status {}",
                round,
                format_parallel_status(statuses)
            );
        }
        ProgressEvent::AgentStart { round, id } => {
            let _ = writeln!(log, "[progress] agent:start round={} id={}", round, id);
        }
        ProgressEvent::AgentReview {
            round,
            id,
            summary,
            highlights,
            details,
        } => {
            let _ = writeln!(log, "[agent-review] round={} id={} {}", round, id, summary);
            let _ = writeln!(log, "[agent-review] details:\n{}", details);
            for h in highlights {
                let _ = writeln!(
                    log,
                    "[agent-review]   [{}] {} {}",
                    h.severity, h.location, h.message
                );
            }
        }
        ProgressEvent::AgentDone { round, id } => {
            let _ = writeln!(log, "[progress] agent:done round={} id={}", round, id);
        }
        ProgressEvent::AgentError { round, id, message } => {
            let _ = writeln!(
                log,
                "[progress] agent:error round={} id={} msg={}",
                round, id, message
            );
        }
        ProgressEvent::RoundDone { round } => {
            let _ = writeln!(log, "[progress] round:{} done", round);
        }
        ProgressEvent::ConvergenceJudgment {
            round,
            verdict,
            rationale,
        } => {
            let _ = writeln!(
                log,
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
            let _ = writeln!(log, "[progress] round:{}/{} complete", round, total_rounds);
        }
        ProgressEvent::AutoFixReady => {
            let _ = writeln!(log, "[progress] autofix:ready");
        }
        ProgressEvent::Completed(_) => {}
        ProgressEvent::Canceled => {
            let _ = writeln!(log, "[progress] canceled");
        }
    }
    let _ = log.flush();
}

fn write_log_json(log: &mut std::fs::File, json: &str) {
    let _ = writeln!(log, "[{}] [report]", log_timestamp());
    let _ = writeln!(log, "{}", json);
    let _ = log.flush();
}

fn write_log_report_sections(log: &mut std::fs::File, report: &ReviewReport) {
    if let Some(final_analysis) = &report.final_analysis_markdown {
        let _ = writeln!(log, "[{}] [final-analysis]", log_timestamp());
        let _ = writeln!(log, "{}", final_analysis);
    }
    if let Some(comment) = &report.pr_comment_markdown {
        let _ = writeln!(log, "[{}] [pr-comment]", log_timestamp());
        let _ = writeln!(log, "{}", comment);
    }
    let _ = log.flush();
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
                "analysis_markdown": report.analysis_markdown,
                "system_context_markdown": report.system_context_markdown,
                "final_analysis_markdown": report.final_analysis_markdown,
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

fn log_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}.{}", d.as_secs(), d.subsec_millis()),
        Err(_) => "0.000".to_string(),
    }
}

fn render_reviewing<'a>(progress: &'a ProgressState, spinner: &'static str) -> Paragraph<'a> {
    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(format!("{spinner} "), Style::default().fg(Color::Cyan)),
        Span::styled("Review in progress", Style::default().fg(Color::Yellow)),
    ]));
    if let Some(run_header) = &progress.run_header {
        lines.push(Line::from(run_header.clone()));
    }
    if let Some(phase) = &progress.phase {
        lines.push(Line::from(format!("Phase: {}", phase)));
    }
    let round = progress.round.unwrap_or(1);
    let total = progress.total_rounds.max(1);
    let analyzer = if progress.analyzer_done {
        "done"
    } else {
        "running"
    };
    lines.push(Line::from(format!(
        "Round {}/{}  (Analyzer: {})",
        round, total, analyzer
    )));
    if let Some(status) = &progress.parallel_status {
        lines.push(Line::from(status.clone()));
    }
    if let Some(analysis) = &progress.analysis {
        lines.push(Line::from(""));
        lines.push(Line::from("Analysis:"));
        lines.push(Line::from(truncate_line(analysis, 180)));
    }
    if let Some(system_context) = &progress.system_context {
        lines.push(Line::from(""));
        lines.push(Line::from("System Context:"));
        lines.push(Line::from(truncate_line(system_context, 180)));
    }
    if let Some(convergence) = &progress.convergence {
        lines.push(Line::from(""));
        lines.push(Line::from(convergence.clone()));
    }
    for id in &progress.agents {
        let status = match progress
            .statuses
            .get(id)
            .copied()
            .unwrap_or(AgentStatus::Queued)
        {
            AgentStatus::Queued => "queued",
            AgentStatus::Running => "running",
            AgentStatus::Done => "done",
            AgentStatus::Error => "error",
        };
        lines.push(Line::from(format!("{:<12} [{}]", id, status)));
        if let Some(review) = progress.reviews.get(id) {
            lines.push(Line::from(format!("  -> {}", review.summary)));
            for h in &review.highlights {
                lines.push(Line::from(format!("     {}", h)));
            }
            for detail in review.details.lines() {
                lines.push(Line::from(format!("     {}", detail)));
            }
        }
    }
    Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false })
}

fn spinner_frame(idx: usize) -> &'static str {
    const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[idx % FRAMES.len()]
}

fn render_diff(report: Option<&ReviewReport>, scroll: u16) -> Paragraph<'_> {
    let diff = report
        .and_then(|r| r.auto_fix.as_ref())
        .map(|a| a.unified_diff.as_str())
        .unwrap_or("No diff available");
    Paragraph::new(Text::from(diff.to_string()))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Auto-fix Diff"),
        )
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false })
}

fn apply_patch(diff: &str) -> Result<()> {
    let mut file = NamedTempFile::new()?;
    std::io::Write::write_all(&mut file, diff.as_bytes())?;
    let status = std::process::Command::new("git")
        .arg("apply")
        .arg(file.path())
        .status()
        .context("git apply")?;
    if !status.success() {
        return Err(anyhow::anyhow!("git apply failed"));
    }
    Ok(())
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
            Some(secs) => format!("{marker} {} ({secs:.1}s)", status.id),
            None => format!("{marker} {}", status.id),
        };
        parts.push(part);
    }
    format!("[{}]", parts.join(" | "))
}

fn format_convergence(verdict: ConvergenceVerdict) -> &'static str {
    match verdict {
        ConvergenceVerdict::Converged => "CONVERGED",
        ConvergenceVerdict::NotConverged => "NOT_CONVERGED",
    }
}

fn truncate_line(input: &str, max: usize) -> String {
    let one_line = input.replace('\n', " ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut out = String::new();
    for (idx, ch) in one_line.chars().enumerate() {
        if idx >= max {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}
