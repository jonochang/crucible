use anyhow::Result;
use clap::Args;
use libcrucible::config::CrucibleConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Args)]
pub struct PromptEvalArgs {
    #[arg(long, help = "Path to golden prompt eval dataset JSON")]
    pub dataset: PathBuf,
    #[arg(long, help = "Output JSON report path")]
    pub out: PathBuf,
    #[arg(long, default_value_t = false, help = "Run with only codex reviewer for faster evaluations")]
    pub fast: bool,
}

#[derive(Debug, Deserialize)]
struct PromptEvalDataset {
    cases: Vec<PromptEvalCase>,
}

#[derive(Debug, Deserialize)]
struct PromptEvalCase {
    name: String,
    diff: String,
    expected: Vec<ExpectedIssue>,
}

#[derive(Debug, Deserialize)]
struct ExpectedIssue {
    severity: Option<String>,
    file: Option<String>,
    line_start: Option<u32>,
    title_contains: Option<String>,
}

#[derive(Debug, Serialize)]
struct PromptEvalCaseResult {
    name: String,
    expected_count: usize,
    actual_count: usize,
    matched: usize,
    precision: f32,
    recall: f32,
}

#[derive(Debug, Serialize)]
struct PromptEvalReport {
    total_cases: usize,
    avg_precision: f32,
    avg_recall: f32,
    cases: Vec<PromptEvalCaseResult>,
}

pub async fn run(args: PromptEvalArgs) -> Result<()> {
    let mut cfg = CrucibleConfig::load()?;
    if args.fast {
        cfg.plugins.agents = vec!["codex".to_string()];
        cfg.plugins.analyzer = "codex".to_string();
        cfg.plugins.judge = "codex".to_string();
        cfg.coordinator.max_rounds = 1;
    }
    let raw = std::fs::read_to_string(&args.dataset)?;
    let dataset: PromptEvalDataset = serde_json::from_str(&raw)?;

    let mut results = Vec::new();
    for case in dataset.cases {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let report = libcrucible::run_review_with_progress_diff(&cfg, tx, case.diff).await?;
        let matched = count_matches(&case.expected, &report.issues);
        let actual_count = report.issues.len();
        let expected_count = case.expected.len();
        let precision = if actual_count == 0 {
            0.0
        } else {
            matched as f32 / actual_count as f32
        };
        let recall = if expected_count == 0 {
            0.0
        } else {
            matched as f32 / expected_count as f32
        };
        results.push(PromptEvalCaseResult {
            name: case.name,
            expected_count,
            actual_count,
            matched,
            precision,
            recall,
        });
    }

    let avg_precision = if results.is_empty() {
        0.0
    } else {
        results.iter().map(|r| r.precision).sum::<f32>() / results.len() as f32
    };
    let avg_recall = if results.is_empty() {
        0.0
    } else {
        results.iter().map(|r| r.recall).sum::<f32>() / results.len() as f32
    };
    let report = PromptEvalReport {
        total_cases: results.len(),
        avg_precision,
        avg_recall,
        cases: results,
    };
    std::fs::write(&args.out, serde_json::to_string_pretty(&report)?)?;
    println!("Wrote prompt evaluation report to {}", args.out.display());
    Ok(())
}

fn count_matches(expected: &[ExpectedIssue], actual: &[libcrucible::report::CanonicalIssue]) -> usize {
    expected
        .iter()
        .filter(|exp| {
            actual.iter().any(|issue| issue_matches(exp, issue))
        })
        .count()
}

fn issue_matches(exp: &ExpectedIssue, issue: &libcrucible::report::CanonicalIssue) -> bool {
    if let Some(sev) = &exp.severity {
        let actual = format!("{:?}", issue.severity);
        if !actual.eq_ignore_ascii_case(sev) {
            return false;
        }
    }
    if let Some(file) = &exp.file {
        let actual = issue.file.as_ref().map(|p| p.display().to_string());
        if actual
            .as_deref()
            .map(|f| !f.eq_ignore_ascii_case(file))
            .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(line) = exp.line_start {
        if issue.line_start != Some(line) {
            return false;
        }
    }
    if let Some(title_contains) = &exp.title_contains {
        if !issue
            .title
            .to_lowercase()
            .contains(&title_contains.to_lowercase())
        {
            return false;
        }
    }
    true
}
