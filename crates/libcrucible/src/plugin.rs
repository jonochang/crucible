use crate::analysis::AgentContext;
use crate::consensus::{ConsensusAnchor, ConsensusItem, ItemImportance, TaskContext};
use crate::analysis::FocusAreas;
use crate::config::CrucibleConfig;
use crate::progress::ConvergenceVerdict;
use crate::report::{AutoFix, CanonicalIssue, Finding, RawFinding};
use crate::task_pack::TaskPack;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Arc;
use which::which;

#[derive(Debug, Clone)]
pub struct AgentReviewOutput {
    pub findings: Vec<RawFinding>,
    pub narrative: String,
}

#[derive(Debug, Clone)]
pub struct ConvergenceDecision {
    pub verdict: ConvergenceVerdict,
    pub rationale: String,
}

#[derive(Debug, Clone)]
pub struct GenericAgentOutput {
    pub items: Vec<RawConsensusItem>,
    pub narrative: String,
}

#[derive(Debug, Clone)]
pub struct GenericFinalOutput {
    pub summary_markdown: String,
    pub result_json: serde_json::Value,
    pub clarification_requests: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RawConsensusItem {
    pub kind: String,
    pub importance: ItemImportance,
    pub title: String,
    pub message: String,
    pub confidence: crate::report::Confidence,
    pub anchors: Vec<ConsensusAnchor>,
}

#[async_trait]
pub trait AgentPlugin: Send + Sync {
    fn id(&self) -> &str;
    fn persona(&self) -> &str;

    async fn analyze(&self, ctx: &AgentContext) -> Result<AgentReviewOutput>;

    async fn debate(
        &self,
        ctx: &AgentContext,
        round: u8,
        synthesis: &crate::coordinator::CrossPollinationSynthesis,
    ) -> Result<AgentReviewOutput>;

    async fn summarize(&self, ctx: &AgentContext, findings: &[Finding]) -> Result<AutoFix>;

    async fn judge_convergence(
        &self,
        _ctx: &AgentContext,
        _round: u8,
        _findings: &[Finding],
    ) -> Result<ConvergenceDecision> {
        Ok(ConvergenceDecision {
            verdict: ConvergenceVerdict::NotConverged,
            rationale: "No explicit convergence judge configured".to_string(),
        })
    }

    async fn structurize_issues(
        &self,
        _ctx: &AgentContext,
        _findings: &[Finding],
    ) -> Result<Vec<CanonicalIssue>> {
        Ok(Vec::new())
    }

    async fn analyze_task(&self, _ctx: &TaskContext, _pack: &TaskPack) -> Result<GenericAgentOutput> {
        Err(anyhow!("generic task analysis is not implemented for {}", self.id()))
    }

    async fn debate_task(
        &self,
        _ctx: &TaskContext,
        _pack: &TaskPack,
        _round: u8,
        _prior_summary: &str,
    ) -> Result<GenericAgentOutput> {
        Err(anyhow!("generic task debate is not implemented for {}", self.id()))
    }

    async fn summarize_task(
        &self,
        _ctx: &TaskContext,
        _pack: &TaskPack,
        _agreed_items: &[ConsensusItem],
        _unresolved_items: &[ConsensusItem],
    ) -> Result<GenericFinalOutput> {
        Err(anyhow!("generic task summary is not implemented for {}", self.id()))
    }

    async fn judge_task_convergence(
        &self,
        _ctx: &TaskContext,
        _round: u8,
        _items: &[ConsensusItem],
    ) -> Result<ConvergenceDecision> {
        Ok(ConvergenceDecision {
            verdict: ConvergenceVerdict::NotConverged,
            rationale: "No generic convergence judge configured".to_string(),
        })
    }

    fn session_capability(&self) -> Option<&dyn SessionCapable> {
        None
    }
}

pub trait SessionCapable {
    fn start_session(&mut self, name: &str) -> Result<String>;
    fn resume_session(&self) -> Option<String>;
    fn mark_messages_sent(&mut self, up_to_index: usize);
    fn last_sent_index(&self) -> usize;
    fn end_session(&mut self);
}

#[async_trait]
pub trait FocusAnalyzer: Send + Sync {
    async fn analyze_focus(&self, ctx: &AgentContext) -> Result<FocusAreas>;

