pub mod analysis;
pub mod config;
pub mod context;
pub mod coordinator;
pub mod plugin;
pub mod plugins;
pub mod progress;
pub mod report;

use anyhow::Result;
use coordinator::Coordinator;
use plugin::PluginRegistry;
use report::ReviewReport;

pub async fn run_review(cfg: &config::CrucibleConfig) -> Result<ReviewReport> {
    let ctx = context::ReviewContext::from_push(&std::env::current_dir()?, cfg).await?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut coord = Coordinator::new(registry, cfg.clone(), None);
    coord.run(&ctx).await
}

pub async fn run_review_with_progress(
    cfg: &config::CrucibleConfig,
    tx: tokio::sync::mpsc::UnboundedSender<progress::ProgressEvent>,
) -> Result<ReviewReport> {
    let ctx = context::ReviewContext::from_push(&std::env::current_dir()?, cfg).await?;
    let registry = PluginRegistry::from_config(cfg)?;
    let mut coord = Coordinator::new(registry, cfg.clone(), Some(tx));
    coord.run(&ctx).await
}
