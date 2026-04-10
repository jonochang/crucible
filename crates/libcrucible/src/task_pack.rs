use crate::config::CrucibleConfig;
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Markdown,
    Text,
    SourceFile,
    Diff,
}

impl AttachmentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AttachmentKind::Markdown => "markdown",
            AttachmentKind::Text => "text",
            AttachmentKind::SourceFile => "source_file",
            AttachmentKind::Diff => "diff",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextProvider {
    PromptOnly,
    RepoDocs,
    SelectedFiles,
    GitDiff,
    GitHistory,
    Prechecks,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPackRole {
    pub id: String,
    pub name: String,
    pub persona: String,
    pub focus: String,
    #[serde(default)]
    pub prompt_template: PromptTemplate,
    #[serde(default = "default_role_weight")]
    pub default_weight: f32,
}

fn default_role_weight() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptTemplate {
    #[default]
    Discover,
    Challenge,
    Verify,
    Judge,
    Analyze,
    Convergence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoundMode {
    #[default]
    Discover,
    Challenge,
    Verify,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub role: String,
    pub plugin: String,
    #[serde(default)]
    pub weight_override: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRound {
    pub name: String,
    #[serde(default)]
    pub mode: RoundMode,
    pub assignments: Vec<TaskAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFinalization {
    #[serde(default)]
    pub analyze: Option<TaskAssignment>,
    pub judge: TaskAssignment,
    #[serde(default)]
    pub convergence: Option<TaskAssignment>,
    #[serde(default)]
    pub structurizer: Option<TaskAssignment>,
    #[serde(default)]
    pub autofix: Option<TaskAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPackManifest {
    pub id: String,
    pub version: String,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub allowed_attachment_kinds: Vec<AttachmentKind>,
    #[serde(default)]
    pub context_providers: Vec<ContextProvider>,
    #[serde(default)]
    pub roles: Vec<TaskPackRole>,
    pub rounds: Vec<TaskRound>,
    #[serde(default = "default_quorum")]
    pub quorum: f32,
    #[serde(default)]
    pub allow_clarifications: bool,
    pub finalization: TaskFinalization,
}

fn default_quorum() -> f32 {
    0.75
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPack {
    pub manifest: TaskPackManifest,
    pub analyzer_prompt: String,
    pub reviewer_prompt: String,
    pub judge_prompt: String,
    pub schema_json: String,
    #[serde(default)]
    pub render_prompt: Option<String>,
    #[serde(default)]
    pub source: TaskPackSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum TaskPackSource {
    #[default]
    BuiltIn,
    Directory(PathBuf),
}

impl TaskPack {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn validate_attachment_kind(&self, kind: &AttachmentKind) -> Result<()> {
        if self.manifest.allowed_attachment_kinds.is_empty()
            || self.manifest.allowed_attachment_kinds.contains(kind)
        {
            return Ok(());
        }
        Err(anyhow!(
            "task pack '{}' does not allow attachment kind '{}'",
            self.id(),
            kind.as_str()
        ))
    }

    pub fn validate_schema(&self) -> Result<()> {
        let role_ids = self
            .manifest
            .roles
            .iter()
            .map(|role| role.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        if role_ids.len() != self.manifest.roles.len() {
            return Err(anyhow!("task pack '{}' has duplicate role ids", self.id()));
        }
        if self.manifest.rounds.is_empty() {
            return Err(anyhow!("task pack '{}' must define at least one round", self.id()));
        }
        for round in &self.manifest.rounds {
            if round.assignments.is_empty() {
                return Err(anyhow!(
                    "task pack '{}' round '{}' must define at least one assignment",
                    self.id(),
                    round.name
                ));
            }
            for assignment in &round.assignments {
                if !role_ids.contains(assignment.role.as_str()) {
                    return Err(anyhow!(
                        "task pack '{}' round '{}' references unknown role '{}'",
                        self.id(),
                        round.name,
                        assignment.role
                    ));
                }
            }
        }
        for assignment in [
            self.manifest.finalization.analyze.as_ref(),
            Some(&self.manifest.finalization.judge),
            self.manifest.finalization.convergence.as_ref(),
            self.manifest.finalization.structurizer.as_ref(),
            self.manifest.finalization.autofix.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !role_ids.contains(assignment.role.as_str()) {
                return Err(anyhow!(
                    "task pack '{}' finalization references unknown role '{}'",
                    self.id(),
                    assignment.role
                ));
            }
        }
        Ok(())
    }
}

pub fn load_task_pack(
    cfg: &CrucibleConfig,
    cwd: Option<&Path>,
    pack_id: &str,
    extra_paths: &[PathBuf],
) -> Result<TaskPack> {
    for base in extra_paths {
        if let Some(pack) = load_pack_from_root(base, pack_id)? {
            return Ok(pack);
        }
    }

    if let Some(cwd) = cwd {
        let repo_root = resolve_repo_root(cwd);
        if let Some(root) = repo_root {
            let repo_dir = root.join(".crucible/tasks");
            if let Some(pack) = load_pack_from_root(&repo_dir, pack_id)? {
                return Ok(pack);
            }
        }
    }

    for base in cfg.task_packs.paths.iter().map(PathBuf::from) {
        if let Some(pack) = load_pack_from_root(&base, pack_id)? {
            return Ok(pack);
        }
    }

    built_in_packs()
        .into_iter()
        .find(|pack| pack.id() == pack_id)
        .ok_or_else(|| anyhow!("task pack '{}' not found", pack_id))
}

pub fn list_task_packs(
    cfg: &CrucibleConfig,
    cwd: Option<&Path>,
    extra_paths: &[PathBuf],
) -> Result<Vec<TaskPack>> {
    let mut packs = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for base in extra_paths {
        load_all_from_root(base, &mut packs, &mut seen)?;
    }
    if let Some(cwd) = cwd.and_then(resolve_repo_root) {
        load_all_from_root(&cwd.join(".crucible/tasks"), &mut packs, &mut seen)?;
    }
    for base in cfg.task_packs.paths.iter().map(PathBuf::from) {
        load_all_from_root(&base, &mut packs, &mut seen)?;
    }
    for pack in built_in_packs() {
        if seen.insert(pack.id().to_string()) {
            packs.push(pack);
        }
    }
    packs.sort_by(|a, b| a.id().cmp(b.id()));
    Ok(packs)
}

fn resolve_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").exists() || dir.join(".crucible.toml").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn load_all_from_root(
    root: &Path,
    packs: &mut Vec<TaskPack>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(pack) = load_pack_from_dir(&path)? {
            if seen.insert(pack.id().to_string()) {
                packs.push(pack);
            }
        }
    }
    Ok(())
}

fn load_pack_from_root(root: &Path, pack_id: &str) -> Result<Option<TaskPack>> {
    let candidate = root.join(pack_id);
    if candidate.exists() {
        return load_pack_from_dir(&candidate);
    }
    if root.ends_with(pack_id) {
        return load_pack_from_dir(root);
    }
    Ok(None)
}

fn load_pack_from_dir(dir: &Path) -> Result<Option<TaskPack>> {
    let manifest_path = dir.join("pack.toml");
    if !manifest_path.exists() {
        return Ok(None);
    }
    let manifest_raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: TaskPackManifest =
        toml::from_str(&manifest_raw).context("parse task pack manifest")?;
    let analyzer_prompt = read_optional(dir.join("analyzer.md"))?.unwrap_or_default();
    let reviewer_prompt = read_required(dir.join("reviewer.md"))?;
    let judge_prompt = read_required(dir.join("judge.md"))?;
    let schema_json = read_required(dir.join("schema.json"))?;
    let render_prompt = read_optional(dir.join("render.md"))?;
    let pack = TaskPack {
        manifest,
        analyzer_prompt,
        reviewer_prompt,
        judge_prompt,
        schema_json,
        render_prompt,
        source: TaskPackSource::Directory(dir.to_path_buf()),
    };
    pack.validate_schema()?;
    Ok(Some(pack))
}

fn read_optional(path: PathBuf) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(
        fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?,
    ))
}

fn read_required(path: PathBuf) -> Result<String> {
    fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
}

fn built_in_packs() -> Vec<TaskPack> {
    vec![
        built_in_review_pack(),
        built_in_requirements_pack(),
        built_in_design_pack(),
        built_in_test_plan_pack(),
    ]
}

fn built_in_review_pack() -> TaskPack {
    let pack = TaskPack {
        manifest: TaskPackManifest {
            id: "review".to_string(),
            version: "1".to_string(),
            title: "Code Review".to_string(),
            description: "Build multi-agent consensus on code review findings.".to_string(),
            allowed_attachment_kinds: vec![AttachmentKind::Diff, AttachmentKind::SourceFile],
            context_providers: vec![
                ContextProvider::GitDiff,
                ContextProvider::SelectedFiles,
                ContextProvider::RepoDocs,
                ContextProvider::GitHistory,
                ContextProvider::Prechecks,
            ],
            roles: vec![
                TaskPackRole {
                    id: "program-semantics".to_string(),
                    name: "Program Semantics Auditor".to_string(),
                    persona: "Program Semantics Auditor".to_string(),
                    focus: "State transitions, invariants, hidden regressions, control-flow mistakes, concurrency hazards, and edge-case correctness".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.5,
                },
                TaskPackRole {
                    id: "maintainer-review".to_string(),
                    name: "Principal Maintainer".to_string(),
                    persona: "Principal Maintainer".to_string(),
                    focus: "API contracts, refactor safety, maintainability, change surface area, and design coherence".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.5,
                },
                TaskPackRole {
                    id: "security-reliability".to_string(),
                    name: "Security and Reliability Auditor".to_string(),
                    persona: "Security and Reliability Auditor".to_string(),
                    focus: "Trust boundaries, secret handling, data exposure, failure modes, recovery behavior, and operational risk".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "verification-review".to_string(),
                    name: "Verification Reviewer".to_string(),
                    persona: "Verification Reviewer".to_string(),
                    focus: "Validate prior findings, prune weak claims, confirm evidence, and surface duplicates or severity inflation".to_string(),
                    prompt_template: PromptTemplate::Verify,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "contrarian-review".to_string(),
                    name: "Contrarian Systems Reviewer".to_string(),
                    persona: "Contrarian Systems Reviewer".to_string(),
                    focus: "Challenge assumptions, cross-file interactions, integration risk, and unlikely but plausible failure paths".to_string(),
                    prompt_template: PromptTemplate::Challenge,
                    default_weight: 1.25,
                },
                TaskPackRole {
                    id: "review-judge".to_string(),
                    name: "Review Judge".to_string(),
                    persona: "Final Review Judge".to_string(),
                    focus: "Synthesize canonical issues, preserve only supported claims, and produce actionable final guidance".to_string(),
                    prompt_template: PromptTemplate::Judge,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "review-analyzer".to_string(),
                    name: "Review Analyzer".to_string(),
                    persona: "Senior Review Analyst".to_string(),
                    focus: "Summarize what changed, major risk areas, tradeoffs, and reviewer checklist".to_string(),
                    prompt_template: PromptTemplate::Analyze,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "convergence-judge".to_string(),
                    name: "Convergence Judge".to_string(),
                    persona: "Strict Convergence Judge".to_string(),
                    focus: "Determine whether material disagreement or net-new high-severity risk remains".to_string(),
                    prompt_template: PromptTemplate::Convergence,
                    default_weight: 1.0,
                },
            ],
            rounds: vec![
                TaskRound {
                    name: "Initial Review".to_string(),
                    mode: RoundMode::Discover,
                    assignments: vec![
                        TaskAssignment {
                            role: "program-semantics".to_string(),
                            plugin: "opencode-glm".to_string(),
                            weight_override: None,
                        },
                        TaskAssignment {
                            role: "maintainer-review".to_string(),
                            plugin: "codex".to_string(),
                            weight_override: None,
                        },
                        TaskAssignment {
                            role: "security-reliability".to_string(),
                            plugin: "claude-code".to_string(),
                            weight_override: None,
                        },
                    ],
                },
                TaskRound {
                    name: "Challenge and Verify".to_string(),
                    mode: RoundMode::Challenge,
                    assignments: vec![
                        TaskAssignment {
                            role: "contrarian-review".to_string(),
                            plugin: "claude-code".to_string(),
                            weight_override: None,
                        },
                        TaskAssignment {
                            role: "verification-review".to_string(),
                            plugin: "codex".to_string(),
                            weight_override: None,
                        },
                    ],
                },
            ],
            quorum: 0.75,
            allow_clarifications: false,
            finalization: TaskFinalization {
                analyze: Some(TaskAssignment {
                    role: "review-analyzer".to_string(),
                    plugin: "opencode-glm".to_string(),
                    weight_override: None,
                }),
                judge: TaskAssignment {
                    role: "review-judge".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                },
                convergence: Some(TaskAssignment {
                    role: "convergence-judge".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                }),
                structurizer: Some(TaskAssignment {
                    role: "review-judge".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                }),
                autofix: Some(TaskAssignment {
                    role: "review-judge".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                }),
            },
        },
        analyzer_prompt: "You are a senior architect producing analyzer context for code review. Summarize what changed, architectural impact, focus areas, tradeoffs, affected modules, call chain, design patterns, and a reviewer checklist.".to_string(),
        reviewer_prompt: "Perform an exhaustive code review of the changed code. Assess correctness, security, performance, error handling, edge cases, maintainability, and testing gaps. Every finding must include direct evidence with exact code location and short quote snippets.".to_string(),
        judge_prompt: "Produce final review consensus, convergence judgments, canonical issues, and auto-fix guidance for agreed findings.".to_string(),
        schema_json: r#"{
  "type":"object",
  "required":["findings"],
  "properties":{
    "findings":{"type":"array"}
  }
}"#.to_string(),
        render_prompt: None,
        source: TaskPackSource::BuiltIn,
    };
    pack.validate_schema().expect("built-in review pack should be valid");
    pack
}

fn built_in_requirements_pack() -> TaskPack {
    let pack = TaskPack {
        manifest: TaskPackManifest {
            id: "requirements-review".to_string(),
            version: "1".to_string(),
            title: "Requirements Review".to_string(),
            description: "Build consensus on requirements quality, ambiguity, and coverage."
                .to_string(),
            allowed_attachment_kinds: vec![AttachmentKind::Markdown, AttachmentKind::Text],
            context_providers: vec![ContextProvider::PromptOnly, ContextProvider::RepoDocs],
            roles: vec![
                TaskPackRole {
                    id: "ambiguity-reviewer".to_string(),
                    name: "Ambiguity Reviewer".to_string(),
                    persona: "Ambiguity Reviewer".to_string(),
                    focus: "Find vague wording, ambiguous requirements, and unstated assumptions".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "acceptance-criteria-reviewer".to_string(),
                    name: "Acceptance Criteria Reviewer".to_string(),
                    persona: "Acceptance Criteria Reviewer".to_string(),
                    focus: "Find missing acceptance criteria, measurable outcomes, and testability gaps".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "dependency-risk-reviewer".to_string(),
                    name: "Dependency and Rollout Reviewer".to_string(),
                    persona: "Dependency and Rollout Reviewer".to_string(),
                    focus: "Identify hidden dependencies, rollout risks, downstream coupling, and implementation traps".to_string(),
                    prompt_template: PromptTemplate::Challenge,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "requirements-judge".to_string(),
                    name: "Requirements Judge".to_string(),
                    persona: "Requirements Judge".to_string(),
                    focus: "Produce a clear consensus requirements report and clarification requests".to_string(),
                    prompt_template: PromptTemplate::Judge,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "requirements-analyzer".to_string(),
                    name: "Requirements Analyzer".to_string(),
                    persona: "Requirements Analyzer".to_string(),
                    focus: "Summarize goals, constraints, implied assumptions, and missing context".to_string(),
                    prompt_template: PromptTemplate::Analyze,
                    default_weight: 1.0,
                },
            ],
            rounds: vec![
                TaskRound {
                    name: "Initial Requirements Review".to_string(),
                    mode: RoundMode::Discover,
                    assignments: vec![
                        TaskAssignment {
                            role: "ambiguity-reviewer".to_string(),
                            plugin: "codex".to_string(),
                            weight_override: None,
                        },
                        TaskAssignment {
                            role: "acceptance-criteria-reviewer".to_string(),
                            plugin: "claude-code".to_string(),
                            weight_override: None,
                        },
                    ],
                },
                TaskRound {
                    name: "Dependency Challenge".to_string(),
                    mode: RoundMode::Challenge,
                    assignments: vec![TaskAssignment {
                        role: "dependency-risk-reviewer".to_string(),
                        plugin: "gemini".to_string(),
                        weight_override: None,
                    }],
                },
            ],
            quorum: 0.75,
            allow_clarifications: true,
            finalization: TaskFinalization {
                analyze: Some(TaskAssignment {
                    role: "requirements-analyzer".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                }),
                judge: TaskAssignment {
                    role: "requirements-judge".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                },
                convergence: None,
                structurizer: None,
                autofix: None,
            },
        },
        analyzer_prompt: "Identify the product goals, constraints, implied assumptions, and missing context in the supplied requirements material.".to_string(),
        reviewer_prompt: "Review the requirements for ambiguity, contradictions, missing acceptance criteria, hidden dependencies, rollout risks, and implementation traps.".to_string(),
        judge_prompt: "Produce a consensus requirements report with accepted requirements, ambiguities, missing acceptance criteria, risks, recommended next steps, and clarification requests when blocking information is missing.".to_string(),
        schema_json: r#"{
  "type":"object",
  "required":["accepted_requirements","ambiguities","missing_acceptance_criteria","risks","recommended_next_steps"],
  "properties":{
    "accepted_requirements":{"type":"array"},
    "ambiguities":{"type":"array"},
    "missing_acceptance_criteria":{"type":"array"},
    "risks":{"type":"array"},
    "recommended_next_steps":{"type":"array"}
  }
}"#.to_string(),
        render_prompt: None,
        source: TaskPackSource::BuiltIn,
    };
    pack.validate_schema().expect("built-in requirements pack should be valid");
    pack
}

fn built_in_design_pack() -> TaskPack {
    let pack = TaskPack {
        manifest: TaskPackManifest {
            id: "design-review".to_string(),
            version: "1".to_string(),
            title: "Design Review".to_string(),
            description: "Build consensus on architecture, tradeoffs, and failure modes."
                .to_string(),
            allowed_attachment_kinds: vec![
                AttachmentKind::Markdown,
                AttachmentKind::Text,
                AttachmentKind::SourceFile,
            ],
            context_providers: vec![ContextProvider::PromptOnly, ContextProvider::RepoDocs],
            roles: vec![
                TaskPackRole {
                    id: "architecture-reviewer".to_string(),
                    name: "Architecture Reviewer".to_string(),
                    persona: "Architecture Reviewer".to_string(),
                    focus: "Boundaries, interfaces, cohesion, and design soundness".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "failure-mode-reviewer".to_string(),
                    name: "Failure Mode Reviewer".to_string(),
                    persona: "Failure Mode Reviewer".to_string(),
                    focus: "Operational risk, recovery behavior, partial failure, and bad-case handling".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "migration-reviewer".to_string(),
                    name: "Migration Reviewer".to_string(),
                    persona: "Migration Reviewer".to_string(),
                    focus: "Adoption path, migration cost, backward compatibility, and rollout traps".to_string(),
                    prompt_template: PromptTemplate::Challenge,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "design-judge".to_string(),
                    name: "Design Judge".to_string(),
                    persona: "Design Judge".to_string(),
                    focus: "Synthesize tradeoffs, failure modes, and recommended design changes".to_string(),
                    prompt_template: PromptTemplate::Judge,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "design-analyzer".to_string(),
                    name: "Design Analyzer".to_string(),
                    persona: "Design Analyzer".to_string(),
                    focus: "Summarize the proposed design, interfaces, and most important tradeoffs".to_string(),
                    prompt_template: PromptTemplate::Analyze,
                    default_weight: 1.0,
                },
            ],
            rounds: vec![
                TaskRound {
                    name: "Initial Design Review".to_string(),
                    mode: RoundMode::Discover,
                    assignments: vec![
                        TaskAssignment {
                            role: "architecture-reviewer".to_string(),
                            plugin: "codex".to_string(),
                            weight_override: None,
                        },
                        TaskAssignment {
                            role: "failure-mode-reviewer".to_string(),
                            plugin: "claude-code".to_string(),
                            weight_override: None,
                        },
                    ],
                },
                TaskRound {
                    name: "Migration Challenge".to_string(),
                    mode: RoundMode::Challenge,
                    assignments: vec![TaskAssignment {
                        role: "migration-reviewer".to_string(),
                        plugin: "gemini".to_string(),
                        weight_override: None,
                    }],
                },
            ],
            quorum: 0.75,
            allow_clarifications: true,
            finalization: TaskFinalization {
                analyze: Some(TaskAssignment {
                    role: "design-analyzer".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                }),
                judge: TaskAssignment {
                    role: "design-judge".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                },
                convergence: None,
                structurizer: None,
                autofix: None,
            },
        },
        analyzer_prompt: "Summarize the proposed design, its boundaries, interfaces, and the most important tradeoffs.".to_string(),
        reviewer_prompt: "Review the design for soundness, operational risk, coupling, migration concerns, performance cliffs, and failure handling.".to_string(),
        judge_prompt: "Produce a consensus design report with decision summary, tradeoffs, open questions, failure modes, recommended changes, and clarification requests when needed.".to_string(),
        schema_json: r#"{
  "type":"object",
  "required":["decision_summary","tradeoffs","open_questions","failure_modes","recommended_changes"],
  "properties":{
    "decision_summary":{"type":"string"},
    "tradeoffs":{"type":"array"},
    "open_questions":{"type":"array"},
    "failure_modes":{"type":"array"},
    "recommended_changes":{"type":"array"}
  }
}"#.to_string(),
        render_prompt: None,
        source: TaskPackSource::BuiltIn,
    };
    pack.validate_schema().expect("built-in design pack should be valid");
    pack
}

fn built_in_test_plan_pack() -> TaskPack {
    let pack = TaskPack {
        manifest: TaskPackManifest {
            id: "test-plan-review".to_string(),
            version: "1".to_string(),
            title: "Test Plan Review".to_string(),
            description: "Build consensus on test coverage, gaps, and release blockers."
                .to_string(),
            allowed_attachment_kinds: vec![
                AttachmentKind::Markdown,
                AttachmentKind::Text,
                AttachmentKind::SourceFile,
            ],
            context_providers: vec![ContextProvider::PromptOnly, ContextProvider::RepoDocs],
            roles: vec![
                TaskPackRole {
                    id: "coverage-reviewer".to_string(),
                    name: "Coverage Reviewer".to_string(),
                    persona: "Coverage Reviewer".to_string(),
                    focus: "Missing scenarios, weak assertions, and untested behavior surfaces".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "fixture-reviewer".to_string(),
                    name: "Fixture Reviewer".to_string(),
                    persona: "Fixture Reviewer".to_string(),
                    focus: "Missing fixtures, absent data setup, environment gaps, and cross-system dependencies".to_string(),
                    prompt_template: PromptTemplate::Discover,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "release-gate-reviewer".to_string(),
                    name: "Release Gate Reviewer".to_string(),
                    persona: "Release Gate Reviewer".to_string(),
                    focus: "High-risk scenarios, release blockers, and validation required before rollout".to_string(),
                    prompt_template: PromptTemplate::Challenge,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "test-plan-judge".to_string(),
                    name: "Test Plan Judge".to_string(),
                    persona: "Test Plan Judge".to_string(),
                    focus: "Produce a consensus test plan report with actionable release guidance".to_string(),
                    prompt_template: PromptTemplate::Judge,
                    default_weight: 1.0,
                },
                TaskPackRole {
                    id: "test-plan-analyzer".to_string(),
                    name: "Test Plan Analyzer".to_string(),
                    persona: "Test Plan Analyzer".to_string(),
                    focus: "Identify target behavior, system risk, and expected validation surface".to_string(),
                    prompt_template: PromptTemplate::Analyze,
                    default_weight: 1.0,
                },
            ],
            rounds: vec![
                TaskRound {
                    name: "Initial Test Plan Review".to_string(),
                    mode: RoundMode::Discover,
                    assignments: vec![
                        TaskAssignment {
                            role: "coverage-reviewer".to_string(),
                            plugin: "codex".to_string(),
                            weight_override: None,
                        },
                        TaskAssignment {
                            role: "fixture-reviewer".to_string(),
                            plugin: "claude-code".to_string(),
                            weight_override: None,
                        },
                    ],
                },
                TaskRound {
                    name: "Release Gate Challenge".to_string(),
                    mode: RoundMode::Challenge,
                    assignments: vec![TaskAssignment {
                        role: "release-gate-reviewer".to_string(),
                        plugin: "gemini".to_string(),
                        weight_override: None,
                    }],
                },
            ],
            quorum: 0.75,
            allow_clarifications: true,
            finalization: TaskFinalization {
                analyze: Some(TaskAssignment {
                    role: "test-plan-analyzer".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                }),
                judge: TaskAssignment {
                    role: "test-plan-judge".to_string(),
                    plugin: "codex".to_string(),
                    weight_override: None,
                },
                convergence: None,
                structurizer: None,
                autofix: None,
            },
        },
        analyzer_prompt: "Identify the target behavior, system risks, and expected validation surface for the supplied test plan or feature request.".to_string(),
        reviewer_prompt: "Review the test plan for missing scenarios, weak assertions, absent fixtures, cross-system risks, and release-blocking coverage gaps.".to_string(),
        judge_prompt: "Produce a consensus test-plan report with coverage gaps, high-risk scenarios, missing fixtures, recommended test matrix, release blockers, and clarification requests when needed.".to_string(),
        schema_json: r#"{
  "type":"object",
  "required":["coverage_gaps","high_risk_scenarios","missing_fixtures","recommended_test_matrix","release_blockers"],
  "properties":{
    "coverage_gaps":{"type":"array"},
    "high_risk_scenarios":{"type":"array"},
    "missing_fixtures":{"type":"array"},
    "recommended_test_matrix":{"type":"array"},
    "release_blockers":{"type":"array"}
  }
}"#.to_string(),
        render_prompt: None,
        source: TaskPackSource::BuiltIn,
    };
    pack.validate_schema().expect("built-in test-plan pack should be valid");
    pack
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_packs_are_available() {
        let cfg = CrucibleConfig::default();
        let review = load_task_pack(&cfg, None, "review", &[]).unwrap();
        assert_eq!(review.id(), "review");
        let pack = load_task_pack(&cfg, None, "requirements-review", &[]).unwrap();
        assert_eq!(pack.id(), "requirements-review");
    }
}
