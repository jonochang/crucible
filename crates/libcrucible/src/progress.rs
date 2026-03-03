use crate::report::ReviewReport;

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    AnalyzerStart,
    AnalyzerDone,
    ReviewStart,
    ReviewDone,
    AutoFixReady,
    Completed(ReviewReport),
}