    async fn analyze_task_focus(&self, _ctx: &TaskContext) -> Result<FocusAreas> {
        Ok(FocusAreas {
            summary: String::new(),
            focus_items: Vec::new(),
            trade_offs: Vec::new(),
            affected_modules: Vec::new(),
            call_chain: Vec::new(),
            design_patterns: Vec::new(),
            reviewer_checklist: Vec::new(),
        })
    }
}

pub struct PluginRegistry {
    pub agents: Vec<Arc<dyn AgentPlugin>>,
    pub standby_agents: VecDeque<Arc<dyn AgentPlugin>>,
    pub judge: Arc<dyn AgentPlugin>,
    pub analyzer: Arc<dyn FocusAnalyzer>,
}

impl PluginRegistry {
    pub fn from_config(cfg: &CrucibleConfig) -> Result<Self> {
        let resolved_agents = cfg.plugins.resolve_available_agents()?;
        if resolved_agents.active.is_empty() {
            return Err(anyhow!("no agents configured"));
        }

        let mut agents: Vec<Arc<dyn AgentPlugin>> = Vec::new();
        for id in &resolved_agents.active {
            let plugin_cfg = cfg
                .plugins
                .resolve_role(id)
                .ok_or_else(|| anyhow!("missing config for plugin {id}"))?;
            let plugin = crate::plugins::cli_agent::CliAgentPlugin::from_config(id, plugin_cfg);
            agents.push(Arc::new(plugin));
        }
        let mut standby_agents: VecDeque<Arc<dyn AgentPlugin>> = VecDeque::new();
        for id in &resolved_agents.standby {
            let plugin_cfg = cfg
                .plugins
                .resolve_role(id)
                .ok_or_else(|| anyhow!("missing config for plugin {id}"))?;
            let plugin = crate::plugins::cli_agent::CliAgentPlugin::from_config(id, plugin_cfg);
            standby_agents.push_back(Arc::new(plugin));
        }

        let fallback_role = resolved_agents
            .active
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("no available agents found on PATH"))?;
        let judge_id = cfg.plugins.resolve_available_role(&cfg.plugins.judge, &fallback_role);
        let analyzer_id = cfg
            .plugins
            .resolve_available_role(&cfg.plugins.analyzer, &fallback_role);

        let judge_cfg = cfg
            .plugins
            .resolve_role(&judge_id)
            .ok_or_else(|| anyhow!("missing config for judge"))?;
        let analyzer_cfg = cfg
            .plugins
            .resolve_role(&analyzer_id)
            .ok_or_else(|| anyhow!("missing config for analyzer"))?;

        let judge = crate::plugins::cli_agent::CliAgentPlugin::from_config(&judge_id, judge_cfg);
        let analyzer =
            crate::plugins::cli_agent::CliAgentPlugin::from_config(&analyzer_id, analyzer_cfg);

        Ok(Self {
            agents,
            standby_agents,
            judge: Arc::new(judge),
            analyzer: Arc::new(analyzer),
        })
    }
}

pub struct ResolvedAgents {
    pub active: Vec<String>,
    pub standby: Vec<String>,
}

impl crate::config::PluginsConfig {
    fn preferred_agent_order(&self) -> [&'static str; 4] {
        ["claude-code", "codex", "gemini", "open-code"]
    }

    fn uses_default_agent_pool(&self) -> bool {
        self.agents.len() > 1
            && self
                .agents
                .iter()
                .all(|id| self.preferred_agent_order().contains(&id.as_str()))
    }

    pub fn resolve_available_agents(&self) -> Result<ResolvedAgents> {
        let requested = if self.uses_default_agent_pool() {
            self.preferred_agent_order()
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        } else {
            self.agents.clone()
        };

        let available = requested
            .into_iter()
            .filter(|id| self.role_on_path(id))
            .collect::<Vec<_>>();

        if !available.is_empty() {
            let active = available.iter().take(3).cloned().collect::<Vec<_>>();
            let standby = available.iter().skip(3).cloned().collect::<Vec<_>>();
            return Ok(ResolvedAgents { active, standby });
        }

        if self.uses_default_agent_pool() {
            return Err(anyhow!(
                "no available agents found on PATH from preferred pool: claude-code, codex, gemini, open-code"
            ));
        }

        Err(anyhow!("no configured agents found on PATH"))
    }

