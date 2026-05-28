pub mod analysis;
pub mod artifacts;
pub mod consensus;
pub mod config;
pub mod context;
pub mod coordinator;
pub mod plugin;
pub mod plugins;
pub mod pr_review;
pub mod progress;
pub mod report;
pub mod task_pack;

use anyhow::Result;
use consensus::{ConsensusReport, ConsensusTaskRequest};
use coordinator::Coordinator;
use plugin::PluginRegistry;
use report::ReviewReport;
use task_pack::load_task_pack;
use uuid::Uuid;

pub struct DoctorReport {
    pub config_ok: bool,
    pub config_path: Option<String>,
    pub config_error: Option<String>,
    pub configured_agents: Vec<String>,
    pub agent_resolution: AgentResolutionReport,
    pub execution_plan: ExecutionPlanReport,
    pub agent_checks: Vec<plugins::HealthCheckResult>,
}

pub struct AgentResolutionReport {
    pub ok: bool,
    pub active: Vec<String>,
    pub standby: Vec<String>,
    pub error: Option<String>,
}

pub struct ExecutionPlanReport {
    pub ok: bool,
    pub pack_id: String,
    pub rounds: Vec<String>,
    pub roles: Vec<(String, String)>,
    pub error: Option<String>,
}

pub fn run_doctor(skip_probes: bool) -> DoctorReport {
    let (config_ok, config_path, config_error, cfg) = match config::CrucibleConfig::load() {
        Ok(cfg) => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let path = config::CrucibleConfig::find_config_path(&cwd);
            (true, path.map(|p| p.display().to_string()), None, Some(cfg))
        }
        Err(err) => (false, None, Some(format!("{err:#}")), None),
    };

    let agent_resolution = match &cfg {
        Some(cfg) => match PluginRegistry::from_config(cfg) {
            Ok(registry) => AgentResolutionReport {
                ok: true,
                active: registry.active_plugin_ids.clone(),
                standby: registry.standby_plugin_ids.iter().cloned().collect(),
                error: None,
            },
            Err(err) => AgentResolutionReport {
                ok: false,
                active: vec![],
                standby: vec![],
                error: Some(format!("{err:#}")),
            },
        },
        None => AgentResolutionReport {
            ok: false,
            active: vec![],
            standby: vec![],
            error: Some("config not loaded".to_string()),
        },
    };

    let execution_plan = match &cfg {
        Some(cfg) => {
            let registry = PluginRegistry::from_config(cfg).ok();
            match registry {
                Some(registry) => {
                    let pack_result = load_task_pack(cfg, Some(&std::env::current_dir().unwrap_or_default()), "review", &[]);
                    match pack_result {
                        Ok(pack) => {
                            let plan_result = registry.build_execution_plan(&pack);
                            match plan_result {
                                Ok(plan) => {
                                    let rounds = plan.rounds.iter().map(|r| r.name.clone()).collect();
                                    let roles = plan
                                        .rounds
                                        .iter()
                                        .flat_map(|r| {
                                            r.assignments
                                                .iter()
                                                .map(|a| (a.role.id.clone(), a.plugin_id.clone()))
                                        })
                                        .collect();
                                    ExecutionPlanReport {
                                        ok: true,
                                        pack_id: pack.id().to_string(),
                                        rounds,
                                        roles,
                                        error: None,
                                    }
                                }
                                Err(err) => ExecutionPlanReport {
                                    ok: false,
                                    pack_id: pack.id().to_string(),
                                    rounds: vec![],
                                    roles: vec![],
                                    error: Some(format!("{err:#}")),
                                },
                            }
                        }
                        Err(err) => ExecutionPlanReport {
                            ok: false,
                            pack_id: "review".to_string(),
                            rounds: vec![],
                            roles: vec![],
                            error: Some(format!("{err:#}")),
                        },
                    }
                }
                None => ExecutionPlanReport {
                    ok: false,
                    pack_id: "review".to_string(),
                    rounds: vec![],
                    roles: vec![],
                    error: Some("registry not available".to_string()),
                },
            }
        }
        None => ExecutionPlanReport {
            ok: false,
            pack_id: "review".to_string(),
            rounds: vec![],
            roles: vec![],
            error: Some("config not loaded".to_string()),
        },
    };

    let agent_checks = if skip_probes {
        vec![]
    } else {
        match &cfg {
            Some(cfg) => {
                let mut checks = Vec::new();
                for plugin_id in &cfg.plugins.agents {
                    let plugin_cfg = cfg.plugins.resolve_role(plugin_id);
                    if let Some(pcfg) = plugin_cfg {
                        let dummy_role = task_pack::TaskPackRole {
                            id: "doctor".to_string(),
                            name: "Health Check".to_string(),
                            persona: "Health check agent".to_string(),
                            focus: String::new(),
                            prompt_template: task_pack::PromptTemplate::Discover,
                            default_weight: 1.0,
                        };
                        let agent = plugins::cli_agent::CliAgentPlugin::from_role(
                            &format!("doctor@{}", plugin_id),
                            plugin_id,
                            pcfg,
                            &dummy_role,
                        );
                        eprintln!("  Checking {plugin_id}...");
                        checks.push(agent.health_check());
                    }
                }
                checks
            }
            None => vec![],
        }
    };

    DoctorReport {
        config_ok,
        config_path,
        config_error,
        configured_agents: cfg.as_ref().map(|c| c.plugins.agents.clone()).unwrap_or_default(),
        agent_resolution,
        execution_plan,
        agent_checks,
    }
}

