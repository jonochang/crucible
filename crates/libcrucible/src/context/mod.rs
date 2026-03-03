use crate::config::CrucibleConfig;
use crate::context::docs::DocsCollector;
use crate::context::history::HistoryCollector;
use crate::context::reference::ReferenceCollector;
use anyhow::{Context, Result};
use git2::{DiffFormat, DiffOptions, Repository};
use std::path::{Path, PathBuf};
use tokio::task;

pub mod docs;
pub mod history;
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

#[derive(Debug, Clone, Default)]
pub struct GatheredContext {
    pub references: Vec<reference::Reference>,
    pub history: Vec<history::CommitSummary>,
    pub docs: Vec<docs::DocSnippet>,
}

impl ReviewContext {
    pub async fn from_push(cwd: &Path, cfg: &CrucibleConfig) -> Result<Self> {
        let repo = Repository::discover(cwd).context("discover git repo")?;
        let repo_root = repo.workdir().context("repo has no workdir")?.to_path_buf();

        let diff = build_diff(&repo)?;
        let changed_files = diff_changed_files(&repo)?;
        let repo_path = repo.path().to_path_buf();

        let context_cfg = cfg.context.clone();
        let repo_root_clone = repo_root.clone();
        let diff_clone = diff.clone();

        let refs_task = task::spawn_blocking(move || {
            ReferenceCollector::collect(&diff_clone, &repo_root_clone, &context_cfg)
        });

        let history_cfg = context_cfg.clone();
        let changed_files_clone = changed_files.clone();
        let history_task = task::spawn_blocking(move || {
            let repo = Repository::open(repo_path)?;
            HistoryCollector::new(history_cfg.history_max_commits, history_cfg.history_max_days)
                .collect(&changed_files_clone, &repo)
        });

        let docs_cfg = context_cfg.clone();
        let docs_root = repo_root.clone();
        let docs_task = task::spawn_blocking(move || {
            DocsCollector::new(docs_cfg.docs_patterns, docs_cfg.docs_max_bytes).collect(&docs_root)
        });

        let (references, history, docs) = tokio::join!(refs_task, history_task, docs_task);
        let gathered = GatheredContext {
            references: references??,
            history: history??,
            docs: docs??,
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

    pub fn into_agent_ctx(&self, focus: Option<&crate::analysis::FocusAreas>) -> crate::analysis::AgentContext {
        crate::analysis::AgentContext {
            diff: self.diff.clone(),
            gathered: self.gathered.clone(),
            focus: focus.cloned(),
            dep_graph: self.dep_graph.clone(),
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
            buf.push_str(content);
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
