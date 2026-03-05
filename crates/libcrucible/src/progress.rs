use crate::report::ReviewReport;

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    AnalyzerStart,
    AnalyzerDone,
    RoundStart { round: u8, total_rounds: u8, agents: Vec<String> },
    AgentStart { round: u8, id: String },
    AgentDone { round: u8, id: String },
    AgentError { round: u8, id: String, message: String },
    RoundDone { round: u8 },
    AutoFixReady,
    Completed(ReviewReport),
    Canceled,
}
