use crate::analysis::FocusAreas;
use crate::config::CrucibleConfig;
use crate::context::ReviewContext;
use crate::plugin::{AgentReviewOutput, PluginRegistry};
use crate::progress::{ConvergenceVerdict, ProgressEvent, ReviewerState, ReviewerStatus};
use crate::report::{
    ConsensusMap, ConsensusStatus, Finding, FindingKey, LineSpan, RawFinding, Severity,
};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub struct Coordinator {
    registry: PluginRegistry,
    cfg: CrucibleConfig,
    snapshotter: MessageSnapshotter,
    consensus: ConsensusTracker,
    progress: Option<tokio::sync::mpsc::UnboundedSender<crate::progress::ProgressEvent>>,
}

impl Coordinator {
    pub fn new(
        registry: PluginRegistry,
        cfg: CrucibleConfig,
        progress: Option<tokio::sync::mpsc::UnboundedSender<crate::progress::ProgressEvent>>,
    ) -> Self {
        let consensus =
            ConsensusTracker::new(cfg.coordinator.quorum_threshold, cfg.plugins.agents.len());
        Self {
            registry,
            cfg,
            snapshotter: MessageSnapshotter::default(),
            consensus,
            progress,
        }
    }

    pub async fn run(&mut self, ctx: &ReviewContext) -> Result<crate::report::ReviewReport> {
        self.emit(ProgressEvent::RunHeader {
            reviewers: self
                .registry
                .agents
                .iter()
                .map(|a| a.id().to_string())
                .collect(),
            max_rounds: self.cfg.coordinator.max_rounds.max(1),
            changed_files: ctx.changed_files.len(),
            changed_lines: count_changed_lines(&ctx.diff),
            convergence_enabled: self.cfg.coordinator.max_rounds > 1,
            context_enabled: true,
        });
        self.emit(ProgressEvent::PhaseStart {
            phase: "analyzer".to_string(),
        });
        self.emit(ProgressEvent::AnalyzerStart);
        let focus = self
            .registry
            .analyzer
            .analyze_focus(&ctx.into_agent_ctx(None))
            .await?;
        self.emit(ProgressEvent::AnalysisReady {
            markdown: render_analysis_markdown(&focus),
        });
        self.emit(ProgressEvent::SystemContextReady {
            markdown: render_system_context_markdown(ctx),
        });
        self.emit(ProgressEvent::AnalyzerDone);
        self.emit(ProgressEvent::PhaseDone {
            phase: "analyzer".to_string(),
        });

        let total_rounds = self.cfg.coordinator.max_rounds.max(1);
        let agent_ctx = ctx.into_agent_ctx(Some(&focus));
        let mut previous_count = 0usize;
        let mut prior_round_findings: HashMap<String, Vec<RawFinding>> = HashMap::new();
        for round in 1..=total_rounds {
            self.snapshotter.freeze_round(round, &HashMap::new());
            let agents = self
                .registry
                .agents
                .iter()
                .map(|a| a.id().to_string())
                .collect();
            self.emit(ProgressEvent::PhaseStart {
                phase: format!("round-{round}"),
            });
            self.emit(ProgressEvent::RoundStart {
                round,
                total_rounds,
                agents,
            });

            let mut statuses = self
                .registry
                .agents
                .iter()
                .map(|a| ReviewerStatus {
                    id: a.id().to_string(),
                    state: ReviewerState::Queued,
                    duration_secs: None,
                })
                .collect::<Vec<_>>();
            let mut started_at: HashMap<String, Instant> = HashMap::new();
            self.emit(ProgressEvent::ParallelStatus {
                round,
                statuses: statuses.clone(),
            });

            let mut round_findings: HashMap<String, Vec<RawFinding>> = HashMap::new();
            for agent in &self.registry.agents {
                let id = agent.id();
                update_status(&mut statuses, id, ReviewerState::Running, None);
                started_at.insert(id.to_string(), Instant::now());
                self.emit(ProgressEvent::ParallelStatus {
                    round,
                    statuses: statuses.clone(),
                });
                self.emit(ProgressEvent::AgentStart {
                    round,
                    id: id.to_string(),
                });
                let output = match if round == 1 {
                    agent.analyze(&agent_ctx).await
                } else {
                    let synthesis = CrossPollinationSynthesis {
                        summary: format_round_synthesis(round - 1, &prior_round_findings),
                    };
                    agent.debate(&agent_ctx, round, &synthesis).await
                } {
                    Ok(output) => output,
                    Err(err) => {
                        update_status(&mut statuses, id, ReviewerState::Error, None);
                        self.emit(ProgressEvent::ParallelStatus {
                            round,
                            statuses: statuses.clone(),
                        });
                        self.emit(ProgressEvent::AgentError {
                            round,
                            id: id.to_string(),
                            message: err.to_string(),
                        });
                        return Err(err);
                    }
                };
                self.consensus.ingest_round(&output.findings, round, id);
                round_findings.insert(id.to_string(), output.findings.clone());
                let (summary, highlights) = summarize_agent_output(&output);
                self.emit(ProgressEvent::AgentReview {
                    round,
                    id: id.to_string(),
                    summary,
                    highlights,
                });
                let elapsed = started_at
                    .remove(id)
                    .map(|start| start.elapsed().as_secs_f32())
                    .unwrap_or(0.0);
                update_status(&mut statuses, id, ReviewerState::Done, Some(elapsed));
                self.emit(ProgressEvent::ParallelStatus {
                    round,
                    statuses: statuses.clone(),
                });
                self.emit(ProgressEvent::AgentDone {
                    round,
                    id: id.to_string(),
                });
            }
            prior_round_findings = round_findings;
            self.emit(ProgressEvent::RoundDone { round });

            let current_count = self.consensus.all_findings().len();
            if total_rounds > 1 && round < total_rounds {
                let converged = round > 1 && current_count == previous_count;
                let verdict = if converged {
                    ConvergenceVerdict::Converged
                } else {
                    ConvergenceVerdict::NotConverged
                };
                let rationale = if converged {
                    "No net-new findings compared with prior round.".to_string()
                } else {
                    "New findings were raised; another round required.".to_string()
                };
                self.emit(ProgressEvent::ConvergenceJudgment {
                    round,
                    verdict,
                    rationale,
                });
                if converged {
                    self.emit(ProgressEvent::RoundComplete {
                        round,
                        total_rounds,
                    });
                    self.emit(ProgressEvent::PhaseDone {
                        phase: format!("round-{round}"),
                    });
                    break;
                }
            }
            previous_count = current_count;
            self.emit(ProgressEvent::RoundComplete {
                round,
                total_rounds,
            });
            self.emit(ProgressEvent::PhaseDone {
                phase: format!("round-{round}"),
            });
        }
        let findings = self.consensus.all_findings();

        self.emit(ProgressEvent::PhaseStart {
            phase: "finalize".to_string(),
        });
        let auto_fix = if findings.iter().any(|f| f.severity >= Severity::Warning) {
            Some(self.registry.judge.summarize(&agent_ctx, &findings).await?)
        } else {
            None
        };
        if auto_fix.is_some() {
            self.emit(ProgressEvent::AutoFixReady);
        }

        let report = crate::report::ReviewReport::from_findings(
            &findings,
            &self.cfg.verdict,
            self.consensus.consensus_map(),
            auto_fix,
        );
        self.emit(ProgressEvent::Completed(report.clone()));
        self.emit(ProgressEvent::PhaseDone {
            phase: "finalize".to_string(),
        });
        Ok(report)
    }

