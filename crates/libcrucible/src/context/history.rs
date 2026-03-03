use anyhow::Result;
use git2::{Repository, Sort};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone)]
pub struct CommitSummary {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub date: chrono::DateTime<chrono::Utc>,
}

pub struct HistoryCollector {
    max_commits: usize,
    max_days: u32,
}

impl HistoryCollector {
    pub fn new(max_commits: usize, max_days: u32) -> Self {
        Self { max_commits, max_days }
    }

    pub fn collect(&self, files: &[PathBuf], repo: &Repository) -> Result<Vec<CommitSummary>> {
        let mut revwalk = repo.revwalk()?;
        revwalk.push_head()?;
        revwalk.set_sorting(Sort::TIME)?;

        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(self.max_days as u64 * 86400))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let cutoff_ts = cutoff.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs() as i64;

        let mut summaries = Vec::new();
        for oid in revwalk {
            let oid = oid?;
            let commit = repo.find_commit(oid)?;
            let time = commit.time();
            if time.seconds() < cutoff_ts {
                continue;
            }

            let mut touched = false;
            let tree = commit.tree()?;
            let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
            let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
            diff.foreach(
                &mut |delta, _| {
                    if let Some(path) = delta.new_file().path() {
                        if files.iter().any(|f| f == path) {
                            touched = true;
                        }
                    }
                    true
                },
                None,
                None,
                None,
            )?;

            if touched {
                let author = commit.author();
                let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(time.seconds(), 0)
                    .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap());
                let summary = CommitSummary {
                    sha: commit.id().to_string(),
                    message: commit.summary().unwrap_or("").to_string(),
                    author: author.name().unwrap_or("unknown").to_string(),
                    date: dt,
                };
                summaries.push(summary);
                if summaries.len() >= self.max_commits {
                    break;
                }
            }
        }

        Ok(summaries)
    }
}
