use crate::analysis::{AgentContext, FocusAreas};
use crate::config::CliPluginConfig;
use crate::plugin::{AgentPlugin, AgentReviewOutput, FocusAnalyzer};
use crate::report::{AutoFix, Confidence, Finding, RawFinding, Severity};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct CliAgentPlugin {
    id: String,
    persona: String,
    command: String,
    args: Vec<String>,
}

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(enabled: bool) {
    VERBOSE.store(enabled, Ordering::Relaxed);
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
        let is_claude = self.command == "claude";
        let is_gemini = self.command == "gemini";
        let prompt = if is_claude {
            user
        } else if is_gemini {
            format!("System: {}\n\nuser: {}", system, user)
        } else {
            format!("[SYSTEM]\n{}\n\n[USER]\n{}\n", system, user)
        };

        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        if is_claude {
            if !self.args.iter().any(|a| a == "-p" || a == "--print") {
                cmd.arg("-p");
            }
            if !self.args.iter().any(|a| a == "--output-format") {
                cmd.args(["--output-format", "json"]);
            }
            cmd.args(["--system-prompt", &system]);
        }
        if is_gemini {
            if !self.args.iter().any(|a| a == "-y" || a == "--yes") {
                cmd.arg("-y");
            }
            if !self.args.iter().any(|a| a == "-o" || a == "--output") {
                cmd.args(["-o", "json"]);
            }
            cmd.arg(&prompt);
        }

        let mut child = cmd
            .stdin(if is_gemini {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {}", self.command))?;

        if !is_gemini {
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                stdin.write_all(prompt.as_bytes()).context("write prompt")?;
            }
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
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if is_verbose() {
            eprintln!(
                "crucible: {} raw stdout:\n{}",
                self.command,
                sanitize_terminal_output(&stdout)
            );
            if !stderr.trim().is_empty() {
                eprintln!(
                    "crucible: {} raw stderr:\n{}",
                    self.command,
                    sanitize_terminal_output(&stderr)
                );
            }
        }
        let parsed: T = match serde_json::from_str(&stdout) {
            Ok(val) => val,
            Err(err) => {
                if is_claude {
                    if let Some(parsed) = parse_claude_envelope(&stdout) {
                        if is_verbose() {
                            eprintln!(
                                "crucible: {} parsed JSON from Claude envelope",
                                self.command
                            );
                        }
                        return Ok(parsed);
                    }
                }
                if is_gemini {
                    if let Some(parsed) = parse_gemini_envelope(&stdout) {
                        if is_verbose() {
                            eprintln!(
                                "crucible: {} parsed JSON from Gemini envelope",
                                self.command
                            );
                        }
                        return Ok(parsed);
                    }
                }
                if let Some(parsed) = parse_json_from_mixed(&stdout) {
                    if is_verbose() {
                        eprintln!("crucible: {} parsed JSON from mixed output", self.command);
                    } else {
                        eprintln!(
                            "crucible warning: {} returned mixed output; parsed JSON fallback was used (rerun with --verbose to inspect raw output)",
                            self.command
                        );
                    }
                    parsed
                } else {
                    let snippet = truncate(&stdout, 2000);
                    return Err(anyhow!(
                        "parse JSON from CLI failed: {err}\nstdout (truncated):\n{snippet}\nrun with --verbose to include raw stderr"
                    ));
                }
            }
        };
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

    fn review_prompt_round1(&self, ctx: &AgentContext) -> (String, String) {
        let (focus, references, history) = self.build_context_sections(ctx);
        let system = format!(
            "You are a {} performing an exhaustive round-1 code review.\n\
            You MUST review all changed files/functions and assess correctness, security, performance, error handling, edge cases, and maintainability.\n\
            Respond ONLY with valid JSON matching this schema:\n\
            {{\n  \"narrative\": \"<concise analysis for terminal display>\",\n  \"findings\": [{{\n    \"severity\": \"Critical | Warning | Info\",\n    \"file\": \"<relative path or null>\",\n    \"line_start\": <integer or null>,\n    \"line_end\": <integer or null>,\n    \"message\": \"<concise actionable issue>\",\n    \"confidence\": \"High | Medium | Low\"\n  }}]\n}}",
            self.persona
        );
        let user = format!(
            "Round: 1\nReview context:\n- Suggested focus areas: {}\n- Relevant references: {}\n- Recent commit history: {}\n\nDiff to review:\n{}",
            focus, references, history, ctx.diff
        );
        (system, user)
    }

    fn review_prompt_round_n(
        &self,
        ctx: &AgentContext,
        round: u8,
        synthesis: &crate::coordinator::CrossPollinationSynthesis,
    ) -> (String, String) {
        let (focus, references, history) = self.build_context_sections(ctx);
        let system = format!(
            "You are a {} performing adversarial round-{} review.\n\
            You MUST use only prior-round discussion below (no same-round leakage), explicitly agree/disagree with prior points, and call out missed issues if any.\n\
            Respond ONLY with valid JSON matching this schema:\n\
            {{\n  \"narrative\": \"<concise agreement/disagreement summary>\",\n  \"findings\": [{{\n    \"severity\": \"Critical | Warning | Info\",\n    \"file\": \"<relative path or null>\",\n    \"line_start\": <integer or null>,\n    \"line_end\": <integer or null>,\n    \"message\": \"<concise actionable issue>\",\n    \"confidence\": \"High | Medium | Low\"\n  }}]\n}}",
            self.persona, round
        );
        let user = format!(
            "Round: {}\nPrior round reviewer discussion:\n{}\n\nReview context:\n- Suggested focus areas: {}\n- Relevant references: {}\n- Recent commit history: {}\n\nDiff to review:\n{}",
            round, synthesis.summary, focus, references, history, ctx.diff
        );
        (system, user)
    }
}

fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

fn parse_json_from_mixed<T: for<'de> Deserialize<'de>>(input: &str) -> Option<T> {
    let mut last: Option<T> = None;
    for (idx, ch) in input.char_indices() {
        if ch != '{' {
            continue;
        }
        let slice = &input[idx..];
        let mut de = serde_json::Deserializer::from_str(slice);
        let value = match serde_json::Value::deserialize(&mut de) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if !value.is_object() {
            continue;
        }
        let parsed = match serde_json::from_value(value) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };
        last = Some(parsed);
    }
    last
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        return input.to_string();
    }
    let mut s = String::new();
    for (i, ch) in input.chars().enumerate() {
        if i >= max {
            break;
        }
        s.push(ch);
    }
    s.push_str("\n…");
    s
}

