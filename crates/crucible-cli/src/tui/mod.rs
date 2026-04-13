use crate::log_helpers;
use anyhow::{Context, Result};
use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use libcrucible::config::CrucibleConfig;
use libcrucible::progress::{ProgressEvent, ReviewerState, TranscriptDirection};
use libcrucible::report::{ReviewReport, Verdict};
use ratatui::Terminal;
use ratatui::layout::Alignment;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::io::{Write, stdout};
use std::path::PathBuf;
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
    startup_updates: Vec<String>,
    agents: Vec<String>,
    statuses: std::collections::HashMap<String, AgentStatus>,
    reviews: std::collections::HashMap<String, AgentReviewState>,
    transcript_entries: Vec<TranscriptEntry>,
}

#[derive(Debug, Clone, Default)]
struct AgentReviewState {
    summary: String,
    highlights: Vec<String>,
    details: String,
}

#[derive(Debug, Clone)]
struct TranscriptEntry {
    id: String,
    rendered: String,
    seq: u64,
}

pub async fn run_review_tui(
    cfg: &CrucibleConfig,
    interactive: bool,
    diff_override: Option<String>,
    scope_label: String,
    output_report: Option<PathBuf>,
    artifacts: libcrucible::artifacts::RunArtifacts,
) -> Result<i32> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut log = open_review_log(&artifacts)?;
    let cfg = cfg.clone();
    let run_id = artifacts.run_id;
    let mut handle = Some(tokio::spawn(async move {
        if let Some(diff) = diff_override {
            libcrucible::run_review_with_progress_diff_run_id(&cfg, tx, diff, run_id).await
        } else {
            libcrucible::run_review_with_progress_run_id(&cfg, tx, run_id).await
        }
    }));

    let mut screen = Screen::Analyzing;
    let mut report: Option<ReviewReport> = None;
    let mut diff_scroll: u16 = 0;
    let mut status_line: Option<String> = None;
    let mut progress = ProgressState::default();
    progress.run_header = Some(scope_label);
    let mut last_tick = Instant::now();
    let mut spinner_idx: usize = 0;
    let mut exit_code: Option<i32> = None;
    let mut transcript_seq: u64 = 0;

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
                        Some(scope) => format!("{scope} | {runtime}"),
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
                ProgressEvent::AnalysisSource {
                    id,
                    role,
                    plugin,
                    fallback,
                } => {
                    let mode = if *fallback { "fallback" } else { "agent" };
                    progress.phase = Some(format!(
                        "analysis source: {} role={} plugin={} ({})",
                        id, role, plugin, mode
                    ));
                }
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
                    progress.startup_updates.push(format!(
                        "{} {}{}{} - {}",
                        log_helpers::format_startup_phase(*phase),
                        log_helpers::format_startup_status(*status),
                        count_suffix,
                        duration_suffix,
                        detail
                    ));
                }
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
                ProgressEvent::AgentTranscript {
                    id,
                    direction,
                    message,
                } => {
                    let prefix = match direction {
                        TranscriptDirection::ToAgent => "->",
                        TranscriptDirection::FromAgent => "<-",
                    };
                    transcript_seq = transcript_seq.wrapping_add(1);
                    upsert_transcript_entry(
                        &mut progress.transcript_entries,
                        id,
                        format!("{prefix} {id}: {}", truncate_line(message, 150)),
                        transcript_seq,
                    );
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
                ProgressEvent::AgentError { id, message, .. } => {
                    progress.statuses.insert(id.clone(), AgentStatus::Error);
                    status_line = Some(format!("{} error: {}", id, message));
                }
                ProgressEvent::ConvergenceJudgment {
                    round,
                    verdict,
                    rationale,
                } => {
                    progress.convergence = Some(format!(
                        "Round {} convergence: {} - {}",
                        round,
                        log_helpers::format_convergence(*verdict),
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
                    if let Some(path) = &output_report {
                        write_report(path, &json)?;
                    }
                    write_report(&artifacts.report_json, &json)?;
                    write_log_json(&mut log, artifacts.run_id, &json);
                    write_log_report_sections(&mut log, artifacts.run_id, rep);
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
            write_log_event(&mut log, artifacts.run_id, &event);
        }

        if report.is_none() && exit_code.is_none() {
            if let Some(h) = &handle {
                if h.is_finished() {
                    let h = handle.take().expect("join handle exists");
                    match h.await.context("review task join")? {
                        Ok(rep) => {
                            let json = render_report_json(&rep);
                            if let Some(path) = &output_report {
                                write_report(path, &json)?;
                            }
                            write_report(&artifacts.report_json, &json)?;
                            write_log_json(&mut log, artifacts.run_id, &json);
                            write_log_report_sections(&mut log, artifacts.run_id, &rep);
                            report = Some(rep.clone());
                            screen = Screen::Review;
                            if !interactive {
                                exit_code = Some(match rep.verdict {
                                    Verdict::Block => 1,
                                    _ => 0,
                                });
                            }
                        }
                        Err(err) => {
                            status_line = Some(format!("Review failed: {err}"));
                            exit_code = Some(1);
                        }
                    }
                }
            }
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
                        write_log_event(&mut log, artifacts.run_id, &ProgressEvent::Canceled);
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
        if let Some(h) = handle.take() {
            h.abort();
        }
        return Ok(code);
    }

    let Some(handle) = handle else {
        return Ok(1);
    };
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
    if !progress.startup_updates.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Startup phases:",
            Style::default().fg(Color::Gray),
        )));
        for item in &progress.startup_updates {
            lines.push(Line::from(format!("  - {}", item)));
        }
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

        if !report.agent_failures.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from("Agent Execution Issues:"));
            for failure in &report.agent_failures {
                let round = failure
                    .round
                    .map(|value| format!(" round {}", value))
                    .unwrap_or_default();
                lines.push(Line::from(format!(
                    "- {} [{}{}] {}",
                    failure.agent, failure.stage, round, failure.message
                )));
            }
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

