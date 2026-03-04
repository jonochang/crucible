use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use crate::analysis::{AgentContext, FocusAreas};
use crate::config::CliPluginConfig;
use crate::plugin::{AgentPlugin, FocusAnalyzer};
use crate::report::{AutoFix, Finding, RawFinding, Severity, Confidence};
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
            .stdin(if is_gemini { Stdio::null() } else { Stdio::piped() })
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
                            eprintln!("crucible: {} parsed JSON from Claude envelope", self.command);
                        }
                        return Ok(parsed);
                    }
                }
                if is_gemini {
                    if let Some(parsed) = parse_gemini_envelope(&stdout) {
                        if is_verbose() {
                            eprintln!("crucible: {} parsed JSON from Gemini envelope", self.command);
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
