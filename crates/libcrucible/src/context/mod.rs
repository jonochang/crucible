use crate::config::CrucibleConfig;
use crate::context::docs::DocsCollector;
use crate::context::history::HistoryCollector;
use crate::context::precheck::collect_precheck_signals;
use crate::context::reference::ReferenceCollector;
use crate::progress::{ProgressEvent, StartupPhase, StartupPhaseStatus};
use anyhow::{Context, Result};
use git2::{DiffFormat, DiffOptions, Repository};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::task;

pub mod docs;
pub mod history;
pub mod precheck;
pub mod reference;

#[derive(Debug, Clone)]
pub struct ReviewContext {
    pub diff: String,
    pub changed_files: Vec<PathBuf>,
    pub base_ref: String,
    pub head_ref: String,
    pub repo_root: PathBuf,
    pub gathered: GatheredContext,
    pub dep_graph: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatheredContext {
    pub references: Vec<reference::Reference>,
    pub history: Vec<history::CommitSummary>,
    pub docs: Vec<docs::DocSnippet>,
    #[serde(default)]
    pub prechecks: Vec<precheck::PrecheckSignal>,
}

impl ReviewContext {
    pub async fn from_push(cwd: &Path, cfg: &CrucibleConfig) -> Result<Self> {
        Self::from_push_with_progress(cwd, cfg, None).await
    }

