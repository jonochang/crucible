use crate::analysis::FocusAreas;
use crate::analysis::AgentContext;
use crate::config::CrucibleConfig;
use crate::consensus::{ConsensusAnchor, ConsensusItem, ItemImportance, TaskContext};
use crate::progress::ConvergenceVerdict;
use crate::report::{AutoFix, CanonicalIssue, Finding, RawFinding};
use crate::task_pack::{TaskAssignment, TaskPack, TaskPackRole};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashSet, VecDeque};
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
    pub active_plugin_ids: Vec<String>,
    pub standby_plugin_ids: VecDeque<String>,
    plugin_configs: BTreeMap<String, crate::config::CliPluginConfig>,
}

impl PluginRegistry {
    pub fn from_config(cfg: &CrucibleConfig) -> Result<Self> {
        let resolved_agents = cfg.plugins.resolve_available_agents()?;
        if resolved_agents.active.is_empty() {
            return Err(anyhow!("no agents configured"));
        }

        Ok(Self {
            active_plugin_ids: resolved_agents.active,
            standby_plugin_ids: resolved_agents.standby.into(),
            plugin_configs: cfg.plugins.agent_configs.clone(),
        })
    }

    pub fn build_execution_plan(&self, pack: &TaskPack) -> Result<ResolvedTaskPlan> {
        let mut rounds = Vec::new();
        for round in &pack.manifest.rounds {
            let mut used_plugins = HashSet::new();
            let mut resolved_assignments = Vec::new();
            for assignment in &round.assignments {
                let role = self.role_from_pack(pack, &assignment.role)?;
                let plugin_id = self.resolve_assignment_plugin(assignment, &used_plugins)?;
                used_plugins.insert(plugin_id.clone());
                resolved_assignments.push(ResolvedRoleAssignment::new(
                    role,
                    &plugin_id,
                    assignment.weight_override,
                ));
            }
            rounds.push(ResolvedRoundPlan {
                name: round.name.clone(),
                mode: round.mode.clone(),
                assignments: resolved_assignments,
            });
        }

        let finalization = ResolvedFinalizationPlan {
            analyze: self.resolve_optional_assignment(pack, pack.manifest.finalization.analyze.as_ref())?,
            judge: self.resolve_required_assignment(pack, &pack.manifest.finalization.judge)?,
            convergence: self.resolve_optional_assignment(
                pack,
                pack.manifest.finalization.convergence.as_ref(),
            )?,
            structurizer: self.resolve_optional_assignment(
                pack,
                pack.manifest.finalization.structurizer.as_ref(),
            )?,
            autofix: self.resolve_optional_assignment(pack, pack.manifest.finalization.autofix.as_ref())?,
        };

        Ok(ResolvedTaskPlan { rounds, finalization })
    }

    pub fn instantiate_role_agent(
        &self,
        assignment: &ResolvedRoleAssignment,
    ) -> Result<Arc<dyn AgentPlugin>> {
        let plugin_cfg = self
            .plugin_configs
            .get(&assignment.plugin_id)
            .ok_or_else(|| anyhow!("missing config for plugin {}", assignment.plugin_id))?;
        let plugin = crate::plugins::cli_agent::CliAgentPlugin::from_role(
            &assignment.runtime_id,
            &assignment.plugin_id,
            plugin_cfg,
            &assignment.role,
        );
        Ok(Arc::new(plugin))
    }

    pub fn instantiate_focus_analyzer(
        &self,
        assignment: &ResolvedRoleAssignment,
    ) -> Result<Arc<dyn FocusAnalyzer>> {
        let plugin_cfg = self
            .plugin_configs
            .get(&assignment.plugin_id)
            .ok_or_else(|| anyhow!("missing config for plugin {}", assignment.plugin_id))?;
        let plugin = crate::plugins::cli_agent::CliAgentPlugin::from_role(
            &assignment.runtime_id,
            &assignment.plugin_id,
            plugin_cfg,
            &assignment.role,
        );
        Ok(Arc::new(plugin))
    }

