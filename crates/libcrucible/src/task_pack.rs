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
    pub persona: String,
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
    #[serde(default = "default_rounds")]
    pub rounds: u8,
    #[serde(default = "default_quorum")]
    pub quorum: f32,
    #[serde(default)]
    pub allow_clarifications: bool,
}

fn default_rounds() -> u8 {
    2
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
    Ok(Some(TaskPack {
        manifest,
        analyzer_prompt,
        reviewer_prompt,
        judge_prompt,
        schema_json,
        render_prompt,
        source: TaskPackSource::Directory(dir.to_path_buf()),
    }))
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
    TaskPack {
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
            roles: vec![],
            rounds: 2,
            quorum: 0.75,
            allow_clarifications: false,
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
    }
}

fn built_in_requirements_pack() -> TaskPack {
    TaskPack {
        manifest: TaskPackManifest {
            id: "requirements-review".to_string(),
            version: "1".to_string(),
            title: "Requirements Review".to_string(),
            description: "Build consensus on requirements quality, ambiguity, and coverage."
                .to_string(),
            allowed_attachment_kinds: vec![AttachmentKind::Markdown, AttachmentKind::Text],
            context_providers: vec![ContextProvider::PromptOnly, ContextProvider::RepoDocs],
            roles: vec![],
            rounds: 2,
            quorum: 0.75,
            allow_clarifications: true,
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
    }
}

fn built_in_design_pack() -> TaskPack {
    TaskPack {
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
            roles: vec![],
            rounds: 2,
            quorum: 0.75,
            allow_clarifications: true,
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
    }
}

fn built_in_test_plan_pack() -> TaskPack {
    TaskPack {
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
            roles: vec![],
            rounds: 2,
            quorum: 0.75,
            allow_clarifications: true,
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
    }
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
