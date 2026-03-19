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
use uuid::Uuid;

pub async fn run_review(cfg: &config::CrucibleConfig) -> Result<ReviewReport> {
    run_review_with_run_id(cfg, Uuid::new_v4()).await
}

pub async fn run_review_with_run_id(
    cfg: &config::CrucibleConfig,
    run_id: Uuid,
) -> Result<ReviewReport> {
    let ctx = context::ReviewContext::from_push(&std::env::current_dir()?, cfg).await?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut coord = Coordinator::new(registry, cfg.clone(), None, run_id);
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
    let ctx =
        context::ReviewContext::from_push_with_progress(&std::env::current_dir()?, cfg, Some(&tx))
            .await?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut coord = Coordinator::new(registry, cfg.clone(), Some(tx), run_id);
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
    let ctx = context::ReviewContext::from_diff_with_progress(
        &std::env::current_dir()?,
        cfg,
        diff,
        Some(&tx),
    )
    .await?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut coord = Coordinator::new(registry, cfg.clone(), Some(tx), run_id);
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
