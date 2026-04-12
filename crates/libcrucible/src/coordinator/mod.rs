use crate::analysis::FocusAreas;
use crate::config::CrucibleConfig;
use crate::context::ReviewContext;
use crate::plugin::{AgentReviewOutput, PluginRegistry};
use crate::progress::{ConvergenceVerdict, ProgressEvent, ReviewerState, ReviewerStatus};
use crate::report::{
    AgentFailure, CanonicalIssue, ConsensusMap, ConsensusStatus, EvidenceAnchor,
    FinalActionPlan, Finding, FindingKey, LineSpan, RawFinding, Severity,
};
use anyhow::{Result, anyhow};
use futures::future::{BoxFuture, FutureExt};
use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub struct Coordinator {
    registry: PluginRegistry,
    cfg: CrucibleConfig,
    snapshotter: MessageSnapshotter,
    consensus: ConsensusTracker,
    progress: Option<tokio::sync::mpsc::UnboundedSender<crate::progress::ProgressEvent>>,
    run_id: Uuid,
    review_pack: Option<crate::task_pack::TaskPack>,
}

impl Coordinator {
    pub fn new(
        registry: PluginRegistry,
        cfg: CrucibleConfig,
        progress: Option<tokio::sync::mpsc::UnboundedSender<crate::progress::ProgressEvent>>,
        run_id: Uuid,
    ) -> Self {
        let consensus = ConsensusTracker::new(cfg.coordinator.quorum_threshold, 1);
        Self {
            registry,
            cfg,
            snapshotter: MessageSnapshotter::default(),
            consensus,
            progress,
            run_id,
            review_pack: None,
        }
    }

    pub fn with_review_pack(mut self, review_pack: crate::task_pack::TaskPack) -> Self {
        self.review_pack = Some(review_pack);
        self
    }

