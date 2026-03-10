use crate::report::ReviewReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPhase {
    References,
    History,
    Docs,
    Prechecks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPhaseStatus {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct AgentFindingPreview {
    pub severity: String,
    pub location: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewerState {
    Queued,
    Running,
    Done,
    Error,
}

#[derive(Debug, Clone)]
pub struct ReviewerStatus {
    pub id: String,
    pub state: ReviewerState,
    pub duration_secs: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergenceVerdict {
    Converged,
    NotConverged,
}

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    RunHeader {
        reviewers: Vec<String>,
        max_rounds: u8,
        changed_files: usize,
        changed_lines: usize,
        convergence_enabled: bool,
        context_enabled: bool,
    },
    PhaseStart {
        phase: String,
    },
    PhaseDone {
        phase: String,
    },
    AnalyzerStart,
    AnalyzerDone,
    StartupPhase {
        phase: StartupPhase,
        status: StartupPhaseStatus,
        count: Option<usize>,
        duration_secs: Option<f32>,
        detail: String,
    },
    AnalysisReady {
        markdown: String,
    },
    SystemContextReady {
        markdown: String,
    },
    RoundStart {
        round: u8,
        total_rounds: u8,
        agents: Vec<String>,
    },
    ParallelStatus {
        round: u8,
        statuses: Vec<ReviewerStatus>,
    },
    AgentStart {
        round: u8,
        id: String,
    },
    AgentReview {
        round: u8,
        id: String,
        summary: String,
        highlights: Vec<AgentFindingPreview>,
        details: String,
    },
    AgentDone {
        round: u8,
        id: String,
    },
    AgentError {
        round: u8,
        id: String,
        message: String,
    },
    RoundDone {
        round: u8,
    },
    ConvergenceJudgment {
        round: u8,
        verdict: ConvergenceVerdict,
        rationale: String,
    },
    RoundComplete {
        round: u8,
        total_rounds: u8,
    },
    AutoFixReady,
    Completed(ReviewReport),
    Canceled,
}