struct ReviewLog {
    sinks: Vec<std::fs::File>,
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

fn write_log_event(log: &mut ReviewLog, run_id: uuid::Uuid, event: &ProgressEvent) {
    for sink in &mut log.sinks {
        let _ = write!(sink, "[run:{}] ", run_id);
        log_helpers::write_log_event(sink, event);
    }
}

fn write_log_json(log: &mut ReviewLog, run_id: uuid::Uuid, json: &str) {
    for sink in &mut log.sinks {
        let _ = writeln!(sink, "[run:{}]", run_id);
        log_helpers::write_log_json(sink, json);
    }
}

fn write_log_report_sections(log: &mut ReviewLog, run_id: uuid::Uuid, report: &ReviewReport) {
    for sink in &mut log.sinks {
        let _ = writeln!(sink, "[run:{}]", run_id);
        log_helpers::write_log_report_sections(sink, report);
    }
}

fn render_report_json(report: &ReviewReport) -> String {
    log_helpers::render_report_json(report)
}

fn write_report(path: &std::path::Path, json: &str) -> Result<()> {
    std::fs::write(path, json)?;
    Ok(())
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
    if !progress.startup_updates.is_empty() {
        lines.push(Line::from("Startup phases:"));
        for item in &progress.startup_updates {
            lines.push(Line::from(format!("  - {}", item)));
        }
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
    let transcript_window = latest_transcript_lines(&progress.transcript_entries, progress.agents.len());
    if !transcript_window.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from("Conversation:"));
        for item in &transcript_window {
            lines.push(Line::from(format!("  {}", item)));
        }
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
    log_helpers::format_parallel_status(statuses)
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

fn upsert_transcript_entry(entries: &mut Vec<TranscriptEntry>, id: &str, rendered: String, seq: u64) {
    if let Some(existing) = entries.iter_mut().find(|entry| entry.id == id) {
        existing.rendered = rendered;
        existing.seq = seq;
        return;
    }
    entries.push(TranscriptEntry {
        id: id.to_string(),
        rendered,
        seq,
    });
}

fn latest_transcript_lines(entries: &[TranscriptEntry], limit: usize) -> Vec<String> {
    let mut entries = entries.to_vec();
    entries.sort_by(|a, b| b.seq.cmp(&a.seq));
    entries.truncate(limit);
    entries.sort_by(|a, b| a.seq.cmp(&b.seq));
    entries.into_iter().map(|entry| entry.rendered).collect()
}
