use anyhow::{Context, Result};
use glob::glob;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocSnippet {
    pub path: PathBuf,
    pub contents: String,
}

pub struct DocsCollector {
    patterns: Vec<String>,
    max_bytes: usize,
}

impl DocsCollector {
    pub fn new(patterns: Vec<String>, max_bytes: usize) -> Self {
        Self { patterns, max_bytes }
    }

    pub fn collect(&self, root: &Path) -> Result<Vec<DocSnippet>> {
        let mut snippets = Vec::new();
        for pattern in &self.patterns {
            let full = root.join(pattern);
            let pattern_str = full.to_string_lossy().to_string();
            for entry in glob(&pattern_str).context("invalid glob")? {
                let path = entry?;
                if path.is_file() {
                    let mut contents = fs::read_to_string(&path)
                        .with_context(|| format!("read docs {}", path.display()))?;
                    if contents.len() > self.max_bytes {
                        contents.truncate(self.max_bytes);
                    }
                    snippets.push(DocSnippet { path, contents });
                }
            }
        }
        Ok(snippets)
    }
}
