use crate::analysis::{AgentContext, FocusAreas};
use crate::consensus::{ConsensusAnchor, ConsensusItem, ItemImportance, TaskContext};
use crate::config::CliPluginConfig;
use crate::plugin::{
    AgentPlugin, AgentReviewOutput, ConvergenceDecision, FocusAnalyzer, GenericAgentOutput,
    GenericFinalOutput, RawConsensusItem,
};
use crate::progress::{ProgressEvent, TranscriptDirection};
use crate::report::{
    AutoFix, CanonicalIssue, Confidence, EvidenceAnchor, Finding, RawFinding, Severity,
};
use crate::task_pack::{PromptTemplate, TaskPack, TaskPackRole};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
pub struct CliAgentPlugin {
    id: String,
    persona: String,
    command: String,
    args: Vec<String>,
    reviewer_focus: String,
    prompt_template: PromptTemplate,
}

static VERBOSE: AtomicBool = AtomicBool::new(false);
static DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);
static DEBUG_FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();
static PROGRESS_TX: OnceLock<Mutex<Option<tokio::sync::mpsc::UnboundedSender<ProgressEvent>>>> =
    OnceLock::new();

pub fn set_verbose(enabled: bool) {
    VERBOSE.store(enabled, Ordering::Relaxed);
}

pub fn set_debug_log(path: &Path) -> Result<()> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let _ = DEBUG_FILE.set(Mutex::new(file));
    DEBUG_ENABLED.store(true, Ordering::Relaxed);
    debug_log_line("debug logging enabled");
    Ok(())
}

pub fn set_progress_sender(
    tx: Option<tokio::sync::mpsc::UnboundedSender<ProgressEvent>>,
) -> Result<()> {
    let lock = PROGRESS_TX.get_or_init(|| Mutex::new(None));
    let mut guard = lock.lock().map_err(|_| anyhow!("progress sender lock poisoned"))?;
    *guard = tx;
    Ok(())
}

impl CliAgentPlugin {
    pub fn from_role(id: &str, _plugin_id: &str, cfg: &CliPluginConfig, role: &TaskPackRole) -> Self {
        Self {
            id: id.to_string(),
            persona: role.persona.clone(),
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            reviewer_focus: role.focus.clone(),
            prompt_template: role.prompt_template.clone(),
        }
    }