    pub async fn from_push_with_progress(
        cwd: &Path,
        cfg: &CrucibleConfig,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<ProgressEvent>>,
    ) -> Result<Self> {
        let repo = Repository::discover(cwd).context("discover git repo")?;
        let repo_root = repo.workdir().context("repo has no workdir")?.to_path_buf();

        let diff = build_diff(&repo)?;
        let changed_files = diff_changed_files(&repo)?;
        let repo_path = repo.path().to_path_buf();

        let context_cfg = cfg.context.clone();
        let repo_root_clone = repo_root.clone();
        let diff_clone = diff.clone();

        let refs_cfg = context_cfg.clone();
        let refs_task = task::spawn_blocking(move || {
            ReferenceCollector::collect(&diff_clone, &repo_root_clone, &refs_cfg)
        });

        let history_cfg = context_cfg.clone();
        let changed_files_clone = changed_files.clone();
        let history_task = task::spawn_blocking(move || {
            let repo = Repository::open(repo_path)?;
            HistoryCollector::new(
                history_cfg.history_max_commits,
                history_cfg.history_max_days,
            )
            .collect(&changed_files_clone, &repo)
        });

        let docs_cfg = context_cfg.clone();
        let docs_root = repo_root.clone();
        let docs_task = task::spawn_blocking(move || {
            DocsCollector::new(docs_cfg.docs_patterns, docs_cfg.docs_max_bytes).collect(&docs_root)
        });

        emit_startup(
            progress,
            StartupPhase::References,
            StartupPhaseStatus::Started,
            None,
            None,
            "Scanning related references",
        );
        emit_startup(
            progress,
            StartupPhase::History,
            StartupPhaseStatus::Started,
            None,
            None,
            "Collecting recent history",
        );
        emit_startup(
            progress,
            StartupPhase::Docs,
            StartupPhaseStatus::Started,
            None,
            None,
            "Loading docs context",
        );
        let refs_started = Instant::now();
        let history_started = Instant::now();
        let docs_started = Instant::now();
        let (references, history, docs) = tokio::join!(refs_task, history_task, docs_task);
        let references = match references {
            Ok(Ok(items)) => {
                emit_startup(
                    progress,
                    StartupPhase::References,
                    StartupPhaseStatus::Completed,
                    Some(items.len()),
                    Some(refs_started.elapsed().as_secs_f32()),
                    "Reference scan complete",
                );
                items
            }
            Ok(Err(err)) => {
                emit_startup(
                    progress,
                    StartupPhase::References,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(refs_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err);
            }
            Err(err) => {
                emit_startup(
                    progress,
                    StartupPhase::References,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(refs_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };
        let history = match history {
            Ok(Ok(items)) => {
                emit_startup(
                    progress,
                    StartupPhase::History,
                    StartupPhaseStatus::Completed,
                    Some(items.len()),
                    Some(history_started.elapsed().as_secs_f32()),
                    "History collection complete",
                );
                items
            }
            Ok(Err(err)) => {
                emit_startup(
                    progress,
                    StartupPhase::History,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(history_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err);
            }
            Err(err) => {
                emit_startup(
                    progress,
                    StartupPhase::History,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(history_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };
        let docs = match docs {
            Ok(Ok(items)) => {
                emit_startup(
                    progress,
                    StartupPhase::Docs,
                    StartupPhaseStatus::Completed,
                    Some(items.len()),
                    Some(docs_started.elapsed().as_secs_f32()),
                    "Docs collection complete",
                );
                items
            }
            Ok(Err(err)) => {
                emit_startup(
                    progress,
                    StartupPhase::Docs,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(docs_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err);
            }
            Err(err) => {
                emit_startup(
                    progress,
                    StartupPhase::Docs,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(docs_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };
        emit_startup(
            progress,
            StartupPhase::Prechecks,
            StartupPhaseStatus::Started,
            None,
            None,
            "Running prechecks",
        );
        let prechecks_started = Instant::now();
        let prechecks = match collect_precheck_signals(&repo_root, cfg) {
            Ok(items) => {
                emit_startup(
                    progress,
                    StartupPhase::Prechecks,
                    StartupPhaseStatus::Completed,
                    Some(items.len()),
                    Some(prechecks_started.elapsed().as_secs_f32()),
                    "Prechecks complete",
                );
                items
            }
            Err(err) => {
                emit_startup(
                    progress,
                    StartupPhase::Prechecks,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(prechecks_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err);
            }
        };
        let gathered = GatheredContext {
            references,
            history,
            docs,
            prechecks,
        };

        Ok(Self {
            diff,
            changed_files,
            base_ref: "HEAD~1".to_string(),
            head_ref: "HEAD".to_string(),
            repo_root,
            gathered,
            dep_graph: None,
        })
    }

    /// Build review context from an externally-provided patch string.
    /// Used by CLI target modes such as PR/branch/files that precompute their own diff.
    pub async fn from_diff(cwd: &Path, cfg: &CrucibleConfig, diff: String) -> Result<Self> {
        Self::from_diff_with_progress(cwd, cfg, diff, None).await
    }

    pub async fn from_diff_with_progress(
        cwd: &Path,
        cfg: &CrucibleConfig,
        diff: String,
        progress: Option<&tokio::sync::mpsc::UnboundedSender<ProgressEvent>>,
    ) -> Result<Self> {
        let repo = Repository::discover(cwd).context("discover git repo")?;
        let repo_root = repo.workdir().context("repo has no workdir")?.to_path_buf();
        let changed_files = changed_files_from_patch(&diff);
        let repo_path = repo.path().to_path_buf();

        let context_cfg = cfg.context.clone();
        let repo_root_clone = repo_root.clone();
        let diff_clone = diff.clone();

        let refs_cfg = context_cfg.clone();
        let refs_task = task::spawn_blocking(move || {
            ReferenceCollector::collect(&diff_clone, &repo_root_clone, &refs_cfg)
        });

        let history_cfg = context_cfg.clone();
        let changed_files_clone = changed_files.clone();
        let history_task = task::spawn_blocking(move || {
            let repo = Repository::open(repo_path)?;
            HistoryCollector::new(
                history_cfg.history_max_commits,
                history_cfg.history_max_days,
            )
            .collect(&changed_files_clone, &repo)
        });

        let docs_cfg = context_cfg.clone();
        let docs_root = repo_root.clone();
        let docs_task = task::spawn_blocking(move || {
            DocsCollector::new(docs_cfg.docs_patterns, docs_cfg.docs_max_bytes).collect(&docs_root)
        });

        emit_startup(
            progress,
            StartupPhase::References,
            StartupPhaseStatus::Started,
            None,
            None,
            "Scanning related references",
        );
        emit_startup(
            progress,
            StartupPhase::History,
            StartupPhaseStatus::Started,
            None,
            None,
            "Collecting recent history",
        );
        emit_startup(
            progress,
            StartupPhase::Docs,
            StartupPhaseStatus::Started,
            None,
            None,
            "Loading docs context",
        );
        let refs_started = Instant::now();
        let history_started = Instant::now();
        let docs_started = Instant::now();
        let (references, history, docs) = tokio::join!(refs_task, history_task, docs_task);
        let references = match references {
            Ok(Ok(items)) => {
                emit_startup(
                    progress,
                    StartupPhase::References,
                    StartupPhaseStatus::Completed,
                    Some(items.len()),
                    Some(refs_started.elapsed().as_secs_f32()),
                    "Reference scan complete",
                );
                items
            }
            Ok(Err(err)) => {
                emit_startup(
                    progress,
                    StartupPhase::References,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(refs_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err);
            }
            Err(err) => {
                emit_startup(
                    progress,
                    StartupPhase::References,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(refs_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };
        let history = match history {
            Ok(Ok(items)) => {
                emit_startup(
                    progress,
                    StartupPhase::History,
                    StartupPhaseStatus::Completed,
                    Some(items.len()),
                    Some(history_started.elapsed().as_secs_f32()),
                    "History collection complete",
                );
                items
            }
            Ok(Err(err)) => {
                emit_startup(
                    progress,
                    StartupPhase::History,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(history_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err);
            }
            Err(err) => {
                emit_startup(
                    progress,
                    StartupPhase::History,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(history_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };
        let docs = match docs {
            Ok(Ok(items)) => {
                emit_startup(
                    progress,
                    StartupPhase::Docs,
                    StartupPhaseStatus::Completed,
                    Some(items.len()),
                    Some(docs_started.elapsed().as_secs_f32()),
                    "Docs collection complete",
                );
                items
            }
            Ok(Err(err)) => {
                emit_startup(
                    progress,
                    StartupPhase::Docs,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(docs_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err);
            }
            Err(err) => {
                emit_startup(
                    progress,
                    StartupPhase::Docs,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(docs_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err.into());
            }
        };
        emit_startup(
            progress,
            StartupPhase::Prechecks,
            StartupPhaseStatus::Started,
            None,
            None,
            "Running prechecks",
        );
        let prechecks_started = Instant::now();
        let prechecks = match collect_precheck_signals(&repo_root, cfg) {
            Ok(items) => {
                emit_startup(
                    progress,
                    StartupPhase::Prechecks,
                    StartupPhaseStatus::Completed,
                    Some(items.len()),
                    Some(prechecks_started.elapsed().as_secs_f32()),
                    "Prechecks complete",
                );
                items
            }
            Err(err) => {
                emit_startup(
                    progress,
                    StartupPhase::Prechecks,
                    StartupPhaseStatus::Failed,
                    None,
                    Some(prechecks_started.elapsed().as_secs_f32()),
                    &err.to_string(),
                );
                return Err(err);
            }
        };
        let gathered = GatheredContext {
            references,
            history,
            docs,
            prechecks,
        };

        Ok(Self {
            diff,
            changed_files,
            base_ref: "custom".to_string(),
            head_ref: "custom".to_string(),
            repo_root,
            gathered,
            dep_graph: None,
        })
    }

    pub fn into_agent_ctx(
        &self,
        focus: Option<&crate::analysis::FocusAreas>,
    ) -> crate::analysis::AgentContext {
        crate::analysis::AgentContext {
            diff: self.diff.clone(),
            gathered: self.gathered.clone(),
            focus: focus.cloned(),
            dep_graph: self.dep_graph.clone(),
            review_pack: None,
        }
    }
}

fn build_diff(repo: &Repository) -> Result<String> {
    let mut opts = DiffOptions::new();
    opts.include_untracked(true);
    let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo.diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut opts))?;
    let mut buf = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        if let Ok(content) = std::str::from_utf8(line.content()) {
            push_diff_line(&mut buf, line.origin(), content);
        }
        true
    })?;
    Ok(buf)
}

fn diff_changed_files(repo: &Repository) -> Result<Vec<PathBuf>> {
    let mut opts = DiffOptions::new();
    opts.include_untracked(true);
    let head = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo.diff_tree_to_workdir_with_index(head.as_ref(), Some(&mut opts))?;
    let mut files = Vec::new();
    diff.foreach(
        &mut |delta, _| {
            if let Some(path) = delta.new_file().path() {
                files.push(path.to_path_buf());
            }
            true
        },
        None,
        None,
        None,
    )?;
    files.sort();
    files.dedup();
    Ok(files)
}

/// Extract changed file paths from unified diff headers (`+++ b/<path>`).
fn changed_files_from_patch(diff: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            if path != "/dev/null" {
                files.push(PathBuf::from(path));
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn emit_startup(
    progress: Option<&tokio::sync::mpsc::UnboundedSender<ProgressEvent>>,
    phase: StartupPhase,
    status: StartupPhaseStatus,
    count: Option<usize>,
    duration_secs: Option<f32>,
    detail: &str,
) {
    if let Some(tx) = progress {
        let _ = tx.send(ProgressEvent::StartupPhase {
            phase,
            status,
            count,
            duration_secs,
            detail: detail.to_string(),
        });
    }
}

fn push_diff_line(buf: &mut String, origin: char, content: &str) {
    match origin {
        '+' | '-' | ' ' => {
            buf.push(origin);
            buf.push_str(content);
        }
        _ => buf.push_str(content),
    }
}