    pub fn resolve_available_role(&self, requested: &str, fallback: &str) -> String {
        if self.role_on_path(requested) {
            requested.to_string()
        } else {
            fallback.to_string()
        }
    }

    fn role_on_path(&self, id: &str) -> bool {
        self.resolve_role(id)
            .map(|role| which(&role.command).is_ok())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::PluginRegistry;
    use crate::config::CrucibleConfig;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn path_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct PathGuard {
        original: Option<std::ffi::OsString>,
    }

    impl PathGuard {
        fn set(path: &Path) -> Self {
            let original = env::var_os("PATH");
            unsafe {
                env::set_var("PATH", path);
            }
            Self { original }
        }
    }

    impl Drop for PathGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => unsafe {
                    env::set_var("PATH", value);
                },
                None => unsafe {
                    env::remove_var("PATH");
                },
            }
        }
    }

    fn make_executable(dir: &Path, name: &str) {
        let path = dir.join(name);
        fs::write(&path, "#!/usr/bin/env sh\nexit 0\n").expect("write mock binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
        }
    }

    fn path_with_bins(dir: &TempDir) -> PathBuf {
        dir.path().to_path_buf()
    }

    #[test]
    fn registry_prefers_first_three_available_agents_in_order() {
        let _lock = path_lock().lock().expect("path lock");
        let dir = TempDir::new().expect("tempdir");
        make_executable(dir.path(), "claude");
        make_executable(dir.path(), "codex");
        make_executable(dir.path(), "opencode");
        let _guard = PathGuard::set(&path_with_bins(&dir));

        let cfg = CrucibleConfig::default();
        let registry = PluginRegistry::from_config(&cfg).expect("registry");
        let ids = registry
            .agents
            .iter()
            .map(|agent| agent.id().to_string())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["claude-code", "codex", "open-code"]);
        let standby = registry
            .standby_agents
            .iter()
            .map(|agent| agent.id().to_string())
            .collect::<Vec<_>>();
        assert!(standby.is_empty());
        assert_eq!(registry.judge.id(), "claude-code");
    }

    #[test]
    fn registry_honors_explicit_single_reviewer_override() {
        let _lock = path_lock().lock().expect("path lock");
        let dir = TempDir::new().expect("tempdir");
        make_executable(dir.path(), "opencode");
        let _guard = PathGuard::set(&path_with_bins(&dir));

        let mut cfg = CrucibleConfig::default();
        cfg.plugins.agents = vec!["open-code".to_string()];
        cfg.plugins.judge = "open-code".to_string();
        cfg.plugins.analyzer = "open-code".to_string();

        let registry = PluginRegistry::from_config(&cfg).expect("registry");
        let ids = registry
            .agents
            .iter()
            .map(|agent| agent.id().to_string())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["open-code"]);
        assert_eq!(registry.judge.id(), "open-code");
    }

    #[test]
    fn registry_keeps_fourth_available_agent_as_standby() {
        let _lock = path_lock().lock().expect("path lock");
        let dir = TempDir::new().expect("tempdir");
        make_executable(dir.path(), "claude");
        make_executable(dir.path(), "codex");
        make_executable(dir.path(), "gemini");
        make_executable(dir.path(), "opencode");
        let _guard = PathGuard::set(&path_with_bins(&dir));

        let cfg = CrucibleConfig::default();
        let registry = PluginRegistry::from_config(&cfg).expect("registry");
        let ids = registry
            .agents
            .iter()
            .map(|agent| agent.id().to_string())
            .collect::<Vec<_>>();
        let standby = registry
            .standby_agents
            .iter()
            .map(|agent| agent.id().to_string())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["claude-code", "codex", "gemini"]);
        assert_eq!(standby, vec!["open-code"]);
    }
}