    fn run_cli<T: for<'de> Deserialize<'de>>(&self, system: String, user: String) -> Result<T> {
        let is_claude = self.command == "claude";
        let is_gemini = self.command == "gemini";
        let is_opencode = self.command == "opencode";
        debug_log_line(&format!(
            "[{}] invoking {} {:?}",
            self.id, self.command, self.args
        ));
        if is_verbose() {
            debug_log_line(&format!("[{}] system prompt:\n{}", self.id, system));
            debug_log_line(&format!("[{}] user prompt:\n{}", self.id, user));
        } else {
            debug_log_line(&format!(
                "[{}] prompts omitted in debug mode (enable --verbose to include full prompts/diff)",
                self.id
            ));
        }
        emit_transcript_event(&self.id, TranscriptDirection::ToAgent, &preview_outbound(&user));
        let prompt = if is_claude {
            user
        } else if is_gemini {
            format!("System: {}\n\nuser: {}", system, user)
        } else if is_opencode {
            format!("{system}\n\n---\n\n{user}")
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
        if is_opencode {
            let has_run = self.args.iter().any(|a| a == "run");
            let has_prompt_flag = self.args.iter().any(|a| a == "-p" || a == "--prompt");
            let has_format_flag = self
                .args
                .iter()
                .any(|a| a == "-f" || a == "--output-format" || a == "--format");
            if has_run {
                if !has_format_flag {
                    cmd.args(["--format", "json"]);
                }
                cmd.arg(&prompt);
            } else {
                if !has_prompt_flag {
                    cmd.args(["-p", &prompt]);
                }
                if !has_format_flag {
                    cmd.args(["-f", "json"]);
                }
            }
        }

        let mut child = cmd
            .stdin(if is_gemini || is_opencode {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn {}", self.command))?;

        if !is_gemini && !is_opencode {
            if let Some(stdin) = child.stdin.as_mut() {
                use std::io::Write;
                stdin.write_all(prompt.as_bytes()).context("write prompt")?;
            }
        }

        let output = child.wait_with_output().context("wait for CLI output")?;
        if !output.status.success() {
            debug_log_line(&format!(
                "[{}] command failed with stderr:\n{}",
                self.id,
                String::from_utf8_lossy(&output.stderr)
            ));
            return Err(anyhow!(
                "{} failed: {}",
                self.command,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        debug_log_line(&format!("[{}] stdout:\n{}", self.id, stdout));
        if !stderr.trim().is_empty() {
            debug_log_line(&format!("[{}] stderr:\n{}", self.id, stderr));
        }
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
                if is_opencode {
                    if let Some(parsed) = parse_opencode_event_stream(&stdout) {
                        if is_verbose() {
                            eprintln!(
                                "crucible: {} parsed JSON from OpenCode event stream",
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
                    debug_log_line(&format!(
                        "[{}] parse failure: {}\nstdout:\n{}",
                        self.id, err, snippet
                    ));
                    return Err(anyhow!(
                        "parse JSON from CLI failed: {err}\nstdout (truncated):\n{snippet}\nrun with --verbose to include raw stderr"
                    ));
                }
            }
        };
        emit_transcript_event(
            &self.id,
            TranscriptDirection::FromAgent,
            &preview_inbound(&stdout),
        );
        debug_log_line(&format!("[{}] parsed response successfully", self.id));
        Ok(parsed)
    }

    fn build_context_sections(&self, ctx: &AgentContext) -> (String, String, String, String) {
        let focus = ctx
            .focus
            .as_ref()
            .map(|f| serde_json::to_string(f).unwrap_or_default())
            .unwrap_or_else(|| "null".to_string());
        let references = serde_json::to_string(&ctx.gathered.references).unwrap_or_default();
        let history = serde_json::to_string(&ctx.gathered.history).unwrap_or_default();
        let prechecks = serde_json::to_string(&ctx.gathered.prechecks).unwrap_or_default();
        (focus, references, history, prechecks)
    }

    fn review_prompt_round1(&self, ctx: &AgentContext) -> (String, String) {
        let (focus, references, history, prechecks) = self.build_context_sections(ctx);
        let role_focus = self.reviewer_focus.as_str();
        let pack_title = ctx
            .review_pack
            .as_ref()
            .map(|pack| pack.manifest.title.as_str())
            .unwrap_or("code review");
        let reviewer_prompt = ctx
            .review_pack
            .as_ref()
            .map(|pack| pack.reviewer_prompt.as_str())
            .unwrap_or(
                "You MUST review all changed files/functions and assess correctness, security, performance, error handling, edge cases, and maintainability.",
            );
        let round_label = match self.prompt_template {
            PromptTemplate::Verify => "verification round-1",
            PromptTemplate::Challenge => "challenge round-1",
            _ => "exhaustive round-1",
        };
        let system = format!(
            "You are a {} performing a {} {}.\n\
            {}\n\
            Your primary lens is: {}.\n\
            Every finding MUST include direct evidence with exact code location and short quote snippets.\n\
            Respond ONLY with valid JSON matching this schema:\n\
            {{\n  \"narrative\": \"<concise analysis for terminal display>\",\n  \"findings\": [{{\n    \"severity\": \"Critical | Warning | Info\",\n    \"category\": \"<correctness|security|performance|maintainability|testing|style>\",\n    \"file\": \"<relative path or null>\",\n    \"line_start\": <integer or null>,\n    \"line_end\": <integer or null>,\n    \"title\": \"<short issue title>\",\n    \"description\": \"<detailed issue description>\",\n    \"message\": \"<concise actionable issue>\",\n    \"suggested_fix\": \"<recommended fix or null>\",\n    \"evidence\": [{{\"location\":\"<path:line>\",\"quote\":\"<short code excerpt>\"}}],\n    \"confidence\": \"High | Medium | Low\"\n  }}]\n}}",
            self.persona, round_label, pack_title, reviewer_prompt, role_focus
        );
        let user = format!(
            "Round: 1\nReview context:\n- Suggested focus areas: {}\n- Relevant references: {}\n- Recent commit history: {}\n- Deterministic precheck signals: {}\n\nDiff to review:\n{}",
            focus, references, history, prechecks, ctx.diff
        );
        (system, user)
    }

    fn review_prompt_round_n(
        &self,
        ctx: &AgentContext,
        round: u8,
        synthesis: &crate::coordinator::CrossPollinationSynthesis,
    ) -> (String, String) {
        let (focus, references, history, prechecks) = self.build_context_sections(ctx);
        let role_focus = self.reviewer_focus.as_str();
        let pack_title = ctx
            .review_pack
            .as_ref()
            .map(|pack| pack.manifest.title.as_str())
            .unwrap_or("review");
        let reviewer_prompt = ctx
            .review_pack
            .as_ref()
            .map(|pack| pack.reviewer_prompt.as_str())
            .unwrap_or(
                "You MUST use only prior-round discussion below (no same-round leakage), explicitly agree/disagree with prior points, and call out missed issues if any.",
            );
        let round_label = match self.prompt_template {
            PromptTemplate::Verify => "verification",
            _ => "adversarial",
        };
        let system = format!(
            "You are a {} performing {} round-{} {}.\n\
            {}\n\
            Your primary lens is: {}.\n\
            Every finding MUST include direct evidence with exact code location and short quote snippets.\n\
            Respond ONLY with valid JSON matching this schema:\n\
            {{\n  \"narrative\": \"<concise agreement/disagreement summary>\",\n  \"findings\": [{{\n    \"severity\": \"Critical | Warning | Info\",\n    \"category\": \"<correctness|security|performance|maintainability|testing|style>\",\n    \"file\": \"<relative path or null>\",\n    \"line_start\": <integer or null>,\n    \"line_end\": <integer or null>,\n    \"title\": \"<short issue title>\",\n    \"description\": \"<detailed issue description>\",\n    \"message\": \"<concise actionable issue>\",\n    \"suggested_fix\": \"<recommended fix or null>\",\n    \"evidence\": [{{\"location\":\"<path:line>\",\"quote\":\"<short code excerpt>\"}}],\n    \"confidence\": \"High | Medium | Low\"\n  }}]\n}}",
            self.persona, round_label, round, pack_title, reviewer_prompt, role_focus
        );
        // Omit the full diff in round-N: agents already reviewed it in round 1.
        // Include a compact summary of all changed files to anchor discussion.
        let changed_files_summary = ctx.diff.lines()
            .filter(|l| l.starts_with("diff --git "))
            .collect::<Vec<_>>()
            .join("\n");
        let user = format!(
            "Round: {}\nPrior round reviewer discussion:\n{}\n\nReview context:\n- Suggested focus areas: {}\n- Relevant references: {}\n- Recent commit history: {}\n- Deterministic precheck signals: {}\n\nChanged files (for reference; full diff was provided in round 1):\n{}",
            round, synthesis.summary, focus, references, history, prechecks, changed_files_summary
        );
        (system, user)
    }

    fn task_context_json(&self, ctx: &TaskContext) -> String {
        let payload = serde_json::json!({
            "prompt": ctx.prompt,
            "attachments": ctx.attachments,
            "docs": ctx.docs,
            "history": ctx.history,
            "prechecks": ctx.prechecks,
            "clarifications": ctx.clarification_history,
            "analyzer_summary": ctx.analyzer_summary,
        });
        serde_json::to_string(&payload).unwrap_or_default()
    }

    fn task_prompt_round1(&self, ctx: &TaskContext, pack: &TaskPack) -> (String, String) {
        let system = format!(
            "You are a {} contributing to a multi-agent consensus task.\n\
             Task pack: {} - {}.\n\
             {}\n\
             Your primary lens is: {}.\n\
             Respond ONLY with valid JSON matching this schema:\n\
             {{\"narrative\":\"<concise summary>\",\"items\":[{{\"kind\":\"risk|decision|question|recommendation|gap\",\"importance\":\"high|medium|low\",\"title\":\"<short title>\",\"message\":\"<actionable message>\",\"confidence\":\"High|Medium|Low\",\"anchors\":[{{\"attachment_id\":\"<attachment id>\",\"quote\":\"<short supporting quote>\"}}]}}]}}",
            self.persona,
            pack.manifest.title,
            pack.manifest.description,
            pack.reviewer_prompt,
            self.reviewer_focus
        );
        let user = format!(
            "Round: 1\nTask context:\n{}\n\nExpected final schema:\n{}\n",
            self.task_context_json(ctx),
            pack.schema_json
        );
        (system, user)
    }

    fn task_prompt_round_n(
        &self,
        ctx: &TaskContext,
        pack: &TaskPack,
        round: u8,
        prior_summary: &str,
    ) -> (String, String) {
        let system = format!(
            "You are a {} contributing to round-{} of a multi-agent consensus task.\n\
             Task pack: {} - {}.\n\
             {}\n\
             Your primary lens is: {}.\n\
             Explicitly agree or disagree with prior points, surface missed issues, and preserve unresolved disagreements.\n\
             Respond ONLY with valid JSON matching this schema:\n\
             {{\"narrative\":\"<agreement/disagreement summary>\",\"items\":[{{\"kind\":\"risk|decision|question|recommendation|gap\",\"importance\":\"high|medium|low\",\"title\":\"<short title>\",\"message\":\"<actionable message>\",\"confidence\":\"High|Medium|Low\",\"anchors\":[{{\"attachment_id\":\"<attachment id>\",\"quote\":\"<short supporting quote>\"}}]}}]}}",
            self.persona,
            round,
            pack.manifest.title,
            pack.manifest.description,
            pack.reviewer_prompt,
            self.reviewer_focus
        );
        let user = format!(
            "Round: {round}\nPrior consensus summary:\n{prior_summary}\n\nTask context:\n{}\n\nExpected final schema:\n{}\n",
            self.task_context_json(ctx),
            pack.schema_json
        );
        (system, user)
    }
}

fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

fn debug_log_line(message: &str) {
    if !DEBUG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(lock) = DEBUG_FILE.get() else {
        return;
    };
    if let Ok(mut file) = lock.lock() {
        let _ = writeln!(file, "[{}] {}", timestamp_string(), message);
        let _ = file.flush();
    }
}

fn timestamp_string() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}.{:03}", d.as_secs(), d.subsec_millis()),
        Err(_) => "0.000".to_string(),
    }
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
            if let Some(parsed) = parse_jsonish_text(result) {
                return Some(parsed);
            }
        }
    }
    None
}

fn parse_gemini_envelope<T: for<'de> Deserialize<'de>>(input: &str) -> Option<T> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let response = value.get("response")?;
    match response {
        serde_json::Value::String(text) => parse_jsonish_text(text),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
            serde_json::from_value(response.clone()).ok()
        }
        _ => None,
    }
}