    fn resolve_required_assignment(
        &self,
        pack: &TaskPack,
        assignment: &TaskAssignment,
    ) -> Result<ResolvedRoleAssignment> {
        let role = self.role_from_pack(pack, &assignment.role)?;
        let plugin_id = self.resolve_assignment_plugin(assignment, &HashSet::new())?;
        Ok(ResolvedRoleAssignment::new(
            role,
            &plugin_id,
            assignment.weight_override,
        ))
    }

    fn resolve_optional_assignment(
        &self,
        pack: &TaskPack,
        assignment: Option<&TaskAssignment>,
    ) -> Result<Option<ResolvedRoleAssignment>> {
        assignment
            .map(|assignment| self.resolve_required_assignment(pack, assignment))
            .transpose()
    }

    fn role_from_pack(&self, pack: &TaskPack, role_id: &str) -> Result<TaskPackRole> {
        pack.manifest
            .roles
            .iter()
            .find(|role| role.id == role_id)
            .cloned()
            .ok_or_else(|| anyhow!("task pack '{}' missing role '{}'", pack.id(), role_id))
    }

    fn resolve_assignment_plugin(
        &self,
        assignment: &TaskAssignment,
        used_plugins: &HashSet<String>,
    ) -> Result<String> {
        let allowed = &self.active_plugin_ids;
        if allowed.is_empty() {
            return Err(anyhow!("no configured plugins available on PATH"));
        }
        if allowed.contains(&assignment.plugin) && !used_plugins.contains(&assignment.plugin) {
            return Ok(assignment.plugin.clone());
        }
        if let Some(id) = allowed.iter().find(|id| !used_plugins.contains(*id)) {
            return Ok(id.clone());
        }
        if allowed.contains(&assignment.plugin) {
            return Ok(assignment.plugin.clone());
        }
        allowed
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("no configured plugins available on PATH"))
    }
}

pub struct ResolvedAgents {
    pub active: Vec<String>,
    pub standby: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedRoleAssignment {
    pub runtime_id: String,
    pub plugin_id: String,
    pub role: TaskPackRole,
    pub weight: f32,
}

impl ResolvedRoleAssignment {
    fn new(role: TaskPackRole, plugin_id: &str, weight_override: Option<f32>) -> Self {
        Self {
            runtime_id: format!("{}@{}", role.id, plugin_id),
            plugin_id: plugin_id.to_string(),
            weight: weight_override.unwrap_or(role.default_weight),
            role,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedRoundPlan {
    pub name: String,
    pub mode: crate::task_pack::RoundMode,
    pub assignments: Vec<ResolvedRoleAssignment>,
}

#[derive(Debug, Clone)]
pub struct ResolvedFinalizationPlan {
    pub analyze: Option<ResolvedRoleAssignment>,
    pub judge: ResolvedRoleAssignment,
    pub convergence: Option<ResolvedRoleAssignment>,
    pub structurizer: Option<ResolvedRoleAssignment>,
    pub autofix: Option<ResolvedRoleAssignment>,
}

#[derive(Debug, Clone)]
pub struct ResolvedTaskPlan {
    pub rounds: Vec<ResolvedRoundPlan>,
    pub finalization: ResolvedFinalizationPlan,
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
        let ids = registry.active_plugin_ids.clone();
        assert_eq!(ids, vec!["claude-code", "codex", "open-code"]);
        let standby = registry.standby_plugin_ids.iter().cloned().collect::<Vec<_>>();
        assert!(standby.is_empty());
    }

    #[test]
    fn registry_honors_explicit_single_reviewer_override() {
        let _lock = path_lock().lock().expect("path lock");
        let dir = TempDir::new().expect("tempdir");
        make_executable(dir.path(), "opencode");
        let _guard = PathGuard::set(&path_with_bins(&dir));

        let mut cfg = CrucibleConfig::default();
        cfg.plugins.agents = vec!["open-code".to_string()];

        let registry = PluginRegistry::from_config(&cfg).expect("registry");
        let ids = registry.active_plugin_ids.clone();

        assert_eq!(ids, vec!["open-code"]);
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
        let ids = registry.active_plugin_ids.clone();
        let standby = registry.standby_plugin_ids.iter().cloned().collect::<Vec<_>>();

        assert_eq!(ids, vec!["claude-code", "codex", "gemini"]);
        assert_eq!(standby, vec!["open-code"]);
    }
}
