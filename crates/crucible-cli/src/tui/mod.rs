use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use libcrucible::config::CrucibleConfig;
use libcrucible::progress::ProgressEvent;
use libcrucible::report::{ReviewReport, Verdict};
use ratatui::layout::Alignment;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Terminal;
use std::io::stdout;
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

pub async fn run_review_tui(cfg: &CrucibleConfig) -> Result<i32> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let cfg = cfg.clone();
    let handle = tokio::spawn(async move { libcrucible::run_review_with_progress(&cfg, tx).await });

    let mut screen = Screen::Analyzing;
    let mut report: Option<ReviewReport> = None;
    let mut diff_scroll: u16 = 0;
    let mut status_line: Option<String> = None;
    let mut last_tick = Instant::now();

    loop {
        while let Ok(event) = rx.try_recv() {
            match event {
                ProgressEvent::AnalyzerStart => screen = Screen::Analyzing,
                ProgressEvent::ReviewStart => screen = Screen::Reviewing,
                ProgressEvent::Completed(rep) => {
                    report = Some(rep);
                    screen = Screen::Review;
                }
                _ => {}
            }
        }

        terminal.draw(|f| {
            let size = f.area();
            let block = Block::default().borders(Borders::ALL).title("Crucible");
            let inner = block.inner(size);
            f.render_widget(block, size);

            let content = match screen {
                Screen::Analyzing => render_status("Analyzing diff…"),
                Screen::Reviewing => render_status("Reviewing diff…"),
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
                                            Err(err) => status_line = Some(format!("Patch failed: {err}")),
                                        }
                                    }
                                }
                            }
                            _ => {}
                        },
                        Screen::DiffView => match key.code {
                            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => screen = Screen::Review,
                            KeyCode::Down => diff_scroll = diff_scroll.saturating_add(1),
                            KeyCode::Up => diff_scroll = diff_scroll.saturating_sub(1),
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
        }

        if last_tick.elapsed() > Duration::from_millis(250) {
            last_tick = Instant::now();
        }

        if let Some(rep) = &report {
            if matches!(rep.verdict, Verdict::Block) && screen == Screen::Review {
                // keep showing until user quits
            }
        }
    }

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    let report = match handle.await.context("review task join")? {
        Ok(rep) => rep,
        Err(err) => {
            eprintln!("Review failed: {err}");
            return Ok(1);
        }
    };

    let exit_code = match report.verdict {
        Verdict::Block => 1,
        _ => 0,
    };
    Ok(exit_code)
}

fn render_status(message: &str) -> Paragraph<'_> {
    Paragraph::new(Text::from(Line::from(vec![Span::raw(message)])))
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
            lines.push(Line::from("[Enter] Apply patch    [D] View diff    [Q] Skip"));
        } else {
            lines.push(Line::from(""));
            lines.push(Line::from("[Q] Quit"));
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

fn render_diff(report: Option<&ReviewReport>, scroll: u16) -> Paragraph<'_> {
    let diff = report
        .and_then(|r| r.auto_fix.as_ref())
        .map(|a| a.unified_diff.as_str())
        .unwrap_or("No diff available");
    Paragraph::new(Text::from(diff.to_string()))
        .block(Block::default().borders(Borders::ALL).title("Auto-fix Diff"))
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
