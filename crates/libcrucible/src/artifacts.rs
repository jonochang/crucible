use anyhow::Result;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct RunArtifacts {
    pub run_id: Uuid,
    pub run_dir: PathBuf,
    pub progress_log: PathBuf,
    pub debug_log: PathBuf,
    pub report_json: PathBuf,
}

impl RunArtifacts {
    pub fn create(cwd: &Path, run_id: Uuid) -> Result<Self> {
        let run_dir = cwd.join(".crucible").join("runs").join(run_id.to_string());
        std::fs::create_dir_all(&run_dir)?;
        Ok(Self {
            run_id,
            progress_log: run_dir.join("progress.log"),
            debug_log: run_dir.join("debug.log"),
            report_json: run_dir.join("report.json"),
            run_dir,
        })
    }
}