fn parse_claude_envelope<T: for<'de> Deserialize<'de>>(input: &str) -> Option<T> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    if let Some(structured) = value.get("structured_output") {
        return serde_json::from_value(structured.clone()).ok();
    }
    if let Some(result) = value.get("result").and_then(|r| r.as_str()) {
        if !result.trim().is_empty() {
            let candidate = strip_fenced_json(result);
            return serde_json::from_str(candidate.as_str()).ok();
        }
    }
    None
}

fn parse_gemini_envelope<T: for<'de> Deserialize<'de>>(input: &str) -> Option<T> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let response = value.get("response")?;
    match response {
        serde_json::Value::String(text) => {
            let candidate = strip_fenced_json(text);
            serde_json::from_str::<T>(&candidate)
                .ok()
                .or_else(|| parse_json_from_mixed(&candidate))
        }
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::from_value(response.clone()).ok()
        }
        _ => None,
    }
}

fn strip_fenced_json(input: &str) -> String {
    let trimmed = input.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let mut lines = rest.lines();
        let _lang = lines.next().unwrap_or("");
        let mut body = lines.collect::<Vec<_>>().join("\n");
        if let Some(stripped) = body.rfind("```") {
            body.truncate(stripped);
        }
        return body.trim().to_string();
    }
    trimmed.to_string()
}

fn sanitize_terminal_output(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if let Some('[') = chars.peek().copied() {
                chars.next();
                while let Some(next) = chars.next() {
                    if matches!(next, 'A'..='Z' | 'a'..='z') {
                        break;
                    }
                }
                continue;
            }
            if let Some(']') = chars.peek().copied() {
                chars.next();
                while let Some(next) = chars.next() {
                    if next == '\u{07}' {
                        break;
                    }
                    if next == '\u{1b}' {
                        if let Some('\\') = chars.peek().copied() {
                            chars.next();
                            break;
                        }
                    }
                }
                continue;
            }
            continue;
        }
        out.push(ch);
    }
    out
}

#[async_trait]
impl AgentPlugin for CliAgentPlugin {
    fn id(&self) -> &str {
        &self.id
    }

