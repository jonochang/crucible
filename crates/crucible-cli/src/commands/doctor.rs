use anyhow::Result;
use clap::Args;
use std::io::Write;

#[derive(Args)]
pub struct DoctorArgs {
    #[arg(long, help = "Only check config and agent resolution, skip live agent probes")]
    pub skip_probes: bool,
}

pub fn run(args: DoctorArgs) -> Result<()> {
    let mut pass = true;
    let mut out = std::io::stdout().lock();

    writeln!(out, "crucible doctor")?;
    writeln!(out, "===============\n")?;

    let report = libcrucible::run_doctor(args.skip_probes);

    write!(out, "Config: ")?;
    if report.config_ok {
        if let Some(path) = &report.config_path {
            writeln!(out, "OK ({})", path)?;
        } else {
            writeln!(out, "OK (defaults)")?;
        }
    } else {
        pass = false;
        writeln!(
            out,
            "FAIL {}",
            report.config_error.as_deref().unwrap_or("unknown error")
        )?;
    }

    write!(out, "Agent resolution: ")?;
    if report.agent_resolution.ok {
        writeln!(out, "OK")?;
        writeln!(
            out,
            "  Active:  {}",
            if report.agent_resolution.active.is_empty() {
                "(none)".to_string()
            } else {
                report.agent_resolution.active.join(", ")
            }
        )?;
        writeln!(
            out,
            "  Standby: {}",
            if report.agent_resolution.standby.is_empty() {
                "(none)".to_string()
            } else {
                report.agent_resolution.standby.join(", ")
            }
        )?;
    } else {
        pass = false;
        writeln!(
            out,
            "FAIL {}",
            report.agent_resolution.error.as_deref().unwrap_or("unknown error")
        )?;
    }

    write!(out, "Execution plan ({}): ", report.execution_plan.pack_id)?;
    if report.execution_plan.ok {
        writeln!(out, "OK")?;
        for round in &report.execution_plan.rounds {
            writeln!(out, "  Round: {round}")?;
        }
        for (role, plugin) in &report.execution_plan.roles {
            writeln!(out, "    {role} -> {plugin}")?;
        }
    } else {
        pass = false;
        writeln!(
            out,
            "FAIL {}",
            report.execution_plan.error.as_deref().unwrap_or("unknown error")
        )?;
    }

    if !report.agent_checks.is_empty() {
        let active: Vec<&str> = report.agent_resolution.active.iter().map(|s| s.as_str()).collect();
        let standby: Vec<&str> = report.agent_resolution.standby.iter().map(|s| s.as_str()).collect();
        writeln!(out, "\nAgent probes:")?;
        for check in &report.agent_checks {
        let agent_name = check.agent_id.strip_prefix("doctor@").unwrap_or(&check.agent_id);
        let tag = if active.contains(&agent_name) {
            "active"
        } else if standby.contains(&agent_name) {
            "standby"
        } else {
            "not on PATH"
        };
        if check.reachable && check.json_parsable && check.valid_response {
            writeln!(out, "  {} ({tag}): OK", agent_name)?;
            } else if check.reachable {
                writeln!(
                    out,
                    "  {} ({tag}): WARN  reachable but response parse failed ({})",
                    agent_name,
                    check.error.as_deref().unwrap_or("unexpected output")
                )?;
            } else {
                pass = false;
                writeln!(
                    out,
                    "  {} ({tag}): FAIL  not reachable - {}",
                    agent_name,
                    check.error.as_deref().unwrap_or("binary not found")
                )?;
            }
        }
    }

    writeln!(out)?;
    if pass {
        writeln!(out, "All checks passed.")?;
        Ok(())
    } else {
        writeln!(out, "Some checks failed.")?;
        std::process::exit(1);
    }
}
