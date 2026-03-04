use anyhow::Result;
use clap::Args;
use libcrucible::config::CrucibleConfig;
use libcrucible::report::{ReviewReport, Verdict};
use std::io::IsTerminal;

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
    if args.json {
        let report = libcrucible::run_review(&cfg).await?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let use_tui = !args.hook && std::io::stdout().is_terminal();
    if use_tui {
        let exit_code = crate::tui::run_review_tui(&cfg).await?;
        std::process::exit(exit_code);
    }

    let report = libcrucible::run_review(&cfg).await?;
    print_report(&report);

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