    fn emit(&self, event: ProgressEvent) {
        if let Some(tx) = &self.progress {
            let _ = tx.send(event);
        }
    }
}

fn update_status(
    statuses: &mut [ReviewerStatus],
    id: &str,
    state: ReviewerState,
    duration_secs: Option<f32>,
) {
    for s in statuses.iter_mut() {
        if s.id == id {
            s.state = state;
            if duration_secs.is_some() {
                s.duration_secs = duration_secs;
            }
            break;
        }
    }
}

fn count_changed_lines(diff: &str) -> usize {
    diff.lines()
        .filter(|line| line.starts_with('+') || line.starts_with('-'))
        .filter(|line| !line.starts_with("+++ ") && !line.starts_with("--- "))
        .count()
}

fn render_analysis_markdown(focus: &FocusAreas) -> String {
    let mut out = String::new();
    out.push_str("## Local Diff Analysis\n\n");
    out.push_str(&focus.summary);
    if !focus.focus_items.is_empty() {
        out.push_str("\n\n### Suggested Review Focus\n");
        for item in &focus.focus_items {
            out.push_str(&format!("- {}: {}\n", item.area, item.rationale));
        }
    }
    if !focus.trade_offs.is_empty() {
        out.push_str("\n### Trade-offs\n");
        for tradeoff in &focus.trade_offs {
            out.push_str(&format!("- {}\n", tradeoff));
        }
    }
    out
}

