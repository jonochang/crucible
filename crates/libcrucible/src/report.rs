use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReport {
    pub run_id: Uuid,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub issues: Vec<CanonicalIssue>,
    #[serde(default)]
    pub analysis_markdown: Option<String>,
    #[serde(default)]
    pub system_context_markdown: Option<String>,
    #[serde(default)]
    pub final_analysis_markdown: Option<String>,
    pub consensus_map: ConsensusMap,
    pub auto_fix: Option<AutoFix>,
    #[serde(default)]
    pub final_action_plan: Option<FinalActionPlan>,
    #[serde(default)]
    pub pr_comment_markdown: Option<String>,
    #[serde(default)]
    pub pr_review_draft: Option<PullRequestReviewDraft>,
    pub session_id: Uuid,
}

impl ReviewReport {
    pub fn from_findings(
        run_id: Uuid,
        findings: &[Finding],
        issues: Vec<CanonicalIssue>,
        analysis_markdown: Option<String>,
        system_context_markdown: Option<String>,
        final_analysis_markdown: Option<String>,
        cfg: &crate::config::VerdictConfig,
        consensus: ConsensusMap,
        auto_fix: Option<AutoFix>,
        final_action_plan: Option<FinalActionPlan>,
        pr_comment_markdown: Option<String>,
        pr_review_draft: Option<PullRequestReviewDraft>,
    ) -> Self {
        let verdict = Verdict::from_findings(findings, cfg);
        Self {
            run_id,
            verdict,
            findings: findings.to_vec(),
            issues,
            analysis_markdown,
            system_context_markdown,
            final_analysis_markdown,
            consensus_map: consensus,
            auto_fix,
            final_action_plan,
            pr_comment_markdown,
            pr_review_draft,
            session_id: run_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Warn,
    Block,
}

impl Verdict {
    pub fn from_findings(findings: &[Finding], cfg: &crate::config::VerdictConfig) -> Self {
        let block_on = cfg.block_on.as_str();
        let has_critical = findings.iter().any(|f| f.severity == Severity::Critical);
        let has_warning = findings.iter().any(|f| f.severity == Severity::Warning);
        match block_on {
            "Critical" if has_critical => Verdict::Block,
            "Warning" if has_critical || has_warning => Verdict::Block,
            _ => {
                if has_critical || has_warning {
                    Verdict::Warn
                } else {
                    Verdict::Pass
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub agent: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub file: Option<PathBuf>,
    pub span: Option<LineSpan>,
    pub message: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub suggested_fix: Option<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceAnchor>,
    pub round: u8,
    pub raised_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawFinding {
    pub severity: Severity,
    pub file: Option<PathBuf>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub message: String,
    pub confidence: Confidence,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub suggested_fix: Option<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct LineSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoFix {
    pub unified_diff: String,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FinalActionPlan {
    pub prioritized_steps: Vec<String>,
    pub quick_wins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PullRequestReviewDraft {
    pub overview_comment: PullRequestOverviewComment,
    #[serde(default)]
    pub inline_comments: Vec<PullRequestCommentDraft>,
    #[serde(default)]
    pub overview_only_comments: Vec<PullRequestCommentDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PullRequestOverviewComment {
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestCommentDraft {
    pub severity: Severity,
    pub category: String,
    pub title: String,
    pub description: String,
    pub body: String,
    pub path: Option<PathBuf>,
    pub line: Option<u32>,
    pub side: Option<PullRequestCommentSide>,
    pub start_line: Option<u32>,
    pub start_side: Option<PullRequestCommentSide>,
    pub source_agents: Vec<String>,
    pub mapping_status: PullRequestCommentMappingStatus,
    #[serde(default)]
    pub mapping_note: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum PullRequestCommentSide {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum PullRequestCommentMappingStatus {
    #[default]
    Inline,
    OverviewOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalIssue {
    pub severity: Severity,
    pub category: String,
    pub file: Option<PathBuf>,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
    pub title: String,
    pub description: String,
    pub suggested_fix: Option<String>,
    pub raised_by: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EvidenceAnchor {
    pub location: String,
    pub quote: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConsensusMap(pub HashMap<FindingKey, ConsensusStatus>);

#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct FindingKey {
    pub file: PathBuf,
    pub span: LineSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusStatus {
    pub agreed_count: usize,
    pub total_agents: usize,
    pub severity: Severity,
    pub reached_quorum: bool,
}