    pub async fn run(&mut self, ctx: &ReviewContext) -> Result<crate::report::ReviewReport> {
        let review_pack = self
            .review_pack
            .clone()
            .ok_or_else(|| anyhow!("review task pack not configured"))?;
        let task_plan = self.registry.build_execution_plan(&review_pack)?;
        let max_plan_rounds = task_plan.rounds.len().max(1);
        let total_rounds = usize::from(self.cfg.coordinator.max_rounds.max(1)).min(max_plan_rounds);
        let reviewers = task_plan
            .rounds
            .first()
            .map(|round| {
                round
                    .assignments
                    .iter()
                    .map(|assignment| assignment.runtime_id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let consensus_agents = task_plan
            .rounds
            .iter()
            .map(|round| round.assignments.len())
            .max()
            .unwrap_or(1);
        self.consensus = ConsensusTracker::new(self.cfg.coordinator.quorum_threshold, consensus_agents);
        self.emit(ProgressEvent::RunHeader {
            reviewers,
            max_rounds: total_rounds as u8,
            changed_files: ctx.changed_files.len(),
            changed_lines: count_changed_lines(&ctx.diff),
            convergence_enabled: total_rounds > 1,
            context_enabled: true,
        });
        self.emit(ProgressEvent::PhaseStart {
            phase: "analyzer".to_string(),
        });
        self.emit(ProgressEvent::AnalyzerStart);
        let mut analyzer_ctx = ctx.into_agent_ctx(None);
        analyzer_ctx.review_pack = Some(review_pack.clone());
        let mut focus = None;
        let mut agent_failures = Vec::new();
        let analyzer_attempts: u8 = 2;
        if let Some(analyze_assignment) = &task_plan.finalization.analyze {
            let analyzer = self.registry.instantiate_focus_analyzer(analyze_assignment)?;
            for attempt in 1..=analyzer_attempts {
                match analyzer.analyze_focus(&analyzer_ctx).await {
                    Ok(result) => {
                        focus = Some(result);
                        break;
                    }
                    Err(err) => {
                        let err_message = format!("{err:#}");
                        self.emit(ProgressEvent::AgentError {
                            round: 0,
                            id: analyze_assignment.runtime_id.clone(),
                            message: format!(
                                "analyzer attempt {}/{} failed: {}",
                                attempt, analyzer_attempts, err_message
                            ),
                        });
                        if attempt == analyzer_attempts {
                            agent_failures.push(AgentFailure {
                                agent: analyze_assignment.runtime_id.clone(),
                                stage: "analyzer".to_string(),
                                round: None,
                                message: err_message.clone(),
                            });
                            focus = Some(fallback_focus(ctx, &err_message));
                        }
                    }
                }
            }
        } else {
            focus = Some(fallback_focus(ctx, "no analyzer assignment configured"));
        }
        let focus = focus.expect("analyzer focus set");
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

        let mut agent_ctx = ctx.into_agent_ctx(Some(&focus));
        agent_ctx.review_pack = Some(review_pack.clone());
        let diff_chunks = chunk_diff(
            &ctx.diff,
            self.cfg.coordinator.max_diff_lines_per_chunk,
            self.cfg.coordinator.max_diff_chunks,
        );
        let using_chunking = diff_chunks.len() > 1;
        let mut previous_count = 0usize;
        let mut previous_high_count = 0usize;
        let mut prior_round_findings: HashMap<String, Vec<RawFinding>> = HashMap::new();
        self.emit(ProgressEvent::PhaseStart {
            phase: "agent-preflight".to_string(),
        });
        self.emit(ProgressEvent::PhaseDone {
            phase: "agent-preflight".to_string(),
        });
        for (round_idx, round_plan) in task_plan.rounds.iter().take(total_rounds).enumerate() {
            let round = (round_idx + 1) as u8;
            self.snapshotter.freeze_round(round, &HashMap::new());
            let active_agents = round_plan
                .assignments
                .iter()
                .map(|assignment| self.registry.instantiate_role_agent(assignment))
                .collect::<Result<Vec<_>>>()?;
            let agents = active_agents.iter().map(|a| a.id().to_string()).collect();
            self.emit(ProgressEvent::PhaseStart {
                phase: format!("round-{round}"),
            });
            self.emit(ProgressEvent::RoundStart {
                round,
                total_rounds: total_rounds as u8,
                agents,
            });

            let mut statuses = active_agents
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
            let mut pending: FuturesUnordered<BoxFuture<'static, (String, Result<AgentReviewOutput>)>> =
                FuturesUnordered::new();
            for agent in &active_agents {
                let agent = agent.clone();
                dispatch_agent_for_round(
                    &mut pending,
                    &mut statuses,
                    &mut started_at,
                    round,
                    agent,
                    agent_ctx.clone(),
                    diff_chunks.clone(),
                    prior_round_findings.clone(),
                    self.cfg.coordinator.agent_timeout_secs,
                    self,
                );
            }
            while let Some((id, output)) = pending.next().await {
                let output = match output {
                    Ok(output) => output,
                    Err(err) => {
                        let err_message = format!("{err:#}");
                        update_status(&mut statuses, &id, ReviewerState::Error, None);
                        self.emit(ProgressEvent::ParallelStatus {
                            round,
                            statuses: statuses.clone(),
                        });
                        self.emit(ProgressEvent::AgentError {
                            round,
                            id: id.clone(),
                            message: err_message.clone(),
                        });
                        agent_failures.push(AgentFailure {
                            agent: id.clone(),
                            stage: "review".to_string(),
                            round: Some(round),
                            message: err_message,
                        });
                        continue;
                    }
                };
                self.consensus
                    .ingest_round(&output.findings, round, &id, &ctx.diff);
                round_findings.insert(id.clone(), output.findings.clone());
                let (summary, highlights, details) =
                    summarize_agent_output(&output, using_chunking);
                self.emit(ProgressEvent::AgentReview {
                    round,
                    id: id.clone(),
                    summary,
                    highlights,
                    details,
                });
                let elapsed = started_at
                    .remove(&id)
                    .map(|start| start.elapsed().as_secs_f32())
                    .unwrap_or(0.0);
                update_status(&mut statuses, &id, ReviewerState::Done, Some(elapsed));
                self.emit(ProgressEvent::ParallelStatus {
                    round,
                    statuses: statuses.clone(),
                });
                self.emit(ProgressEvent::AgentDone {
                    round,
                    id,
                });
            }
            drop(pending);
            prior_round_findings = round_findings;
            self.emit(ProgressEvent::RoundDone { round });

            let current_findings = self.consensus.all_findings();
            let current_count = current_findings.len();
            let current_high_count = current_findings
                .iter()
                .filter(|f| f.severity == Severity::Critical)
                .count();
            let round_completed_cleanly = statuses
                .iter()
                .all(|status| status.state == ReviewerState::Done);
            // Early-exit: skip convergence judge + further rounds when
            // round 1 produced zero findings and all reviewers completed cleanly.
            if current_count == 0 && round < total_rounds && round_completed_cleanly {
                self.emit(ProgressEvent::ConvergenceJudgment {
                    round,
                    verdict: ConvergenceVerdict::Converged,
                    rationale: "No findings to debate; skipping further rounds.".to_string(),
                });
                self.emit(ProgressEvent::RoundComplete {
                    round,
                    total_rounds: total_rounds as u8,
                });
                self.emit(ProgressEvent::PhaseDone {
                    phase: format!("round-{round}"),
                });
                break;
            }
            if total_rounds > 1 && round < total_rounds {
                let convergence_assignment = task_plan
                    .finalization
                    .convergence
                    .as_ref()
                    .unwrap_or(&task_plan.finalization.judge);
                let convergence_judge = self.registry.instantiate_role_agent(convergence_assignment)?;
                let judge_decision = self
                    .run_with_timeout(
                        convergence_judge.judge_convergence(&agent_ctx, round, &current_findings),
                        "judge_convergence",
                    )
                    .await;
                let (verdict, rationale) = if let Ok(decision) = judge_decision {
                    (decision.verdict, decision.rationale)
                } else {
                    let converged = round > 1
                        && current_count == previous_count
                        && current_high_count <= previous_high_count;
                    let verdict = if converged {
                        ConvergenceVerdict::Converged
                    } else {
                        ConvergenceVerdict::NotConverged
                    };
                    let rationale = if converged {
                        "No net-new findings and no increase in critical risk.".to_string()
                    } else {
                        "Net-new findings or unresolved high-severity issues remain.".to_string()
                    };
                    (verdict, rationale)
                };
                self.emit(ProgressEvent::ConvergenceJudgment {
                    round,
                    verdict,
                    rationale,
                });
                let converged = verdict == ConvergenceVerdict::Converged;
                if converged {
                    self.emit(ProgressEvent::RoundComplete {
                        round,
                        total_rounds: total_rounds as u8,
                    });
                    self.emit(ProgressEvent::PhaseDone {
                        phase: format!("round-{round}"),
                    });
                    break;
                }
            }
            previous_count = current_count;
            previous_high_count = current_high_count;
            self.emit(ProgressEvent::RoundComplete {
                round,
                total_rounds: total_rounds as u8,
            });
            self.emit(ProgressEvent::PhaseDone {
                phase: format!("round-{round}"),
            });
        }
        let findings = self.consensus.all_findings();
        let mut issues = build_canonical_issues(&findings);
        if self.cfg.coordinator.enable_structurizer {
            let structurizer_assignment = task_plan
                .finalization
                .structurizer
                .as_ref()
                .unwrap_or(&task_plan.finalization.judge);
            let structurizer = self.registry.instantiate_role_agent(structurizer_assignment)?;
            if let Ok(structured) = self
                .run_with_timeout(
                    structurizer.structurize_issues(&agent_ctx, &findings),
                    "structurize_issues",
                )
                .await
            {
                if !structured.is_empty() {
                    issues = merge_structured_issues(issues, structured);
                }
            }
        }
        let action_plan = build_action_plan(&issues);
        let pr_comment = render_pr_comment(&issues, &action_plan);
        let pr_review_draft =
            crate::pr_review::build_review_draft(pr_comment.clone(), &issues, &ctx.diff);

        self.emit(ProgressEvent::PhaseStart {
            phase: "finalize".to_string(),
        });
        let auto_fix = if findings.iter().any(|f| f.severity >= Severity::Warning) {
            let autofix_assignment = task_plan
                .finalization
                .autofix
                .as_ref()
                .unwrap_or(&task_plan.finalization.judge);
            let autofix_agent = self.registry.instantiate_role_agent(autofix_assignment)?;
            self.run_with_timeout(
                autofix_agent.summarize(&agent_ctx, &findings),
                "final_summarize",
            )
            .await
            .ok()
        } else {
            None
        };
        if auto_fix.is_some() {
            self.emit(ProgressEvent::AutoFixReady);
        }

        let final_analysis =
            render_final_analysis_markdown(&issues, &action_plan, &agent_failures);
        let report = crate::report::ReviewReport::from_findings(
            self.run_id,
            &findings,
            agent_failures,
            issues,
            Some(render_analysis_markdown(&focus)),
            Some(render_system_context_markdown(ctx)),
            Some(final_analysis),
            &self.cfg.verdict,
            self.consensus.consensus_map(),
            auto_fix,
            Some(action_plan),
            Some(pr_comment),
            Some(pr_review_draft),
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

    async fn run_with_timeout<T>(
        &self,
        fut: impl std::future::Future<Output = Result<T>>,
        label: &str,
    ) -> Result<T> {
        match tokio::time::timeout(
            Duration::from_secs(self.cfg.coordinator.agent_timeout_secs.max(1)),
            fut,
        )
        .await
        {
            Ok(res) => res,
            Err(_) => Err(anyhow!(
                "{} timed out after {}s",
                label,
                self.cfg.coordinator.agent_timeout_secs
            )),
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

fn dispatch_agent_for_round(
    pending: &mut FuturesUnordered<BoxFuture<'static, (String, Result<AgentReviewOutput>)>>,
    statuses: &mut [ReviewerStatus],
    started_at: &mut HashMap<String, Instant>,
    round: u8,
    agent: std::sync::Arc<dyn crate::plugin::AgentPlugin>,
    agent_ctx: crate::analysis::AgentContext,
    diff_chunks: Vec<String>,
    prior_round_findings: HashMap<String, Vec<RawFinding>>,
    timeout_secs: u64,
    coordinator: &Coordinator,
) {
    let id = agent.id().to_string();
    update_status(statuses, &id, ReviewerState::Running, None);
    started_at.insert(id.clone(), Instant::now());
    coordinator.emit(ProgressEvent::ParallelStatus {
        round,
        statuses: statuses.to_vec(),
    });
    coordinator.emit(ProgressEvent::AgentStart {
        round,
        id: id.clone(),
    });
    pending.push(
        async move {
            let output = run_agent_for_round(
                agent.as_ref(),
                &agent_ctx,
                round,
                &diff_chunks,
                &prior_round_findings,
                timeout_secs,
            )
            .await;
            (id, output)
        }
        .boxed(),
    );
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
    if !focus.affected_modules.is_empty() {
        out.push_str("\n### Affected Modules\n");
        for module in &focus.affected_modules {
            out.push_str(&format!("- {}\n", module));
        }
    }
    if !focus.call_chain.is_empty() {
        out.push_str("\n### Call Chain\n");
        for item in &focus.call_chain {
            out.push_str(&format!("- {}\n", item));
        }
    }
    if !focus.design_patterns.is_empty() {
        out.push_str("\n### Design Patterns\n");
        for pattern in &focus.design_patterns {
            out.push_str(&format!("- {}\n", pattern));
        }
    }
    if !focus.reviewer_checklist.is_empty() {
        out.push_str("\n### Reviewer Checklist\n");
        for item in &focus.reviewer_checklist {
            out.push_str(&format!("- {}\n", item));
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
    out.push_str(&format!(
        "- Deterministic prechecks: {}\n",
        ctx.gathered.prechecks.len()
    ));
    if !ctx.changed_files.is_empty() {
        out.push_str("- Affected files:\n");
        for file in &ctx.changed_files {
            out.push_str(&format!("  - {}\n", file.display()));
        }
    }
    if !ctx.gathered.prechecks.is_empty() {
        out.push_str("- Precheck results:\n");
        for signal in &ctx.gathered.prechecks {
            out.push_str(&format!(
                "  - {} [{:?}] {}\n",
                signal.tool, signal.status, signal.summary
            ));
        }
    }
    out
}

fn render_final_analysis_markdown(
    issues: &[CanonicalIssue],
    action_plan: &FinalActionPlan,
    agent_failures: &[AgentFailure],
) -> String {
    let mut out = String::from("## Final Analysis\n\n");
    let critical = issues
        .iter()
        .filter(|i| i.severity == Severity::Critical)
        .count();
    let warning = issues
        .iter()
        .filter(|i| i.severity == Severity::Warning)
        .count();
    let info = issues
        .iter()
        .filter(|i| i.severity == Severity::Info)
        .count();
    out.push_str(&format!(
        "- Issues: {} (Critical: {}, Warning: {}, Info: {})\n",
        issues.len(),
        critical,
        warning,
        info
    ));
    out.push_str(&format!("- Agent failures: {}\n", agent_failures.len()));
    if !agent_failures.is_empty() {
        out.push_str("\n### Agent Execution Issues\n");
        for failure in agent_failures {
            let round = failure
                .round
                .map(|value| format!(" round {}", value))
                .unwrap_or_default();
            out.push_str(&format!(
                "- {} [{}{}]: {}\n",
                failure.agent, failure.stage, round, failure.message
            ));
        }
    }
    out.push_str("\n### Top Issues\n");
    if issues.is_empty() {
        out.push_str("- No issues found.\n");
    } else {
        for issue in issues.iter().take(10) {
            let loc = match (&issue.file, issue.line_start) {
                (Some(file), Some(line)) => format!("{}:{}", file.display(), line),
                (Some(file), None) => file.display().to_string(),
                _ => "<unknown>".to_string(),
            };
            out.push_str(&format!(
                "- [{:?}] {} `{}`\n",
                issue.severity, issue.title, loc
            ));
        }
    }
    out.push_str("\n### Action Plan\n");
    if action_plan.prioritized_steps.is_empty() {
        out.push_str("- No blocking actions.\n");
    } else {
        for step in &action_plan.prioritized_steps {
            out.push_str(&format!("- {}\n", step));
        }
    }
    out
}

fn fallback_focus(ctx: &ReviewContext, reason: &str) -> FocusAreas {
    let mut focus_items = Vec::new();
    for file in ctx.changed_files.iter().take(5) {
        focus_items.push(crate::analysis::FocusItem {
            area: file.display().to_string(),
            rationale: "Changed file included via analyzer fallback.".to_string(),
        });
    }
    let mut reviewer_checklist = Vec::new();
    if !ctx.gathered.prechecks.is_empty() {
        reviewer_checklist.push("Validate deterministic precheck failures or warnings.".to_string());
    }
    reviewer_checklist.push("Review changed files for correctness, regressions, and missing tests.".to_string());
    FocusAreas {
        summary: format!(
            "Analyzer fallback in use because the analyzer failed: {}",
            reason
        ),
        focus_items,
        trade_offs: vec!["Analyzer-generated prioritization was unavailable for this run.".to_string()],
        affected_modules: ctx
            .changed_files
            .iter()
            .take(10)
            .map(|path| path.display().to_string())
            .collect(),
        call_chain: Vec::new(),
        design_patterns: Vec::new(),
        reviewer_checklist,
    }
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

async fn run_agent_for_round(
    agent: &dyn crate::plugin::AgentPlugin,
    agent_ctx: &crate::analysis::AgentContext,
    round: u8,
    diff_chunks: &[String],
    prior_round_findings: &HashMap<String, Vec<RawFinding>>,
    timeout_secs: u64,
) -> Result<AgentReviewOutput> {
    let mut all_findings = Vec::new();
    let mut narratives = Vec::new();
    for chunk in diff_chunks {
        let mut chunk_ctx = agent_ctx.clone();
        chunk_ctx.diff = chunk.clone();
        let output = if round == 1 {
            tokio::time::timeout(
                Duration::from_secs(timeout_secs.max(1)),
                agent.analyze(&chunk_ctx),
            )
            .await
            .map_err(|_| anyhow!("agent round-1 review timed out after {}s", timeout_secs))??
        } else {
            let synthesis = CrossPollinationSynthesis {
                summary: format_round_synthesis(round - 1, prior_round_findings),
            };
            tokio::time::timeout(
                Duration::from_secs(timeout_secs.max(1)),
                agent.debate(&chunk_ctx, round, &synthesis),
            )
            .await
            .map_err(|_| anyhow!("agent debate timed out after {}s", timeout_secs))??
        };
        if !output.narrative.trim().is_empty() {
            narratives.push(output.narrative.trim().to_string());
        }
        all_findings.extend(output.findings);
    }
    Ok(AgentReviewOutput {
        findings: all_findings,
        narrative: narratives.join(" | "),
    })
}

fn chunk_diff(diff: &str, max_lines_per_chunk: usize, max_chunks: usize) -> Vec<String> {
    let max_lines_per_chunk = max_lines_per_chunk.max(200);
    let max_chunks = max_chunks.max(1);
    if diff.lines().count() <= max_lines_per_chunk {
        return vec![diff.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_lines = 0usize;

    for line in diff.lines() {
        if line.starts_with("diff --git ")
            && current_lines >= max_lines_per_chunk
            && chunks.len() + 1 < max_chunks
        {
            chunks.push(current.clone());
            current.clear();
            current_lines = 0;
        }
        current.push_str(line);
        current.push('\n');
        current_lines += 1;
    }
    if chunks.len() < max_chunks && !current.trim().is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        vec![diff.to_string()]
    } else {
        chunks
    }
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
    use super::{calibrate_low_confidence, chunk_diff, parse_convergence_verdict};
    use crate::progress::ConvergenceVerdict;
    use crate::report::{Confidence, Finding, Severity};

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

    #[test]
    fn chunking_splits_large_diff() {
        let diff = "diff --git a/a.rs b/a.rs\n@@\n".to_string()
            + &"+x\n".repeat(600)
            + "diff --git a/b.rs b/b.rs\n@@\n"
            + &"+y\n".repeat(700);
        let chunks = chunk_diff(&diff, 500, 4);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().any(|chunk| chunk.contains("diff --git a/b.rs b/b.rs")));
    }

    #[test]
    fn chunking_keeps_remainder_in_last_chunk_when_chunk_limit_hit() {
        let diff = "diff --git a/a.rs b/a.rs\n@@\n".to_string()
            + &"+x\n".repeat(600)
            + "diff --git a/b.rs b/b.rs\n@@\n"
            + &"+y\n".repeat(600)
            + "diff --git a/c.rs b/c.rs\n@@\n"
            + &"+z\n".repeat(600);
        let chunks = chunk_diff(&diff, 500, 2);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].contains("diff --git a/b.rs b/b.rs"));
        assert!(chunks[1].contains("diff --git a/c.rs b/c.rs"));
    }

    #[test]
    fn low_confidence_singleton_is_downgraded() {
        let mut findings = vec![Finding {
            agent: "claude-code".to_string(),
            severity: Severity::Warning,
            confidence: Confidence::Low,
            file: None,
            span: None,
            message: "possible issue".to_string(),
            category: None,
            title: None,
            description: None,
            suggested_fix: None,
            evidence: Vec::new(),
            round: 1,
            raised_by: vec!["claude-code".to_string()],
        }];
        calibrate_low_confidence(&mut findings);
        assert_eq!(findings[0].severity, Severity::Info);
    }
}

fn summarize_agent_output(
    output: &AgentReviewOutput,
    using_chunking: bool,
) -> (String, Vec<crate::progress::AgentFindingPreview>, String) {
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
    if using_chunking {
        summary = format!("{summary} | diff chunking enabled");
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

    let mut details = String::new();
    let narrative = output.narrative.trim();
    if !narrative.is_empty() {
        details.push_str("Narrative:\n");
        details.push_str(narrative);
        details.push_str("\n\n");
    }
    if output.findings.is_empty() {
        details.push_str("Findings: none");
    } else {
        details.push_str("Findings:\n");
        for finding in &output.findings {
            let loc = match (&finding.file, finding.line_start, finding.line_end) {
                (Some(file), Some(start), Some(end)) if start != end => {
                    format!("{}:{}-{}", file.display(), start, end)
                }
                (Some(file), Some(start), _) => format!("{}:{}", file.display(), start),
                (Some(file), None, _) => file.display().to_string(),
                _ => "<unknown>".to_string(),
            };
            details.push_str(&format!(
                "- [{:?}] {} {}\n",
                finding.severity, loc, finding.message
            ));
            if let Some(description) = &finding.description {
                if !description.trim().is_empty() {
                    details.push_str(&format!("  description: {}\n", description.trim()));
                }
            }
        }
    }

    (summary, highlights, details)
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

    pub fn ingest_round(&mut self, raw: &[RawFinding], round: u8, agent_id: &str, diff: &str) {
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
            let evidence = if rf.evidence.is_empty() {
                fallback_evidence_from_diff(diff, &rf.file, rf.line_start)
            } else {
                rf.evidence.clone()
            };
            let finding = Finding {
                agent: agent_id.to_string(),
                severity: rf.severity.clone(),
                confidence: rf.confidence.clone(),
                file: rf.file.clone(),
                span,
                message: rf.message.clone(),
                category: rf.category.clone(),
                title: rf.title.clone(),
                description: rf.description.clone(),
                suggested_fix: rf.suggested_fix.clone(),
                evidence,
                round,
                raised_by: vec![agent_id.to_string()],
            };

            if let Some(key) = key {
                let mut merged_key = None;
                for existing in self.clusters.keys() {
                    if same_cluster(existing, &key)
                        || (existing.file == key.file
                            && message_similar(
                                &self.clusters[existing].findings[0].message,
                                &finding.message,
                            ))
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
            .map(|state| dedup_cluster(state))
            .collect::<Vec<_>>();
        all.extend(self.loose.clone());
        calibrate_low_confidence(&mut all);
        all
    }
}

fn same_cluster(a: &FindingKey, b: &FindingKey) -> bool {
    a.file == b.file && spans_overlap(&a.span, &b.span)
}

fn dedup_cluster(state: &ClusterState) -> Finding {
    let mut findings = state.findings.clone();
    findings.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(confidence_rank(&b.confidence).cmp(&confidence_rank(&a.confidence)))
            .then(a.round.cmp(&b.round))
    });
    let mut finding = findings
        .into_iter()
        .next()
        .expect("cluster has at least one finding");
    let mut raised_by = state.agents.iter().cloned().collect::<Vec<_>>();
    raised_by.sort();
    finding.raised_by = raised_by;
    finding.severity = state.severity.clone();
    finding
}

fn confidence_rank(confidence: &crate::report::Confidence) -> u8 {
    match confidence {
        crate::report::Confidence::Low => 0,
        crate::report::Confidence::Medium => 1,
        crate::report::Confidence::High => 2,
    }
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

fn fallback_evidence_from_diff(
    diff: &str,
    file: &Option<std::path::PathBuf>,
    line_start: Option<u32>,
) -> Vec<EvidenceAnchor> {
    let (Some(file), Some(line)) = (file.as_ref(), line_start) else {
        return Vec::new();
    };
    let mut capture = None;
    let target = format!("+++ b/{}", file.display());
    for raw in diff.lines() {
        if raw == target {
            capture = Some(String::new());
            continue;
        }
        if raw.starts_with("diff --git ") {
            capture = None;
            continue;
        }
        if let Some(buf) = capture.as_mut() {
            if raw.starts_with('+') || raw.starts_with('-') || raw.starts_with(' ') {
                if !raw.starts_with("+++") && !raw.starts_with("---") {
                    buf.push_str(raw);
                    if buf.len() >= 180 {
                        break;
                    }
                }
            }
        }
    }
    let quote = capture
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if quote.is_empty() {
        return Vec::new();
    }
    vec![EvidenceAnchor {
        location: format!("{}:{}", file.display(), line),
        quote,
    }]
}

fn calibrate_low_confidence(findings: &mut [Finding]) {
    let mut counts: HashMap<(Option<std::path::PathBuf>, Option<u32>, String), usize> =
        HashMap::new();
    for finding in findings.iter() {
        let key = (
            finding.file.clone(),
            finding.span.as_ref().map(|s| s.start),
            finding.message.to_lowercase(),
        );
        *counts.entry(key).or_insert(0) += 1;
    }
    for finding in findings.iter_mut() {
        let key = (
            finding.file.clone(),
            finding.span.as_ref().map(|s| s.start),
            finding.message.to_lowercase(),
        );
        let count = counts.get(&key).copied().unwrap_or(0);
        if count <= 1 && finding.confidence == crate::report::Confidence::Low {
            finding.severity = match finding.severity {
                Severity::Critical => Severity::Warning,
                Severity::Warning => Severity::Info,
                Severity::Info => Severity::Info,
            };
        }
    }
}

fn build_canonical_issues(findings: &[Finding]) -> Vec<CanonicalIssue> {
    let mut grouped: BTreeMap<(Option<std::path::PathBuf>, Option<u32>, String), Vec<&Finding>> =
        BTreeMap::new();
    for finding in findings {
        let key = (
            finding.file.clone(),
            finding.span.as_ref().map(|s| s.start),
            normalize_text(finding.title.as_ref().unwrap_or(&finding.message).as_str()),
        );
        grouped.entry(key).or_default().push(finding);
    }

    let mut issues = Vec::new();
    for (_key, group) in grouped {
        if group.is_empty() {
            continue;
        }
        let primary = group[0];
        let mut raised_by = group.iter().map(|f| f.agent.clone()).collect::<Vec<_>>();
        raised_by.sort();
        raised_by.dedup();
        let mut evidence = Vec::new();
        for f in &group {
            evidence.extend(f.evidence.clone());
        }
        evidence.truncate(3);
        let severity = group
            .iter()
            .map(|f| f.severity.clone())
            .max()
            .unwrap_or(Severity::Info);
        let category = primary
            .category
            .clone()
            .unwrap_or_else(|| "maintainability".to_string());
        let title = primary
            .title
            .clone()
            .unwrap_or_else(|| primary.message.clone());
        let description = primary
            .description
            .clone()
            .unwrap_or_else(|| primary.message.clone());
        issues.push(CanonicalIssue {
            severity,
            category,
            file: primary.file.clone(),
            line_start: primary.span.as_ref().map(|s| s.start),
            line_end: primary.span.as_ref().map(|s| s.end),
            title,
            description,
            suggested_fix: primary.suggested_fix.clone(),
            raised_by,
            evidence,
        });
    }
    issues.sort_by(|a, b| {
        severity_rank(&b.severity)
            .cmp(&severity_rank(&a.severity))
            .then(a.file.cmp(&b.file))
            .then(a.line_start.cmp(&b.line_start))
            .then(a.title.cmp(&b.title))
    });
    issues
}

fn merge_structured_issues(
    mut baseline: Vec<CanonicalIssue>,
    structured: Vec<CanonicalIssue>,
) -> Vec<CanonicalIssue> {
    let mut seen = baseline
        .iter()
        .map(|issue| {
            (
                issue.file.clone(),
                issue.line_start,
                normalize_text(&issue.title),
            )
        })
        .collect::<HashSet<_>>();
    for issue in structured {
        let key = (
            issue.file.clone(),
            issue.line_start,
            normalize_text(&issue.title),
        );
        if seen.insert(key) {
            baseline.push(issue);
        }
    }
    baseline.sort_by(|a, b| {
        severity_rank(&b.severity)
            .cmp(&severity_rank(&a.severity))
            .then(a.file.cmp(&b.file))
            .then(a.line_start.cmp(&b.line_start))
    });
    baseline
}

fn normalize_text(input: &str) -> String {
    input
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

fn build_action_plan(issues: &[CanonicalIssue]) -> FinalActionPlan {
    let mut prioritized_steps = Vec::new();
    let mut quick_wins = Vec::new();
    for issue in issues {
        let loc = match (&issue.file, issue.line_start) {
            (Some(file), Some(line)) => format!("{}:{}", file.display(), line),
            (Some(file), None) => file.display().to_string(),
            _ => "<unknown>".to_string(),
        };
        let step = format!("[{:?}] {} ({loc})", issue.severity, issue.title);
        if issue.severity == Severity::Critical || issue.severity == Severity::Warning {
            prioritized_steps.push(step);
        } else {
            quick_wins.push(step);
        }
        if prioritized_steps.len() >= 5 && quick_wins.len() >= 5 {
            break;
        }
    }
    FinalActionPlan {
        prioritized_steps,
        quick_wins,
    }
}

fn render_pr_comment(issues: &[CanonicalIssue], plan: &FinalActionPlan) -> String {
    let mut out = String::from("## Crucible Review Summary\n\n");
    out.push_str(&format!("- Issues: {}\n", issues.len()));
    let critical = issues
        .iter()
        .filter(|i| i.severity == Severity::Critical)
        .count();
    let warning = issues
        .iter()
        .filter(|i| i.severity == Severity::Warning)
        .count();
    out.push_str(&format!("- Critical: {critical}, Warning: {warning}\n\n"));
    out.push_str("### Top Actions\n");
    if plan.prioritized_steps.is_empty() {
        out.push_str("- No blocking actions.\n");
    } else {
        for step in &plan.prioritized_steps {
            out.push_str(&format!("- {step}\n"));
        }
    }
    out.push_str("\n### Notable Issues\n");
    for issue in issues.iter().take(10) {
        let location = match (&issue.file, issue.line_start) {
            (Some(file), Some(line)) => format!("{}:{}", file.display(), line),
            (Some(file), None) => file.display().to_string(),
            _ => "<unknown>".to_string(),
        };
        out.push_str(&format!(
            "- **{}** `{}`: {}\n",
            issue.title, location, issue.description
        ));
    }
    out
}

#[derive(Debug, Clone)]
pub struct DebateTranscript;

#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub focus: FocusAreas,
    pub findings: Vec<Finding>,
    pub auto_fix: Option<crate::report::AutoFix>,
}