fn parse_opencode_event_stream<T: for<'de> Deserialize<'de>>(input: &str) -> Option<T> {
    if let Ok(parsed) = serde_json::from_str::<T>(input) {
        return Some(parsed);
    }

    let mut text_parts = Vec::new();
    for line in input.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        match value.get("type").and_then(|v| v.as_str()) {
            Some("text") => {
                if let Some(text) = value
                    .get("part")
                    .and_then(|part| part.get("text"))
                    .and_then(|v| v.as_str())
                {
                    text_parts.push(text.to_string());
                }
            }
            Some("response") => {
                if let Some(response) = value.get("response") {
                    if let Ok(parsed) = serde_json::from_value::<T>(response.clone()) {
                        return Some(parsed);
                    }
                    if let Some(text) = response.as_str() {
                        if let Some(parsed) = parse_jsonish_text(text) {
                            return Some(parsed);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if text_parts.is_empty() {
        None
    } else {
        parse_jsonish_text(&text_parts.join(""))
    }
}

fn parse_jsonish_text<T: for<'de> Deserialize<'de>>(input: &str) -> Option<T> {
    serde_json::from_str::<T>(input)
        .ok()
        .or_else(|| {
            extract_fenced_json_block(input)
                .and_then(|candidate| serde_json::from_str::<T>(candidate.as_str()).ok())
        })
        .or_else(|| {
            extract_fenced_json_block(input).and_then(|candidate| parse_json_from_mixed(&candidate))
        })
        .or_else(|| parse_json_from_mixed(input))
}

fn extract_fenced_json_block(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.starts_with("```") {
        return Some(strip_fenced_json(trimmed));
    }

    let start = trimmed.find("```")?;
    let rest = &trimmed[start..];
    let end = rest[3..].find("```")?;
    let fenced = &rest[..end + 6];
    Some(strip_fenced_json(fenced))
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

fn emit_transcript_event(id: &str, direction: TranscriptDirection, message: &str) {
    let Some(lock) = PROGRESS_TX.get() else {
        return;
    };
    let Ok(guard) = lock.lock() else {
        return;
    };
    let Some(tx) = guard.as_ref() else {
        return;
    };
    let _ = tx.send(ProgressEvent::AgentTranscript {
        id: id.to_string(),
        direction,
        message: truncate(&sanitize_terminal_output(message), 180),
    });
}

fn preview_outbound(user: &str) -> String {
    let compact = user
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !matches!(
                    *line,
                    "Diff to review:" | "Review context:" | "Prior round reviewer discussion:"
                )
                && !line.starts_with("Round: ")
                && !line.starts_with("- Relevant references:")
                && !line.starts_with("- Recent commit history:")
                && !line.starts_with("- Deterministic precheck signals:")
                && !line.starts_with("diff --git ")
                && !line.starts_with("@@")
                && !line.starts_with('+')
                && !line.starts_with('-')
        })
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    if compact.is_empty() {
        "sending review prompt".to_string()
    } else {
        compact
    }
}

fn preview_inbound(stdout: &str) -> String {
    let sanitized = sanitize_terminal_output(stdout);
    if let Some(value) = parse_transcript_preview_json(&sanitized) {
        return value;
    }
    let compact = sanitized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    if compact.is_empty() {
        "received empty response".to_string()
    } else {
        compact
    }
}

fn parse_transcript_preview_json(input: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    if let Some(summary) = value.get("narrative").and_then(|v| v.as_str()) {
        let summary = summary.trim();
        if !summary.is_empty() {
            return Some(summary.to_string());
        }
    }
    if let Some(summary) = value.get("summary").and_then(|v| v.as_str()) {
        let summary = summary.trim();
        if !summary.is_empty() {
            return Some(summary.to_string());
        }
    }
    if let Some(explanation) = value.get("explanation").and_then(|v| v.as_str()) {
        let explanation = explanation.trim();
        if !explanation.is_empty() {
            return Some(explanation.to_string());
        }
    }
    if let Some(findings) = value.get("findings").and_then(|v| v.as_array()) {
        if let Some(message) = findings
            .first()
            .and_then(|item| item.get("message"))
            .and_then(|v| v.as_str())
        {
            return Some(message.trim().to_string());
        }
        return Some(format!("{} findings returned", findings.len()));
    }
    None
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
        let judge_prompt = ctx
            .review_pack
            .as_ref()
            .map(|pack| pack.judge_prompt.as_str())
            .unwrap_or("Produce final review consensus and auto-fix guidance for agreed findings.");
        let system = format!(
            "You are a {} producing an auto-fix for agreed findings.\n{}\nPrimary lens: {}.\nRespond ONLY with valid JSON: {{ \"unified_diff\": \"...\", \"explanation\": \"...\" }}",
            self.persona, judge_prompt, self.reviewer_focus
        );
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

    async fn judge_convergence(
        &self,
        ctx: &AgentContext,
        round: u8,
        findings: &[Finding],
    ) -> Result<ConvergenceDecision> {
        let judge_prompt = ctx
            .review_pack
            .as_ref()
            .map(|pack| pack.judge_prompt.as_str())
            .unwrap_or("Produce final review consensus and convergence judgments for agreed findings.");
        let system = format!(
            "You are a strict convergence judge for multi-agent code review.\n{}\nPrimary lens: {}.\nRespond ONLY with valid JSON: {{\"verdict\":\"CONVERGED|NOT_CONVERGED\",\"rationale\":\"...\"}}.\nUse CONVERGED only when there are no unresolved material disagreements and no net-new high-severity risk requiring another round.",
            judge_prompt, self.reviewer_focus
        );
        let user = format!(
            "Round: {round}\nFindings so far:\n{}\n\nDiff:\n{}",
            serde_json::to_string(findings).unwrap_or_default(),
            ctx.diff
        );
        let resp: ConvergenceResponse = self.run_cli(system, user)?;
        let verdict = match resp.verdict.as_str() {
            "CONVERGED" => crate::progress::ConvergenceVerdict::Converged,
            _ => crate::progress::ConvergenceVerdict::NotConverged,
        };
        Ok(ConvergenceDecision {
            verdict,
            rationale: resp.rationale,
        })
    }

    async fn structurize_issues(
        &self,
        ctx: &AgentContext,
        findings: &[Finding],
    ) -> Result<Vec<CanonicalIssue>> {
        let judge_prompt = ctx
            .review_pack
            .as_ref()
            .map(|pack| pack.judge_prompt.as_str())
            .unwrap_or("Produce final review consensus and canonical issues for agreed findings.");
        let system = format!(
            "You are an issue structurizer.\n{}\nPrimary lens: {}.\nRespond ONLY with valid JSON array where each item contains: severity, category, file, line_start, line_end, title, description, suggested_fix, raised_by.",
            judge_prompt, self.reviewer_focus
        );
        let user = format!(
            "Normalize these findings into canonical issues (merge duplicates):\n{}",
            serde_json::to_string(findings).unwrap_or_default()
        );
        let resp: Vec<CanonicalIssueResponse> = self.run_cli(system, user)?;
        Ok(resp.into_iter().map(CanonicalIssue::from).collect())
    }

    async fn analyze_task(&self, ctx: &TaskContext, pack: &TaskPack) -> Result<GenericAgentOutput> {
        let (system, user) = self.task_prompt_round1(ctx, pack);
        let resp: GenericItemsResponse = self.run_cli(system, user)?;
        Ok(resp.into())
    }

    async fn debate_task(
        &self,
        ctx: &TaskContext,
        pack: &TaskPack,
        round: u8,
        prior_summary: &str,
    ) -> Result<GenericAgentOutput> {
        let (system, user) = self.task_prompt_round_n(ctx, pack, round, prior_summary);
        let resp: GenericItemsResponse = self.run_cli(system, user)?;
        Ok(resp.into())
    }

    async fn summarize_task(
        &self,
        ctx: &TaskContext,
        pack: &TaskPack,
        agreed_items: &[ConsensusItem],
        unresolved_items: &[ConsensusItem],
    ) -> Result<GenericFinalOutput> {
        let system = format!(
            "You are the final judge for a multi-agent consensus task.\n\
             Task pack: {} - {}.\n\
             {}\n\
             Primary lens: {}.\n\
             Respond ONLY with valid JSON matching this schema:\n\
             {{\"summary_markdown\":\"<markdown summary>\",\"result\":<json object matching schema>,\"clarification_requests\":[\"<question>\"]}}",
            pack.manifest.title, pack.manifest.description, pack.judge_prompt, self.reviewer_focus
        );
        let user = format!(
            "Task context:\n{}\n\nAgreed items:\n{}\n\nUnresolved items:\n{}\n\nExpected final schema:\n{}",
            self.task_context_json(ctx),
            serde_json::to_string(agreed_items).unwrap_or_default(),
            serde_json::to_string(unresolved_items).unwrap_or_default(),
            pack.schema_json
        );
        let resp: GenericFinalResponse = self.run_cli(system, user)?;
        Ok(GenericFinalOutput {
            summary_markdown: resp.summary_markdown,
            result_json: resp.result,
            clarification_requests: resp.clarification_requests,
        })
    }

    async fn judge_task_convergence(
        &self,
        ctx: &TaskContext,
        round: u8,
        items: &[ConsensusItem],
    ) -> Result<ConvergenceDecision> {
        let system = format!(
            "You are a strict convergence judge for a multi-agent consensus task.\nPrimary lens: {}.\nRespond ONLY with valid JSON: {{\"verdict\":\"CONVERGED|NOT_CONVERGED\",\"rationale\":\"...\"}}.",
            self.reviewer_focus
        );
        let user = format!(
            "Round: {round}\nTask context:\n{}\n\nConsensus items so far:\n{}",
            self.task_context_json(ctx),
            serde_json::to_string(items).unwrap_or_default()
        );
        let resp: ConvergenceResponse = self.run_cli(system, user)?;
        let verdict = match resp.verdict.as_str() {
            "CONVERGED" => crate::progress::ConvergenceVerdict::Converged,
            _ => crate::progress::ConvergenceVerdict::NotConverged,
        };
        Ok(ConvergenceDecision {
            verdict,
            rationale: resp.rationale,
        })
    }
}

#[async_trait]
impl FocusAnalyzer for CliAgentPlugin {
    async fn analyze_focus(&self, ctx: &AgentContext) -> Result<FocusAreas> {
        let analyzer_prompt = ctx
            .review_pack
            .as_ref()
            .map(|pack| pack.analyzer_prompt.as_str())
            .unwrap_or("You are a senior architect producing analyzer context for code review.");
        let system = format!(
            "{}\nPrimary lens: {}.\nOutput ONLY valid JSON matching this schema:\n{{\n  \"summary\": \"<what changed, architecture impact, and purpose>\",\n  \"focus_items\": [{{ \"area\": \"<review area>\", \"rationale\": \"<why this needs focus>\" }}],\n  \"trade_offs\": [\"<trade-off or risk>\"],\n  \"affected_modules\": [\"<module/component impacted>\"],\n  \"call_chain\": [\"<entrypoint -> downstream path>\"],\n  \"design_patterns\": [\"<pattern used or violated>\"],\n  \"reviewer_checklist\": [\"<targeted checklist item for reviewers>\"]\n}}\nThe summary must be markdown-ready text.",
            analyzer_prompt, self.reviewer_focus
        );
        let user = format!("Diff to review:\n{}", ctx.diff);
        let resp: FocusAreas = self.run_cli(system, user)?;
        Ok(resp)
    }

    async fn analyze_task_focus(&self, ctx: &TaskContext) -> Result<FocusAreas> {
        let system = format!(
            "You are a senior analyst preparing focus areas for a generic multi-agent consensus task.\nPrimary lens: {}.\nOutput ONLY valid JSON matching this schema:\n{{\n  \"summary\": \"<task summary>\",\n  \"focus_items\": [{{ \"area\": \"<focus area>\", \"rationale\": \"<why it matters>\" }}],\n  \"trade_offs\": [\"<trade-off or risk>\"],\n  \"affected_modules\": [\"<artifact or subsystem>\"],\n  \"call_chain\": [\"<important flow>\"],\n  \"design_patterns\": [\"<pattern or structure>\"],\n  \"reviewer_checklist\": [\"<checklist item>\"]\n}}",
            self.reviewer_focus
        );
        let user = format!(
            "Task prompt:\n{}\n\nAttachments and context:\n{}",
            ctx.prompt,
            self.task_context_json(ctx)
        );
        let resp: FocusAreas = self.run_cli(system, user)?;
        Ok(resp)
    }
}

#[derive(Debug, Deserialize)]
struct FindingsResponse {
    #[serde(default)]
    narrative: Option<String>,
    findings: Vec<RawFindingResponse>,
}

#[derive(Debug, Deserialize)]
struct RawFindingResponse {
    severity: String,
    #[serde(default)]
    category: Option<String>,
    file: Option<String>,
    line_start: Option<u32>,
    line_end: Option<u32>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    message: String,
    #[serde(default)]
    suggested_fix: Option<String>,
    #[serde(default)]
    evidence: Vec<EvidenceAnchor>,
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
            category: value.category,
            title: value.title,
            description: value.description,
            suggested_fix: value.suggested_fix,
            evidence: value.evidence,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AutoFixResponse {
    unified_diff: String,
    explanation: String,
}

#[derive(Debug, Deserialize)]
struct ConvergenceResponse {
    verdict: String,
    rationale: String,
}

#[derive(Debug, Deserialize)]
struct CanonicalIssueResponse {
    severity: String,
    category: String,
    file: Option<String>,
    line_start: Option<u32>,
    line_end: Option<u32>,
    title: String,
    description: String,
    #[serde(default)]
    suggested_fix: Option<String>,
    #[serde(default)]
    raised_by: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GenericItemsResponse {
    #[serde(default)]
    narrative: Option<String>,
    items: Vec<GenericItemResponse>,
}

#[derive(Debug, Deserialize)]
struct GenericItemResponse {
    kind: String,
    importance: String,
    title: String,
    message: String,
    confidence: String,
    #[serde(default)]
    anchors: Vec<ConsensusAnchor>,
}

impl From<GenericItemsResponse> for GenericAgentOutput {
    fn from(value: GenericItemsResponse) -> Self {
        Self {
            narrative: value.narrative.unwrap_or_default(),
            items: value.items.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<GenericItemResponse> for RawConsensusItem {
    fn from(value: GenericItemResponse) -> Self {
        Self {
            kind: value.kind,
            importance: match value.importance.to_lowercase().as_str() {
                "high" => ItemImportance::High,
                "medium" => ItemImportance::Medium,
                _ => ItemImportance::Low,
            },
            title: value.title,
            message: value.message,
            confidence: match value.confidence.as_str() {
                "High" => Confidence::High,
                "Medium" => Confidence::Medium,
                _ => Confidence::Low,
            },
            anchors: value.anchors,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GenericFinalResponse {
    summary_markdown: String,
    result: serde_json::Value,
    #[serde(default)]
    clarification_requests: Vec<String>,
}

impl From<CanonicalIssueResponse> for CanonicalIssue {
    fn from(value: CanonicalIssueResponse) -> Self {
        Self {
            severity: match value.severity.as_str() {
                "Critical" => Severity::Critical,
                "Warning" => Severity::Warning,
                _ => Severity::Info,
            },
            category: value.category,
            file: value.file.map(Into::into),
            line_start: value.line_start,
            line_end: value.line_end,
            title: value.title,
            description: value.description,
            suggested_fix: value.suggested_fix,
            raised_by: value.raised_by,
            evidence: Vec::new(),
        }
    }
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
            review_pack: None,
        }
    }

    fn sample_plugin() -> CliAgentPlugin {
        CliAgentPlugin {
            id: "codex".to_string(),
            persona: "Architecture Lead".to_string(),
            command: "codex".to_string(),
            args: vec![],
            reviewer_focus: "Architecture, maintainability, and API consistency".to_string(),
            prompt_template: PromptTemplate::Discover,
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

    #[test]
    fn findings_response_requires_findings_field() {
        let parsed = serde_json::from_str::<FindingsResponse>(r#"{"narrative":"ok"}"#);
        assert!(parsed.is_err());
    }

    #[test]
    fn findings_response_allows_explicit_empty_findings() {
        let parsed = serde_json::from_str::<FindingsResponse>(
            r#"{"narrative":"ok","findings":[]}"#,
        )
        .expect("findings response");
        assert!(parsed.findings.is_empty());
    }

    #[test]
    fn outbound_preview_skips_diff_bulk() {
        let preview = preview_outbound(
            "Round: 1\nReview context:\n- Relevant references: []\nDiff to review:\n+ massive",
        );
        assert_eq!(preview, "sending review prompt");
    }

    #[test]
    fn inbound_preview_prefers_narrative() {
        let preview = preview_inbound(r#"{"narrative":"Agent found one issue","findings":[]}"#);
        assert_eq!(preview, "Agent found one issue");
    }

    #[test]
    fn parse_opencode_event_stream_extracts_fenced_json_findings() {
        let input = concat!(
            "{\"type\":\"step_start\",\"part\":{\"type\":\"step-start\"}}\n",
            "{\"type\":\"text\",\"part\":{\"text\":\"```json\\n{\\\"narrative\\\":\\\"done\\\",\\\"findings\\\":[]}\\n```\"}}\n",
            "{\"type\":\"step_finish\",\"part\":{\"type\":\"step-finish\",\"reason\":\"stop\"}}\n"
        );
        let parsed = parse_opencode_event_stream::<FindingsResponse>(input).expect("findings");
        assert_eq!(parsed.narrative.as_deref(), Some("done"));
        assert!(parsed.findings.is_empty());
    }

    #[test]
    fn parse_claude_envelope_extracts_fenced_json_after_prose() {
        let input = r#"{
          "type":"result",
          "result":"Now I have enough context.\n\n```json\n{\"summary\":\"ok\",\"focus_items\":[],\"trade_offs\":[],\"affected_modules\":[],\"call_chain\":[],\"design_patterns\":[],\"reviewer_checklist\":[]}\n```"
        }"#;
        let parsed = parse_claude_envelope::<FocusAreas>(input).expect("focus areas");
        assert_eq!(parsed.summary, "ok");
    }

    #[test]
    fn parse_claude_envelope_extracts_findings_from_fenced_json_after_prose() {
        let input = r#"{
          "type":"result",
          "result":"Let me produce the findings.\n\n```json\n{\"narrative\":\"done\",\"findings\":[]}\n```"
        }"#;
        let parsed = parse_claude_envelope::<FindingsResponse>(input).expect("findings");
        assert_eq!(parsed.narrative.as_deref(), Some("done"));
        assert!(parsed.findings.is_empty());
    }
}
