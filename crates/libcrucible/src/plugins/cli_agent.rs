use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use crate::analysis::{AgentContext, FocusAreas};
use crate::config::CliPluginConfig;
use crate::plugin::{AgentPlugin, FocusAnalyzer};
use crate::report::{AutoFix, Finding, RawFinding, Severity, Confidence};
use serde::Deserialize;
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct CliAgentPlugin {
    id: String,
    persona: String,
    command: String,
    args: Vec<String>,
}

impl CliAgentPlugin {
    pub fn from_config(id: &str, cfg: &CliPluginConfig) -> Self {
        Self {
            id: id.to_string(),
            persona: cfg.persona.clone(),
            command: cfg.command.clone(),
            args: cfg.args.clone(),
        }
    }

    fn run_cli<T: for<'de> Deserialize<'de>>(&self, system: String, user: String) -> Result<T> {
        let prompt = format!("[SYSTEM]\n{}\n\n[USER]\n{}\n", system, user);
        let mut child = Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {}", self.command))?;

        if let Some(stdin) = child.stdin.as_mut() {
            use std::io::Write;
            stdin.write_all(prompt.as_bytes()).context("write prompt")?;
        }

        let output = child.wait_with_output().context("wait for CLI output")?;
        if !output.status.success() {
            return Err(anyhow!(
                "{} failed: {}",
                self.command,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let parsed: T = serde_json::from_str(&stdout).context("parse JSON from CLI")?;
        Ok(parsed)
    }

    fn build_context_sections(&self, ctx: &AgentContext) -> (String, String, String) {
        let focus = ctx
            .focus
            .as_ref()
            .map(|f| serde_json::to_string(f).unwrap_or_default())
            .unwrap_or_else(|| "null".to_string());
        let references = serde_json::to_string(&ctx.gathered.references).unwrap_or_default();
        let history = serde_json::to_string(&ctx.gathered.history).unwrap_or_default();
        (focus, references, history)
    }
}

#[async_trait]
impl AgentPlugin for CliAgentPlugin {
    fn id(&self) -> &str {
        &self.id
    }

    fn persona(&self) -> &str {
        &self.persona
    }

    async fn analyze(&self, ctx: &AgentContext) -> Result<Vec<RawFinding>> {
        let (focus, references, history) = self.build_context_sections(ctx);
        let system = format!(
            "You are a {} performing a structured code review.\n\nIMPORTANT: Respond ONLY with a valid JSON object matching this schema.\nDo not include any text outside the JSON object.\n\n{{\n  \"findings\": [\n    {{\n      \"severity\":   \"Critical | Warning | Info\",\n      \"file\":       \"<relative path or null>\",\n      \"line_start\": <integer or null>,\n      \"line_end\":   <integer or null>,\n      \"message\":    \"<concise, actionable description>\",\n      \"confidence\": \"High | Medium | Low\"\n    }}\n  ]\n}}\n",
            self.persona
        );

        let user = format!(
            "Review context:\n- Suggested focus areas: {}\n- Relevant references: {}\n- Recent commit history: {}\n\nDiff to review:\n{}",
            focus, references, history, ctx.diff
        );

        let resp: FindingsResponse = self.run_cli(system, user)?;
        Ok(resp.findings.into_iter().map(|f| f.into()).collect())
    }

    async fn debate(
        &self,
        _ctx: &AgentContext,
        _round: u8,
        _synthesis: &crate::coordinator::CrossPollinationSynthesis,
    ) -> Result<Vec<RawFinding>> {
        Ok(Vec::new())
    }

    async fn summarize(&self, ctx: &AgentContext, findings: &[Finding]) -> Result<AutoFix> {
        let system = "You are a senior engineer producing an auto-fix for agreed findings.\nRespond ONLY with valid JSON: { \"unified_diff\": \"...\", \"explanation\": \"...\" }".to_string();
        let user = format!(
            "Findings:\n{}\n\nDiff:\n{}",
            serde_json::to_string(findings).unwrap_or_default(),
            ctx.diff
        );
        let resp: AutoFixResponse = self.run_cli(system, user)?;
        Ok(AutoFix { unified_diff: resp.unified_diff, explanation: resp.explanation })
    }
}

#[async_trait]
impl FocusAnalyzer for CliAgentPlugin {
    async fn analyze_focus(&self, ctx: &AgentContext) -> Result<FocusAreas> {
        let system = "You are a senior architect providing pre-review context.\nOutput ONLY valid JSON matching this schema:\n{\n  \"summary\":     \"<what this change does in 2 sentences>\",\n  \"focus_items\": [{ \"area\": \"Security\", \"rationale\": \"...\" }],\n  \"trade_offs\":  [\"...\"]\n}\nFocus items are SUGGESTIONS — debaters should also flag anything beyond these.".to_string();
        let user = format!("Diff to review:\n{}", ctx.diff);
        let resp: FocusAreas = self.run_cli(system, user)?;
        Ok(resp)
    }
}

#[derive(Debug, Deserialize)]
struct FindingsResponse {
    findings: Vec<RawFindingResponse>,
}

#[derive(Debug, Deserialize)]
struct RawFindingResponse {
    severity: String,
    file: Option<String>,
    line_start: Option<u32>,
    line_end: Option<u32>,
    message: String,
    confidence: String,
}

impl From<RawFindingResponse> for RawFinding {
    fn from(value: RawFindingResponse) -> Self {
        RawFinding {
            severity: match value.severity.as_str() {
                "Critical" => Severity::Critical,
                "Warning" => Severity::Warning,
                _ => Severity::Info,
            },
            file: value.file.map(|f| f.into()),
            line_start: value.line_start,
            line_end: value.line_end,
            message: value.message,
            confidence: match value.confidence.as_str() {
                "High" => Confidence::High,
                "Medium" => Confidence::Medium,
                _ => Confidence::Low,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct AutoFixResponse {
    unified_diff: String,
    explanation: String,
}
