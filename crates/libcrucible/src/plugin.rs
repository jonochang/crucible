use crate::analysis::AgentContext;
use crate::analysis::FocusAreas;
use crate::config::CrucibleConfig;
use crate::report::{AutoFix, Finding, RawFinding};
use anyhow::{Result, anyhow};
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct AgentReviewOutput {
    pub findings: Vec<RawFinding>,
    pub narrative: String,
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
}

pub struct PluginRegistry {
    pub agents: Vec<Box<dyn AgentPlugin>>,
    pub judge: Box<dyn AgentPlugin>,
    pub analyzer: Box<dyn FocusAnalyzer>,
}

impl PluginRegistry {
    pub fn from_config(cfg: &CrucibleConfig) -> Result<Self> {
        if cfg.plugins.agents.is_empty() {
            return Err(anyhow!("no agents configured"));
        }

        let mut agents: Vec<Box<dyn AgentPlugin>> = Vec::new();
        for id in &cfg.plugins.agents {
            let plugin_cfg = cfg
                .plugins
                .resolve_role(id)
                .ok_or_else(|| anyhow!("missing config for plugin {id}"))?;
            let plugin = crate::plugins::cli_agent::CliAgentPlugin::from_config(id, plugin_cfg);
            agents.push(Box::new(plugin));
        }

        let judge_cfg = cfg
            .plugins
            .resolve_role(&cfg.plugins.judge)
            .ok_or_else(|| anyhow!("missing config for judge"))?;
        let analyzer_cfg = cfg
            .plugins
            .resolve_role(&cfg.plugins.analyzer)
            .ok_or_else(|| anyhow!("missing config for analyzer"))?;

        let judge =
            crate::plugins::cli_agent::CliAgentPlugin::from_config(&cfg.plugins.judge, judge_cfg);
        let analyzer = crate::plugins::cli_agent::CliAgentPlugin::from_config(
            &cfg.plugins.analyzer,
            analyzer_cfg,
        );

        Ok(Self {
            agents,
            judge: Box::new(judge),
            analyzer: Box::new(analyzer),
        })
    }
}