    fn persona(&self) -> &str {
        &self.persona
    }

    async fn analyze(&self, ctx: &AgentContext) -> Result<AgentReviewOutput> {
        let (system, user) = self.review_prompt_round1(ctx);
        let resp: FindingsResponse = self.run_cli(system, user)?;
        Ok(AgentReviewOutput {
            findings: resp.findings.into_iter().map(|f| f.into()).collect(),
            narrative: resp.narrative.unwrap_or_default(),
        })
    }

    async fn debate(
        &self,
        ctx: &AgentContext,
        round: u8,
        synthesis: &crate::coordinator::CrossPollinationSynthesis,
    ) -> Result<AgentReviewOutput> {
        let (system, user) = self.review_prompt_round_n(ctx, round, synthesis);
        let resp: FindingsResponse = self.run_cli(system, user)?;
        Ok(AgentReviewOutput {
            findings: resp.findings.into_iter().map(|f| f.into()).collect(),
            narrative: resp.narrative.unwrap_or_default(),
        })
    }

    async fn summarize(&self, ctx: &AgentContext, findings: &[Finding]) -> Result<AutoFix> {
        let system = "You are a senior engineer producing an auto-fix for agreed findings.\nRespond ONLY with valid JSON: { \"unified_diff\": \"...\", \"explanation\": \"...\" }".to_string();
        let user = format!(
            "Findings:\n{}\n\nDiff:\n{}",
            serde_json::to_string(findings).unwrap_or_default(),
            ctx.diff
        );
        let resp: AutoFixResponse = self.run_cli(system, user)?;
        Ok(AutoFix {
            unified_diff: resp.unified_diff,
            explanation: resp.explanation,
        })
    }
}

#[async_trait]
impl FocusAnalyzer for CliAgentPlugin {
    async fn analyze_focus(&self, ctx: &AgentContext) -> Result<FocusAreas> {
        let system = "You are a senior architect producing analyzer context for code review.\nOutput ONLY valid JSON matching this schema:\n{\n  \"summary\": \"<what changed, architecture impact, and purpose>\",\n  \"focus_items\": [{ \"area\": \"<review area>\", \"rationale\": \"<why this needs focus>\" }],\n  \"trade_offs\": [\"<trade-off or risk>\"]\n}\nThe summary must be markdown-ready text.".to_string();
        let user = format!("Diff to review:\n{}", ctx.diff);
        let resp: FocusAreas = self.run_cli(system, user)?;
        Ok(resp)
    }
}

#[derive(Debug, Deserialize)]
struct FindingsResponse {
    #[serde(default)]
    narrative: Option<String>,
    #[serde(default)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::GatheredContext;

    fn sample_ctx() -> AgentContext {
        AgentContext {
            diff: "diff --git a/src/lib.rs b/src/lib.rs\n+let x = 1;".to_string(),
            gathered: GatheredContext::default(),
            focus: None,
            dep_graph: None,
        }
    }

    fn sample_plugin() -> CliAgentPlugin {
        CliAgentPlugin {
            id: "codex".to_string(),
            persona: "Architecture Lead".to_string(),
            command: "codex".to_string(),
            args: vec![],
        }
    }

    #[test]
    fn round1_prompt_contract_contains_required_sections() {
        let plugin = sample_plugin();
        let ctx = sample_ctx();
        let (system, user) = plugin.review_prompt_round1(&ctx);
        let prompt = format!("{system}\n{user}");
        assert!(prompt.contains("exhaustive round-1 code review"));
        assert!(prompt.contains("correctness, security, performance"));
        assert!(prompt.contains("\"narrative\""));
        assert!(prompt.contains("\"findings\""));
    }

    #[test]
    fn round_n_prompt_contract_contains_adversarial_constraints() {
        let plugin = sample_plugin();
        let ctx = sample_ctx();
        let synthesis = crate::coordinator::CrossPollinationSynthesis {
            summary: "Round 1 prior reviewer findings".to_string(),
        };
        let (system, user) = plugin.review_prompt_round_n(&ctx, 2, &synthesis);
        let prompt = format!("{system}\n{user}");
        assert!(prompt.contains("adversarial round-2 review"));
        assert!(prompt.contains("prior-round discussion"));
        assert!(prompt.contains("no same-round leakage"));
        assert!(prompt.contains("Round 1 prior reviewer findings"));
    }
}
