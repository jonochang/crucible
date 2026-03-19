use crate::config::CrucibleConfig;
use crate::context::docs::DocsCollector;
use crate::context::history::HistoryCollector;
use crate::context::precheck::collect_precheck_signals;
use crate::plugin::{
    ConvergenceDecision, GenericAgentOutput, GenericFinalOutput, PluginRegistry, RawConsensusItem,
};
use crate::progress::ConvergenceVerdict;
use crate::task_pack::{AttachmentKind, ContextProvider, TaskPack, load_task_pack};
use anyhow::{Context, Result, anyhow};
use git2::Repository;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusTaskRequest {
    pub pack_id: String,
    pub prompt: String,
    #[serde(default)]
    pub attachments: Vec<TaskAttachment>,
    #[serde(default)]
    pub task_paths: Vec<PathBuf>,
    #[serde(default)]
    pub clarification_history: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAttachment {
    pub id: String,
    pub kind: AttachmentKind,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub inline: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAttachmentContent {
    pub id: String,
    pub kind: AttachmentKind,
    #[serde(default)]
    pub path: Option<PathBuf>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ItemImportance {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusAnchor {
    pub attachment_id: String,
    pub quote: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusItem {
    pub agent: String,
    pub kind: String,
    pub importance: ItemImportance,
    pub title: String,
    pub message: String,
    pub confidence: crate::report::Confidence,
    #[serde(default)]
    pub anchors: Vec<ConsensusAnchor>,
    pub round: u8,
    pub raised_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusReport {
    pub run_id: Uuid,
    pub pack_id: String,
    pub title: String,
    pub summary_markdown: String,
    pub result_json: Value,
    #[serde(default)]
    pub agreed_items: Vec<ConsensusItem>,
    #[serde(default)]
    pub unresolved_items: Vec<ConsensusItem>,
    #[serde(default)]
    pub clarification_requests: Vec<String>,
    #[serde(default)]
    pub agent_failures: Vec<crate::report::AgentFailure>,
    pub session_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub prompt: String,
    pub pack: TaskPack,
    pub attachments: Vec<TaskAttachmentContent>,
    #[serde(default)]
    pub docs: Vec<crate::context::docs::DocSnippet>,
    #[serde(default)]
    pub history: Vec<crate::context::history::CommitSummary>,
    #[serde(default)]
    pub prechecks: Vec<crate::context::precheck::PrecheckSignal>,
    #[serde(default)]
    pub clarification_history: Vec<String>,
    #[serde(default)]
    pub analyzer_summary: Option<String>,
}

pub async fn run_consensus(
    cfg: &CrucibleConfig,
    request: ConsensusTaskRequest,
    run_id: Uuid,
) -> Result<ConsensusReport> {
    let cwd = std::env::current_dir()?;
    let request_snapshot = request.clone();
    let pack = load_task_pack(cfg, Some(&cwd), &request.pack_id, &request.task_paths)?;
    let ctx = build_task_context(cfg, pack.clone(), request).await?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut engine = ConsensusEngine::new(cfg, registry, run_id, pack);
    engine.request_snapshot = Some(request_snapshot);
    let report = engine.run(ctx).await?;
    persist_session(&cwd, &report, &engine.request_snapshot)?;
    Ok(report)
}

async fn build_task_context(
    cfg: &CrucibleConfig,
    pack: TaskPack,
    request: ConsensusTaskRequest,
) -> Result<TaskContext> {
    let repo_root = std::env::current_dir().ok();
    let mut attachments = Vec::new();
    for attachment in request.attachments {
        pack.validate_attachment_kind(&attachment.kind)?;
        let content = match (&attachment.path, &attachment.inline) {
            (Some(path), _) => fs::read_to_string(path)
                .with_context(|| format!("read attachment {}", path.display()))?,
            (_, Some(inline)) => inline.clone(),
            _ => return Err(anyhow!("attachment '{}' must include path or inline", attachment.id)),
        };
        attachments.push(TaskAttachmentContent {
            id: attachment.id,
            kind: attachment.kind,
            path: attachment.path,
            content,
        });
    }

    let mut docs = Vec::new();
    let mut history = Vec::new();
    let mut prechecks = Vec::new();

    if let Some(repo_root) = repo_root {
        if pack
            .manifest
            .context_providers
            .contains(&ContextProvider::RepoDocs)
        {
            docs = DocsCollector::new(
                cfg.context.docs_patterns.clone(),
                cfg.context.docs_max_bytes,
            )
            .collect(&repo_root)
            .unwrap_or_default();
        }
        if pack
            .manifest
            .context_providers
            .contains(&ContextProvider::GitHistory)
        {
            if let Ok(repo) = Repository::discover(&repo_root) {
                history = HistoryCollector::new(
                    cfg.context.history_max_commits,
                    cfg.context.history_max_days,
                )
                .collect(&Vec::new(), &repo)
                .unwrap_or_default();
            }
        }
        if pack
            .manifest
            .context_providers
            .contains(&ContextProvider::Prechecks)
        {
            prechecks = collect_precheck_signals(&repo_root, cfg).unwrap_or_default();
        }
    }

    Ok(TaskContext {
        prompt: request.prompt,
        pack,
        attachments,
        docs,
        history,
        prechecks,
        clarification_history: request.clarification_history,
        analyzer_summary: None,
    })
}

struct ConsensusEngine<'a> {
    registry: PluginRegistry,
    run_id: Uuid,
    pack: TaskPack,
    request_snapshot: Option<ConsensusTaskRequest>,
    _cfg: &'a CrucibleConfig,
}

impl<'a> ConsensusEngine<'a> {
    fn new(
        cfg: &'a CrucibleConfig,
        registry: PluginRegistry,
        run_id: Uuid,
        pack: TaskPack,
    ) -> Self {
        Self {
            registry,
            run_id,
            pack,
            request_snapshot: None,
            _cfg: cfg,
        }
    }

    async fn run(&mut self, mut ctx: TaskContext) -> Result<ConsensusReport> {
        ctx.analyzer_summary = self
            .registry
            .analyzer
            .analyze_task_focus(&ctx)
            .await
            .ok()
            .map(|focus| focus.summary);

        let total_rounds = self.pack.manifest.rounds.max(1);
        let mut tracker = ConsensusTracker::new(self.pack.manifest.quorum, self.registry.agents.len());
        let mut prior_summary = String::new();
        let mut failures = Vec::new();

        for round in 1..=total_rounds {
            for agent in &self.registry.agents {
                let output = if round == 1 {
                    agent.analyze_task(&ctx, &self.pack).await
                } else {
                    agent.debate_task(&ctx, &self.pack, round, &prior_summary).await
                };
                match output {
                    Ok(output) => tracker.ingest(round, agent.id(), output),
                    Err(err) => failures.push(crate::report::AgentFailure {
                        agent: agent.id().to_string(),
                        stage: "consensus-round".to_string(),
                        round: Some(round),
                        message: err.to_string(),
                    }),
                }
            }

            let all_items = tracker.all_items();
            if round < total_rounds {
                let decision = self
                    .registry
                    .judge
                    .judge_task_convergence(&ctx, round, &all_items)
                    .await
                    .unwrap_or(ConvergenceDecision {
                        verdict: ConvergenceVerdict::NotConverged,
                        rationale: "fallback: continue until final round".to_string(),
                    });
                prior_summary = format!("Round {round}: {}", decision.rationale);
                if decision.verdict == ConvergenceVerdict::Converged {
                    break;
                }
            }
        }

        let all_items = tracker.all_items();
        let unresolved_items = tracker.unresolved_items();
        let final_output = self
            .registry
            .judge
            .summarize_task(&ctx, &self.pack, &all_items, &unresolved_items)
            .await
            .unwrap_or_else(|_| GenericFinalOutput {
                summary_markdown: fallback_summary(&self.pack, &all_items, &unresolved_items),
                result_json: serde_json::json!({
                    "pack_id": self.pack.id(),
                    "agreed_items": all_items.iter().map(|item| item.title.clone()).collect::<Vec<_>>(),
                    "unresolved_items": unresolved_items.iter().map(|item| item.title.clone()).collect::<Vec<_>>()
                }),
                clarification_requests: Vec::new(),
            });

        Ok(ConsensusReport {
            run_id: self.run_id,
            pack_id: self.pack.id().to_string(),
            title: self.pack.manifest.title.clone(),
            summary_markdown: final_output.summary_markdown,
            result_json: final_output.result_json,
            agreed_items: all_items,
            unresolved_items,
            clarification_requests: final_output.clarification_requests,
            agent_failures: failures,
            session_id: self.run_id,
        })
    }
}

fn fallback_summary(
    pack: &TaskPack,
    agreed_items: &[ConsensusItem],
    unresolved_items: &[ConsensusItem],
) -> String {
    let mut out = format!("# {}\n\n", pack.manifest.title);
    out.push_str("## Agreed Items\n");
    if agreed_items.is_empty() {
        out.push_str("- None\n");
    } else {
        for item in agreed_items.iter().take(10) {
            out.push_str(&format!("- {}: {}\n", item.title, item.message));
        }
    }
    out.push_str("\n## Unresolved Items\n");
    if unresolved_items.is_empty() {
        out.push_str("- None\n");
    } else {
        for item in unresolved_items.iter().take(10) {
            out.push_str(&format!("- {}: {}\n", item.title, item.message));
        }
    }
    out
}

fn persist_session(
    cwd: &Path,
    report: &ConsensusReport,
    request: &Option<ConsensusTaskRequest>,
) -> Result<()> {
    let dir = cwd.join(".crucible/sessions").join(report.session_id.to_string());
    fs::create_dir_all(&dir)?;
    fs::write(
        dir.join("report.json"),
        serde_json::to_vec_pretty(report).context("serialize consensus report")?,
    )?;
    if let Some(request) = request {
        fs::write(
            dir.join("request.json"),
            serde_json::to_vec_pretty(request).context("serialize consensus request")?,
        )?;
    }
    Ok(())
}

#[derive(Debug)]
struct ConsensusTracker {
    quorum: f32,
    agents: usize,
    groups: HashMap<String, ItemGroup>,
}

#[derive(Debug)]
struct ItemGroup {
    items: Vec<ConsensusItem>,
    agents: HashSet<String>,
}

impl ConsensusTracker {
    fn new(quorum: f32, agents: usize) -> Self {
        Self {
            quorum,
            agents: agents.max(1),
            groups: HashMap::new(),
        }
    }

    fn ingest(&mut self, round: u8, agent_id: &str, output: GenericAgentOutput) {
        for raw in output.items {
            let key = normalize_item_key(&raw);
            let item = ConsensusItem {
                agent: agent_id.to_string(),
                kind: raw.kind,
                importance: raw.importance,
                title: raw.title,
                message: raw.message,
                confidence: raw.confidence,
                anchors: raw.anchors,
                round,
                raised_by: vec![agent_id.to_string()],
            };
            self.groups
                .entry(key)
                .and_modify(|group| {
                    if group.agents.insert(agent_id.to_string()) {
                        group.items.push(item.clone());
                    }
                })
                .or_insert_with(|| {
                    let mut agents = HashSet::new();
                    agents.insert(agent_id.to_string());
                    ItemGroup {
                        items: vec![item],
                        agents,
                    }
                });
        }
    }

    fn all_items(&self) -> Vec<ConsensusItem> {
        self.groups
            .values()
            .filter_map(dedup_group)
            .collect::<Vec<_>>()
    }

    fn unresolved_items(&self) -> Vec<ConsensusItem> {
        self.groups
            .values()
            .filter(|group| (group.agents.len() as f32) / (self.agents as f32) < self.quorum)
            .filter_map(dedup_group)
            .collect()
    }
}

fn dedup_group(group: &ItemGroup) -> Option<ConsensusItem> {
    let mut items = group.items.clone();
    items.sort_by(|a, b| {
        importance_rank(&b.importance)
            .cmp(&importance_rank(&a.importance))
            .then(a.title.cmp(&b.title))
    });
    let mut primary = items.into_iter().next()?;
    let mut raised_by = group.agents.iter().cloned().collect::<Vec<_>>();
    raised_by.sort();
    primary.raised_by = raised_by;
    Some(primary)
}

fn importance_rank(value: &ItemImportance) -> u8 {
    match value {
        ItemImportance::High => 3,
        ItemImportance::Medium => 2,
        ItemImportance::Low => 1,
    }
}

fn normalize_item_key(raw: &RawConsensusItem) -> String {
    format!(
        "{}:{}:{}",
        raw.kind.to_lowercase(),
        raw.title.to_lowercase(),
        raw.message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    )
}
