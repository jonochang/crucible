use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrucibleConfig {
    pub crucible: CrucibleSection,
    pub gate: GateConfig,
    pub context: ContextConfig,
    pub coordinator: CoordinatorConfig,
    pub verdict: VerdictConfig,
    pub rate_limits: RateLimitConfig,
    #[serde(default)]
    pub prechecks: PrecheckConfig,
    #[serde(default)]
    pub task_packs: TaskPackConfig,
    pub plugins: PluginsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrucibleSection {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateConfig {
    pub enabled: bool,
    pub untangle_bin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextConfig {
    pub reference_max_depth: usize,
    pub reference_max_files: usize,
    pub history_max_commits: usize,
    pub history_max_days: u32,
    pub docs_patterns: Vec<String>,
    pub docs_max_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorConfig {
    pub max_rounds: u8,
    pub quorum_threshold: f32,
    pub agent_timeout_secs: u64,
    pub devil_advocate: bool,
    #[serde(default = "default_max_diff_lines_per_chunk")]
    pub max_diff_lines_per_chunk: usize,
    #[serde(default = "default_max_diff_chunks")]
    pub max_diff_chunks: usize,
    #[serde(default = "default_enable_structurizer")]
    pub enable_structurizer: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerdictConfig {
    pub block_on: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    pub anthropic_rpm: u32,
    pub google_rpm: u32,
    pub openai_rpm: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrecheckConfig {
    pub enabled: bool,
    pub include_untangle: bool,
    pub include_linters: bool,
    pub include_type_checks: bool,
    pub include_tests: bool,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TaskPackConfig {
    #[serde(default)]
    pub paths: Vec<String>,
}

impl Default for PrecheckConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            include_untangle: true,
            include_linters: true,
            include_type_checks: true,
            include_tests: true,
            timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginsConfig {
    pub agents: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(flatten)]
    pub agent_configs: BTreeMap<String, CliPluginConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CliPluginConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

impl Default for CrucibleConfig {
    fn default() -> Self {
        Self {
            crucible: CrucibleSection {
                version: "1".to_string(),
            },
            gate: GateConfig {
                enabled: true,
                untangle_bin: "untangle".to_string(),
            },
            context: ContextConfig {
                reference_max_depth: 2,
                reference_max_files: 30,
                history_max_commits: 20,
                history_max_days: 30,
                docs_patterns: vec![
                    "docs/**/*.md".to_string(),
                    "README.md".to_string(),
                    "ARCHITECTURE.md".to_string(),
                ],
                docs_max_bytes: 50_000,
            },
            coordinator: CoordinatorConfig {
                max_rounds: 2,
                quorum_threshold: 0.75,
                agent_timeout_secs: 90,
                devil_advocate: false,
                max_diff_lines_per_chunk: default_max_diff_lines_per_chunk(),
                max_diff_chunks: default_max_diff_chunks(),
                enable_structurizer: default_enable_structurizer(),
            },
            verdict: VerdictConfig {
                block_on: "Critical".to_string(),
            },
            rate_limits: RateLimitConfig {
                anthropic_rpm: 50,
                google_rpm: 60,
                openai_rpm: 60,
            },
            prechecks: PrecheckConfig::default(),
            task_packs: TaskPackConfig::default(),
            plugins: PluginsConfig {
                agents: vec![
                    "claude-code".to_string(),
                    "codex".to_string(),
                    "gemini".to_string(),
                    "open-code".to_string(),
                ],
                paths: vec![],
                agent_configs: {
                    let mut m = BTreeMap::new();
                    m.insert(
                        "claude-code".to_string(),
                        CliPluginConfig {
                            command: "claude".to_string(),
                            args: vec![
                                "-p".to_string(),
                                "--output-format".to_string(),
                                "json".to_string(),
                            ],
                        },
                    );
                    m.insert(
                        "codex".to_string(),
                        CliPluginConfig {
                            command: "codex".to_string(),
                            args: vec![
                                "exec".to_string(),
                                "-".to_string(),
                                "--color".to_string(),
                                "never".to_string(),
                            ],
                        },
                    );
                    m.insert(
                        "gemini".to_string(),
                        CliPluginConfig {
                            command: "gemini".to_string(),
                            args: vec!["-y".to_string(), "-o".to_string(), "json".to_string()],
                        },
                    );
                    m.insert(
                        "open-code".to_string(),
                        CliPluginConfig {
                            command: "opencode".to_string(),
                            args: vec![],
                        },
                    );
                    m.insert(
                        "opencode-kimi".to_string(),
                        CliPluginConfig {
                            command: "opencode".to_string(),
                            args: vec![
                                "run".to_string(),
                                "--model".to_string(),
                                "moonshot/kimi-k2-5".to_string(),
                                "--format".to_string(),
                                "json".to_string(),
                            ],
                        },
                    );
                    m.insert(
                        "opencode-glm".to_string(),
                        CliPluginConfig {
                            command: "opencode".to_string(),
                            args: vec![
                                "run".to_string(),
                                "--model".to_string(),
                                "zai-coding-plan/glm-5.1".to_string(),
                                "--format".to_string(),
                                "json".to_string(),
                            ],
                        },
                    );
                    m
                },
            },
        }
    }
}

fn default_max_diff_lines_per_chunk() -> usize {
    1200
}

fn default_max_diff_chunks() -> usize {
    6
}

fn default_enable_structurizer() -> bool {
    true
}

impl CrucibleConfig {
    pub fn default_full() -> Self {
        let mut cfg = Self::default();
        cfg.plugins.agents = cfg.plugins.agent_configs.keys().cloned().collect();
        cfg
    }

    pub fn load() -> Result<Self> {
        let cwd = std::env::current_dir().context("get current dir")?;
        if let Some(path) = find_config_path(&cwd) {
            return load_from_path(&path);
        }

        let home = std::env::var("HOME").map_err(|_| anyhow!("HOME not set"))?;
        let fallback = PathBuf::from(home).join(".config/crucible/config.toml");
        if fallback.exists() {
            return load_from_path(&fallback);
        }

        Ok(Self::default())
    }
}

fn find_config_path(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        let candidate = dir.join(".crucible.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

fn load_from_path(path: &Path) -> Result<CrucibleConfig> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("read config {}", path.display()))?;
    let expanded = expand_env(&raw)?;
    let cfg: CrucibleConfig = toml::from_str(&expanded).context("parse config toml")?;
    Ok(cfg)
}

fn expand_env(input: &str) -> Result<String> {
    let re = regex::Regex::new(r"\$\{([A-Z0-9_]+)\}").expect("regex");
    let mut missing = Vec::new();
    let output = re.replace_all(input, |caps: &regex::Captures| {
        let key = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        match std::env::var(key) {
            Ok(val) => val,
            Err(_) => {
                missing.push(key.to_string());
                String::new()
            }
        }
    });

    if !missing.is_empty() {
        return Err(anyhow!(
            "missing env vars in config: {}",
            missing.join(", ")
        ));
    }
    Ok(output.to_string())
}

impl PluginsConfig {
    pub fn resolve_role(&self, id: &str) -> Option<&CliPluginConfig> {
        self.agent_configs.get(id)
    }
}