fn render_system_context_markdown(ctx: &ReviewContext) -> String {
    let mut out = String::new();
    out.push_str("### System Context\n");
    out.push_str(&format!("- Changed files: {}\n", ctx.changed_files.len()));
    out.push_str(&format!(
        "- Reference snippets: {}\n",
        ctx.gathered.references.len()
    ));
    out.push_str(&format!(
        "- History commits: {}\n",
        ctx.gathered.history.len()
    ));
    out.push_str(&format!("- Docs snippets: {}\n", ctx.gathered.docs.len()));
    if !ctx.changed_files.is_empty() {
        out.push_str("- Affected files:\n");
        for file in &ctx.changed_files {
            out.push_str(&format!("  - {}\n", file.display()));
        }
    }
    out
}

fn format_round_synthesis(round: u8, findings: &HashMap<String, Vec<RawFinding>>) -> String {
    let mut out = format!("Round {round} prior reviewer findings:\n");
    let mut ids = findings.keys().cloned().collect::<Vec<_>>();
    ids.sort();
    for id in ids {
        out.push_str(&format!("\n- {id}\n"));
        if let Some(agent_findings) = findings.get(&id) {
            if agent_findings.is_empty() {
                out.push_str("  - No findings\n");
            } else {
                for finding in agent_findings {
                    let loc = match (&finding.file, finding.line_start) {
                        (Some(file), Some(line)) => format!("{}:{}", file.display(), line),
                        (Some(file), None) => file.display().to_string(),
                        _ => "<unknown>".to_string(),
                    };
                    out.push_str(&format!(
                        "  - [{:?}] {} {}\n",
                        finding.severity, loc, finding.message
                    ));
                }
            }
        }
    }
    out
}

pub fn parse_convergence_verdict(input: &str) -> Option<ConvergenceVerdict> {
    if input.contains("NOT_CONVERGED") {
        return Some(ConvergenceVerdict::NotConverged);
    }
    if input.contains("CONVERGED") {
        return Some(ConvergenceVerdict::Converged);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_convergence_verdict;
    use crate::progress::ConvergenceVerdict;

    #[test]
    fn parse_convergence_token() {
        assert_eq!(
            parse_convergence_verdict("Verdict: CONVERGED"),
            Some(ConvergenceVerdict::Converged)
        );
        assert_eq!(
            parse_convergence_verdict("NOT_CONVERGED due to unresolved issues"),
            Some(ConvergenceVerdict::NotConverged)
        );
        assert_eq!(parse_convergence_verdict("unknown"), None);
    }
}

fn summarize_agent_output(
    output: &AgentReviewOutput,
) -> (String, Vec<crate::progress::AgentFindingPreview>) {
    let critical = output
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Critical)
        .count();
    let warning = output
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Warning)
        .count();
    let info = output
        .findings
        .iter()
        .filter(|f| f.severity == Severity::Info)
        .count();
    let mut summary = if output.findings.is_empty() {
        "No findings".to_string()
    } else {
        format!(
            "{} findings ({} Critical, {} Warning, {} Info)",
            output.findings.len(),
            critical,
            warning,
            info
        )
    };
    let narrative = output.narrative.trim();
    if !narrative.is_empty() {
        summary = format!(
            "{} | {}",
            summary,
            narrative.lines().next().unwrap_or_default().trim()
        );
    }

    let mut ranked = output.findings.clone();
    ranked.sort_by(|a, b| b.severity.cmp(&a.severity));
    let highlights = ranked
        .into_iter()
        .take(3)
        .map(|f| {
            let location = match (&f.file, f.line_start) {
                (Some(file), Some(line)) => format!("{}:{}", file.display(), line),
                (Some(file), None) => file.display().to_string(),
                _ => "<unknown>".to_string(),
            };
            crate::progress::AgentFindingPreview {
                severity: format!("{:?}", f.severity).to_uppercase(),
                location,
                message: f.message,
            }
        })
        .collect();

    (summary, highlights)
}

#[derive(Debug, Clone, Default)]
pub struct MessageSnapshotter {
    rounds: Vec<RoundSnapshot>,
}

#[derive(Debug, Clone)]
pub struct RoundSnapshot {
    pub round: u8,
    pub messages: HashMap<String, Vec<AgentMessage>>,
}

#[derive(Debug, Clone)]
pub struct AgentMessage {
    pub role: String,
    pub content: String,
}

impl MessageSnapshotter {
    pub fn freeze_round(&mut self, round: u8, messages: &HashMap<String, Vec<AgentMessage>>) {
        self.rounds.push(RoundSnapshot {
            round,
            messages: messages.clone(),
        });
    }

