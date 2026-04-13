use libcrucible::progress::{
    ConvergenceVerdict, ProgressEvent, ReviewerState, StartupPhase, StartupPhaseStatus,
};
use libcrucible::report::ReviewReport;
use chrono::Local;
use std::io::Write;

/// Returns a human-readable local timestamp with milliseconds.
pub fn log_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S%.3f %:z").to_string()
}

/// Serialize a `ReviewReport` to pretty JSON, with a manual fallback if `serde`
/// derives fail on any inner type.
pub fn render_report_json(report: &ReviewReport) -> String {
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
                "run_id": report.run_id,
                "verdict": report.verdict,
                "findings": report.findings,
                "agent_failures": report.agent_failures,
                "issues": report.issues,
                "analysis_markdown": report.analysis_markdown,
                "system_context_markdown": report.system_context_markdown,
                "final_analysis_markdown": report.final_analysis_markdown,
                "consensus": consensus,
                "auto_fix": report.auto_fix,
                "final_action_plan": report.final_action_plan,
                "pr_comment_markdown": report.pr_comment_markdown,
                "pr_review_draft": report.pr_review_draft,
                "session_id": report.session_id
            }))
            .unwrap_or_else(|_| "{}".to_string())
        }
    }
}

/// Write a progress event to a log writer. Errors are silently ignored.
pub fn write_log_event(w: &mut dyn Write, event: &ProgressEvent) {
    let _ = write!(w, "[{}] ", log_timestamp());
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
                w,
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
            let _ = writeln!(w, "[progress] phase:start {}", phase);
        }
        ProgressEvent::PhaseDone { phase } => {
            let _ = writeln!(w, "[progress] phase:done {}", phase);
        }
        ProgressEvent::AnalyzerStart => {
            let _ = writeln!(w, "[progress] analyzer:start");
        }
        ProgressEvent::AnalyzerDone => {
            let _ = writeln!(w, "[progress] analyzer:done");
        }
        ProgressEvent::AnalysisSource {
            id,
            role,
            plugin,
            fallback,
        } => {
            let mode = if *fallback { "fallback" } else { "agent" };
            let _ = writeln!(
                w,
                "[progress] analyzer:source id={} role={} plugin={} mode={}",
                id, role, plugin, mode
            );
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
                .map(|value| format!(" duration={}", format_duration(value)))
                .unwrap_or_default();
            let _ = writeln!(
                w,
                "[progress] startup:{} {}{}{} {}",
                format_startup_phase(*phase),
                format_startup_status(*status),
                count_suffix,
                duration_suffix,
                detail
            );
        }
        ProgressEvent::AnalysisReady { markdown } => {
            let _ = writeln!(w, "[analysis]");
            let _ = writeln!(w, "{}", markdown);
        }
        ProgressEvent::SystemContextReady { markdown } => {
            let _ = writeln!(w, "[system-context]");
            let _ = writeln!(w, "{}", markdown);
        }
        ProgressEvent::RoundStart { round, agents, .. } => {
            let _ = writeln!(
                w,
                "[progress] round:{} start (agents: {})",
                round,
                agents.join(",")
            );
        }
        ProgressEvent::ParallelStatus { round, statuses } => {
            let _ = writeln!(
                w,
                "[progress] round:{} status {}",
                round,
                format_parallel_status(statuses)
            );
        }
        ProgressEvent::AgentStart { round, id } => {
            let _ = writeln!(w, "[progress] agent:start round={} id={}", round, id);
        }
        ProgressEvent::AgentTranscript {
            id,
            direction,
            message,
        } => {
            let arrow = match direction {
                libcrucible::progress::TranscriptDirection::ToAgent => "->",
                libcrucible::progress::TranscriptDirection::FromAgent => "<-",
            };
            let _ = writeln!(w, "[agent-chat] {} {} {}", arrow, id, message);
        }
        ProgressEvent::AgentReview {
            round,
            id,
            summary,
            highlights,
            details,
        } => {
            let _ = writeln!(w, "[agent-review] round={} id={} {}", round, id, summary);
            let _ = writeln!(w, "[agent-review] details:\n{}", details);
            for h in highlights {
                let _ = writeln!(
                    w,
                    "[agent-review]   [{}] {} {}",
                    h.severity, h.location, h.message
                );
            }
        }
        ProgressEvent::AgentDone { round, id } => {
            let _ = writeln!(w, "[progress] agent:done round={} id={}", round, id);
        }
        ProgressEvent::AgentError { round, id, message } => {
            let _ = writeln!(
                w,
                "[progress] agent:error round={} id={} msg={}",
                round, id, message
            );
        }
        ProgressEvent::RoundDone { round } => {
            let _ = writeln!(w, "[progress] round:{} done", round);
        }
        ProgressEvent::ConvergenceJudgment {
            round,
            verdict,
            rationale,
        } => {
            let _ = writeln!(
                w,
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
            let _ = writeln!(w, "[progress] round:{}/{} complete", round, total_rounds);
        }
        ProgressEvent::AutoFixReady => {
            let _ = writeln!(w, "[progress] autofix:ready");
        }
        ProgressEvent::Completed(_) => {}
        ProgressEvent::Canceled => {
            let _ = writeln!(w, "[progress] canceled");
        }
    }
    let _ = w.flush();
}

