use anyhow::Result;
use clap::Args;
use libcrucible::config::CrucibleConfig;
use libcrucible::progress::ProgressEvent;
use libcrucible::report::{ReviewReport, Verdict};
use std::io::{IsTerminal, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Args)]
pub struct ReviewArgs {
    #[arg(long)]
    pub hook: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long, help = "Enable verbose CLI agent logging")]
    pub verbose: bool,
}

pub async fn run(args: ReviewArgs) -> Result<()> {
    if args.verbose {
        libcrucible::plugins::set_verbose(true);
    }
    let cfg = CrucibleConfig::load()?;
    let use_tui = !args.hook && std::io::stdout().is_terminal();
    if use_tui {
        let exit_code = crate::tui::run_review_tui(&cfg).await?;
        std::process::exit(exit_code);
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let log = open_review_log()?;
    let log = Arc::new(Mutex::new(log));
    let cfg_for_review = cfg.clone();
    let mut review_handle = tokio::spawn(async move { libcrucible::run_review_with_progress(&cfg_for_review, tx).await });
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
        let json = serde_json::to_string_pretty(&report)?;
        println!("{json}");
        write_log_json(&log, &json);
        return Ok(());
    }

    print_report(&report);
    let json = serde_json::to_string_pretty(&report)?;
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

fn print_report(report: &ReviewReport) {
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

    println!("Crucible Review — {} findings ({} Critical, {} Warning, {} Info)", report.findings.len(), critical, warning, info);
    println!();
    for f in &report.findings {
        let loc = match (&f.file, &f.span) {
            (Some(file), Some(span)) => format!("{}:{}", file.display(), span.start),
            (Some(file), None) => file.display().to_string(),
            _ => "<unknown>".to_string(),
        };
        println!("  [{:8}]  {:20}  {}", format_severity(&f.severity), loc, f.message);
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

fn emit_progress(event: &ProgressEvent) {
    match event {
        ProgressEvent::AnalyzerStart => eprintln!("[progress] analyzer:start"),
        ProgressEvent::AnalyzerDone => eprintln!("[progress] analyzer:done"),
        ProgressEvent::RoundStart { round, agents, .. } => {
            eprintln!("[progress] round:{} start (agents: {})", round, agents.join(","));
        }
        ProgressEvent::AgentStart { round, id } => {
            eprintln!("[progress] agent:start round={} id={}", round, id);
        }
        ProgressEvent::AgentDone { round, id } => {
            eprintln!("[progress] agent:done round={} id={}", round, id);
        }
        ProgressEvent::AgentError { round, id, message } => {
            eprintln!("[progress] agent:error round={} id={} msg={}", round, id, message);
        }
        ProgressEvent::RoundDone { round } => eprintln!("[progress] round:{} done", round),
        ProgressEvent::AutoFixReady => eprintln!("[progress] autofix:ready"),
        ProgressEvent::Completed(_) => {}
        ProgressEvent::Canceled => eprintln!("[progress] canceled"),
    }
}

fn open_review_log() -> Result<std::fs::File> {
    let path = std::env::current_dir()?.join("review_report.log");
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    Ok(file)
}

fn write_log_event(log: &Arc<Mutex<std::fs::File>>, event: &ProgressEvent) -> Result<()> {
    let mut file = log.lock().expect("log lock");
    match event {
        ProgressEvent::AnalyzerStart => writeln!(file, "[progress] analyzer:start")?,
        ProgressEvent::AnalyzerDone => writeln!(file, "[progress] analyzer:done")?,
        ProgressEvent::RoundStart { round, agents, .. } => {
            writeln!(file, "[progress] round:{} start (agents: {})", round, agents.join(","))?
        }
        ProgressEvent::AgentStart { round, id } => {
            writeln!(file, "[progress] agent:start round={} id={}", round, id)?
        }
        ProgressEvent::AgentDone { round, id } => {
            writeln!(file, "[progress] agent:done round={} id={}", round, id)?
        }
        ProgressEvent::AgentError { round, id, message } => {
            writeln!(file, "[progress] agent:error round={} id={} msg={}", round, id, message)?
        }
        ProgressEvent::RoundDone { round } => writeln!(file, "[progress] round:{} done", round)?,
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