    pub fn get_snapshot(&self, round: u8) -> Option<&RoundSnapshot> {
        self.rounds.iter().find(|r| r.round == round)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CrossPollinationSynthesis {
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct ConsensusTracker {
    clusters: HashMap<FindingKey, ClusterState>,
    quorum: f32,
    agents: usize,
    loose: Vec<Finding>,
}

#[derive(Debug, Clone)]
struct ClusterState {
    findings: Vec<Finding>,
    severity: Severity,
    agents: HashSet<String>,
}

impl ConsensusTracker {
    pub fn new(quorum: f32, agents: usize) -> Self {
        Self {
            clusters: HashMap::new(),
            quorum,
            agents: agents.max(1),
            loose: Vec::new(),
        }
    }

    pub fn ingest_round(&mut self, raw: &[RawFinding], round: u8, agent_id: &str) {
        for rf in raw {
            let span = match (rf.line_start, rf.line_end) {
                (Some(start), Some(end)) => Some(LineSpan { start, end }),
                (Some(start), None) => Some(LineSpan { start, end: start }),
                _ => None,
            };
            let key = rf
                .file
                .clone()
                .and_then(|f| span.clone().map(|s| FindingKey { file: f, span: s }));
            let finding = Finding {
                agent: agent_id.to_string(),
                severity: rf.severity.clone(),
                file: rf.file.clone(),
                span,
                message: rf.message.clone(),
                round,
                raised_by: vec![agent_id.to_string()],
            };

            if let Some(key) = key {
                let mut merged_key = None;
                for existing in self.clusters.keys() {
                    if same_cluster(existing, &key)
                        || message_similar(
                            &self.clusters[existing].findings[0].message,
                            &finding.message,
                        )
                    {
                        merged_key = Some(existing.clone());
                        break;
                    }
                }

                let entry_key = merged_key.unwrap_or_else(|| key.clone());
                self.clusters
                    .entry(entry_key)
                    .and_modify(|state| {
                        if !state.agents.contains(agent_id) {
                            state.findings.push(finding.clone());
                            state.agents.insert(agent_id.to_string());
                            if finding.severity > state.severity {
                                state.severity = finding.severity.clone();
                            }
                        }
                    })
                    .or_insert_with(|| {
                        let mut agents = HashSet::new();
                        agents.insert(agent_id.to_string());
                        ClusterState {
                            findings: vec![finding.clone()],
                            severity: finding.severity.clone(),
                            agents,
                        }
                    });
            } else {
                let already = self
                    .loose
                    .iter()
                    .any(|f| f.agent == finding.agent && f.message == finding.message);
                if !already {
                    self.loose.push(finding);
                }
            }
        }
    }

    pub fn consensus_map(&self) -> ConsensusMap {
        let mut map = HashMap::new();
        for (key, state) in &self.clusters {
            let agreed_count = state.agents.len();
            let reached_quorum = (agreed_count as f32) / (self.agents as f32) >= self.quorum;
            map.insert(
                key.clone(),
                ConsensusStatus {
                    agreed_count,
                    total_agents: self.agents,
                    severity: state.severity.clone(),
                    reached_quorum,
                },
            );
        }
        ConsensusMap(map)
    }

    pub fn all_findings(&self) -> Vec<Finding> {
        let mut all = self
            .clusters
            .values()
            .flat_map(|state| state.findings.clone())
            .collect::<Vec<_>>();
        all.extend(self.loose.clone());
        all
    }
}

fn same_cluster(a: &FindingKey, b: &FindingKey) -> bool {
    a.file == b.file && spans_overlap(&a.span, &b.span)
}

fn spans_overlap(a: &LineSpan, b: &LineSpan) -> bool {
    let overlap_start = a.start.max(b.start);
    let overlap_end = a.end.min(b.end);
    if overlap_end < overlap_start {
        return false;
    }
    let overlap = overlap_end - overlap_start + 1;
    let len_a = a.end - a.start + 1;
    let len_b = b.end - b.start + 1;
    let ratio = (overlap as f32) / (len_a.min(len_b) as f32);
    ratio >= 0.5
}

fn message_similar(a: &str, b: &str) -> bool {
    let tokens_a: HashSet<String> = a
        .split_whitespace()
        .map(|s| {
            s.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|s| !s.is_empty())
        .collect();
    let tokens_b: HashSet<String> = b
        .split_whitespace()
        .map(|s| {
            s.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|s| !s.is_empty())
        .collect();
    if tokens_a.is_empty() || tokens_b.is_empty() {
        return false;
    }
    let inter = tokens_a.intersection(&tokens_b).count() as f32;
    let union = tokens_a.union(&tokens_b).count() as f32;
    inter / union >= 0.35
}

#[derive(Debug, Clone)]
pub struct DebateTranscript;

#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub focus: FocusAreas,
    pub findings: Vec<Finding>,
    pub auto_fix: Option<crate::report::AutoFix>,
}