/// Write the JSON report block to a log writer.
pub fn write_log_json(w: &mut dyn Write, json: &str) {
    let _ = writeln!(w, "[{}] [report]", log_timestamp());
    let _ = writeln!(w, "{}", json);
    let _ = w.flush();
}

/// Write final-analysis and pr-comment sections to a log writer.
pub fn write_log_report_sections(w: &mut dyn Write, report: &ReviewReport) {
    if let Some(final_analysis) = &report.final_analysis_markdown {
        let _ = writeln!(w, "[{}] [final-analysis]", log_timestamp());
        let _ = writeln!(w, "{}", final_analysis);
    }
    if let Some(comment) = &report.pr_comment_markdown {
        let _ = writeln!(w, "[{}] [pr-comment]", log_timestamp());
        let _ = writeln!(w, "{}", comment);
    }
    let _ = w.flush();
}

pub fn format_parallel_status(statuses: &[libcrucible::progress::ReviewerStatus]) -> String {
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

pub fn format_duration(seconds: f32) -> String {
    format!("{seconds:.1}s")
}

pub fn format_convergence(verdict: ConvergenceVerdict) -> &'static str {
    match verdict {
        ConvergenceVerdict::Converged => "CONVERGED",
        ConvergenceVerdict::NotConverged => "NOT_CONVERGED",
    }
}

pub fn format_startup_phase(phase: StartupPhase) -> &'static str {
    match phase {
        StartupPhase::References => "references",
        StartupPhase::History => "history",
        StartupPhase::Docs => "docs",
        StartupPhase::Prechecks => "prechecks",
    }
}

pub fn format_startup_status(status: StartupPhaseStatus) -> &'static str {
    match status {
        StartupPhaseStatus::Started => "started",
        StartupPhaseStatus::Completed => "completed",
        StartupPhaseStatus::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_uses_one_decimal_second() {
        assert_eq!(format_duration(0.04), "0.0s");
        assert_eq!(format_duration(1.06), "1.1s");
        assert_eq!(format_duration(12.34), "12.3s");
    }

    #[test]
    fn log_timestamp_has_three_digit_millis() {
        let ts = log_timestamp();
        assert!(
            ts.contains('-') && ts.contains(':') && ts.contains('.'),
            "timestamp should be human-readable: {ts}"
        );
        assert!(
            ts.ends_with("+00:00")
                || ts.ends_with("+10:00")
                || ts.ends_with("+11:00")
                || ts.contains(" +")
                || ts.contains(" -"),
            "timestamp should include a numeric offset: {ts}"
        );
    }
}