pub async fn run_review(cfg: &config::CrucibleConfig) -> Result<ReviewReport> {
    run_review_with_run_id(cfg, Uuid::new_v4()).await
}

pub async fn run_review_with_run_id(
    cfg: &config::CrucibleConfig,
    run_id: Uuid,
) -> Result<ReviewReport> {
    let ctx = context::ReviewContext::from_push(&std::env::current_dir()?, cfg).await?;
    let review_pack = load_task_pack(cfg, Some(&std::env::current_dir()?), "review", &[])?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut coord = Coordinator::new(registry, cfg.clone(), None, run_id).with_review_pack(review_pack);
    coord.run(&ctx).await
}

pub async fn run_review_with_progress(
    cfg: &config::CrucibleConfig,
    tx: tokio::sync::mpsc::UnboundedSender<progress::ProgressEvent>,
) -> Result<ReviewReport> {
    run_review_with_progress_run_id(cfg, tx, Uuid::new_v4()).await
}

pub async fn run_review_with_progress_run_id(
    cfg: &config::CrucibleConfig,
    tx: tokio::sync::mpsc::UnboundedSender<progress::ProgressEvent>,
    run_id: Uuid,
) -> Result<ReviewReport> {
    plugins::set_progress_sender(Some(tx.clone()))?;
    let cwd = std::env::current_dir()?;
    let ctx =
        context::ReviewContext::from_push_with_progress(&cwd, cfg, Some(&tx))
            .await?;
    let review_pack = load_task_pack(cfg, Some(&cwd), "review", &[])?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut coord =
        Coordinator::new(registry, cfg.clone(), Some(tx), run_id).with_review_pack(review_pack);
    let result = coord.run(&ctx).await;
    let _ = plugins::set_progress_sender(None);
    result
}

pub async fn run_review_with_progress_diff(
    cfg: &config::CrucibleConfig,
    tx: tokio::sync::mpsc::UnboundedSender<progress::ProgressEvent>,
    diff: String,
) -> Result<ReviewReport> {
    run_review_with_progress_diff_run_id(cfg, tx, diff, Uuid::new_v4()).await
}

pub async fn run_review_with_progress_diff_run_id(
    cfg: &config::CrucibleConfig,
    tx: tokio::sync::mpsc::UnboundedSender<progress::ProgressEvent>,
    diff: String,
    run_id: Uuid,
) -> Result<ReviewReport> {
    plugins::set_progress_sender(Some(tx.clone()))?;
    let cwd = std::env::current_dir()?;
    let ctx = context::ReviewContext::from_diff_with_progress(
        &cwd,
        cfg,
        diff,
        Some(&tx),
    )
    .await?;
    let review_pack = load_task_pack(cfg, Some(&cwd), "review", &[])?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut coord =
        Coordinator::new(registry, cfg.clone(), Some(tx), run_id).with_review_pack(review_pack);
    let result = coord.run(&ctx).await;
    let _ = plugins::set_progress_sender(None);
    result
}

pub async fn run_consensus(
    cfg: &config::CrucibleConfig,
    request: ConsensusTaskRequest,
) -> Result<ConsensusReport> {
    run_consensus_with_run_id(cfg, request, Uuid::new_v4()).await
}

pub async fn run_consensus_with_run_id(
    cfg: &config::CrucibleConfig,
    request: ConsensusTaskRequest,
    run_id: Uuid,
) -> Result<ConsensusReport> {
    consensus::run_consensus(cfg, request, run_id).await
}
