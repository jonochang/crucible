use crate::analysis::FocusAreas;
use crate::config::CrucibleConfig;
use crate::context::ReviewContext;
use crate::plugin::PluginRegistry;
use crate::report::{ConsensusMap, ConsensusStatus, Finding, FindingKey, LineSpan, RawFinding, Severity};
use anyhow::Result;
use std::collections::{HashMap, HashSet};

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
        let consensus = ConsensusTracker::new(cfg.coordinator.quorum_threshold, cfg.plugins.agents.len());
        Self { registry, cfg, snapshotter: MessageSnapshotter::default(), consensus, progress }
    }

    pub async fn run(&mut self, ctx: &ReviewContext) -> Result<crate::report::ReviewReport> {
        self.emit(crate::progress::ProgressEvent::AnalyzerStart);
        let focus = self
            .registry
            .analyzer
            .analyze_focus(&ctx.into_agent_ctx(None))
            .await?;
        self.emit(crate::progress::ProgressEvent::AnalyzerDone);

        self.snapshotter.freeze_round(1, &HashMap::new());

        let round = 1;
        let agents = self.registry.agents.iter().map(|a| a.id().to_string()).collect();
        self.emit(crate::progress::ProgressEvent::RoundStart { round, agents });
        let agent_ctx = ctx.into_agent_ctx(Some(&focus));
        for agent in &self.registry.agents {
            let id = agent.id();
            self.emit(crate::progress::ProgressEvent::AgentStart { round, id: id.to_string() });
            let raw = match agent.analyze(&agent_ctx).await {
                Ok(raw) => raw,
                Err(err) => {
                    self.emit(crate::progress::ProgressEvent::AgentError {
                        round,
                        id: id.to_string(),
                        message: err.to_string(),
                    });
                    return Err(err);
                }
            };
            self.consensus.ingest_round(&raw, round, id);
            self.emit(crate::progress::ProgressEvent::AgentDone { round, id: id.to_string() });
        }
        let findings = self.consensus.all_findings();
        self.emit(crate::progress::ProgressEvent::RoundDone { round });

        let auto_fix = if findings.iter().any(|f| f.severity >= Severity::Warning) {
            Some(self.registry.judge.summarize(&agent_ctx, &findings).await?)
        } else {
            None
        };
        if auto_fix.is_some() {
            self.emit(crate::progress::ProgressEvent::AutoFixReady);
        }

        let report = crate::report::ReviewReport::from_findings(
            &findings,
            &self.cfg.verdict,
            self.consensus.consensus_map(),
            auto_fix,
        );
        self.emit(crate::progress::ProgressEvent::Completed(report.clone()));
        Ok(report)
    }

    fn emit(&self, event: crate::progress::ProgressEvent) {
        if let Some(tx) = &self.progress {
            let _ = tx.send(event);
        }
    }
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
                        || message_similar(&self.clusters[existing].findings[0].message, &finding.message)
                    {
                        merged_key = Some(existing.clone());
                        break;
                    }
                }

                let entry_key = merged_key.unwrap_or_else(|| key.clone());
                self.clusters
                    .entry(entry_key)
                    .and_modify(|state| {
                        state.findings.push(finding.clone());
                        state.agents.insert(agent_id.to_string());
                        if finding.severity > state.severity {
                            state.severity = finding.severity.clone();
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
                self.loose.push(finding);
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
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let tokens_b: HashSet<String> = b
        .split_whitespace()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
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
