# Crucible Magpie UX Parity Plan

## Context

This plan mines the provided Magpie `review --local` output and translates it into concrete, testable Crucible UX changes.

Primary user problem:
- Crucible currently shows status transitions, but not enough high-value context while running.
- In TTY mode, Crucible should not appear "stuck" after work completes.

Primary UX goals:
- Show meaningful review content while work is in progress.
- Make progress visibility obvious at phase, round, and per-agent levels.
- Exit cleanly once review is complete (unless explicitly interactive mode is requested in future).

## Target UX (Paritized from Magpie)

Crucible should present:
- Startup checks:
  - config loaded
  - diff detected
  - active reviewers
  - round settings
- Phase progress:
  - `Analyzing changes...`
  - `Round N: [agent statuses + elapsed]`
- Pre-debate context blocks:
  - `Analysis` section
  - `System Context` section
- Streaming agent output:
  - each agent's round output with structured header
  - concise summary + highlighted issues
- Round boundary feedback:
  - convergence status
  - `Round N/M complete`
- Final summary:
  - final verdict
  - deduped issues list
  - optional cost/token section (later phase)

Behavioral requirement:
- TTY mode exits automatically once final report is rendered and persisted.

## Magpie Role Model (Reference)

Magpie separates four roles:
- Analyzer: pre-review context and focus generation.
- Reviewers: independent and then adversarial multi-round reviewers.
- Judge: strict convergence judge plus final summarizer/structurizer.
- Convenor: runtime conductor for UX, phases, status, and event ordering.

Crucible parity should keep this same separation.

## Analyzer Design (Including Prompts)

Source patterns from Magpie:
- `config/init.ts` analyzer prompt requests:
  - what changed
  - architecture/design
  - purpose
  - trade-offs
  - things to note
  - suggested review focus (2-4 areas)
- `context-gatherer/prompts/analysis-prompt.ts` requests JSON output with:
  - `affectedModules`
  - `callChain`
  - `designPatterns`
  - `summary`

Target Crucible analyzer contract:
- Inputs:
  - diff
  - changed files
  - references/call-chain hints
  - history/docs context
- Outputs:
  - `analysis_markdown`: human-readable narrative for terminal
  - `focus_items`: list used to guide debaters
  - `system_context`: structured context payload for reviewers

Target analyzer prompt template:
- System intent:
  - "You are a senior engineer producing reviewer context, not final verdict."
- Required sections:
  - what changed
  - architecture and module impact
  - risk/trade-off analysis
  - explicit reviewer focus checklist
- Output format:
  - strict JSON for machine fields
  - optional markdown block for UX rendering

## Reviewer Prompt Design

Source patterns from Magpie:
- Round 1:
  - exhaustive review of every changed file/function.
  - explicit dimensions: correctness, security, performance, error handling, edge cases, maintainability.
- Round 2+:
  - reviewers see previous round discussion (not same-round leakage).
  - explicit adversarial directives:
    - continue exhaustive coverage
    - point out what others missed
    - challenge or agree with evidence
- Structured output expectation:
  - issues array with severity/category/file/line/title/description/suggestedFix
  - verdict and summary

Target Crucible reviewer prompt contract:
- Round 1 prompt:
  - independent exhaustive pass.
  - require explicit "reviewed file with no issues" notes for coverage.
- Round 2+ prompt:
  - incorporate prior round findings.
  - require explicit agreement/disagreement per key issue.
- Output:
  - structured findings JSON for parsing
  - concise reviewer narrative for live UX panel

## Judge Design (Convergence + Final)

Source patterns from Magpie:
- Convergence judge uses strict, conservative criteria:
  - same final verdict across reviewers
  - blocking issues acknowledged by all
  - no ignored unresolved concerns
  - explicit action agreement
- Judge emits verdict:
  - `CONVERGED` or `NOT_CONVERGED`
  - short reasoning displayed in UX.
- Final judge/summarizer:
  - consolidates consensus/disagreement/action items.
  - runs structured issue extraction from final review text.

Target Crucible judge contract:
- Convergence stage:
  - strict binary verdict + rationale each round.
- Final stage:
  - merged deduped issues + final recommendation.
  - clear "why" narrative for users.

Judge prompt templates to add:
- Convergence prompt:
  - strict criteria checklist.
  - mandatory one-token final verdict marker.
- Final summary prompt:
  - consensus points
  - disagreements with rationale
  - prioritized action items

## Convenor Design (Orchestrator + UX Runtime)

Source patterns from Magpie convenor behavior (`commands/review.ts` + `orchestrator.ts`):
- Emits phase transitions (`analyzer`, `round-N`, `convergence-check`, `summarizer`).
- Tracks and displays parallel reviewer status with durations.
- Shows ordered sections:
  - run header
  - analysis panel
  - system context panel
  - reviewer outputs per round
  - convergence verdict
  - final conclusion
  - issues table
- Handles stream buffering and flush ordering to avoid interleaved noisy output.

Target Crucible convenor contract:
- Own all runtime state transitions and user-visible order.
- Present consistent sectioned UX in TTY and non-TTY modes.
- Guarantee completion semantics:
  - logs flushed
  - final summary emitted
  - process exits automatically on completion (default mode)
- Enforce bounded streaming:
  - cap chunk sizes
  - collapse repetitive updates

## Gap Analysis vs Current Crucible

Current strengths:
- Per-agent progress and highlights are now emitted (`[agent-review]`).
- TUI shows per-agent status and findings.
- Progress/log persistence exists (`review_report.log`).

Current gaps:
- No explicit startup/phase banner UX.
- No single compact "parallel status line" with per-agent elapsed timings.
- No separate `Analysis` and `System Context` sections displayed before round output.
- No explicit convergence judge presentation.
- TUI still has a manual review screen flow; auto-exit behavior needs deterministic policy.

## Work Plan

### P1: Structured Run Header and Phase Model

Changes:
- Introduce explicit events for:
  - `ConfigLoaded`
  - `DiffDetected { changed_lines }`
  - `RunConfig { agents, max_rounds, convergence_enabled, context_enabled }`
  - `PhaseStart { name }`
  - `PhaseDone { name }`
- Render a startup block in non-TUI stderr and TUI.

Acceptance criteria:
- Running `crucible review` prints startup context before analyzer starts.
- `review_report.log` contains startup events.

### P2: Live Round Status Line with Durations

Changes:
- Track per-agent start timestamps.
- Emit/update a line shaped like:
  - `Round 1: [.. claude-code | ✓ codex (52.6s) | .. gemini]`
- Repaint line periodically in TTY and print interval snapshots in non-TTY.

Acceptance criteria:
- During round execution, user sees a continuously updating compact status line.
- Completed agents show elapsed duration.

### P3: Pre-Round Analysis and System Context Panels

Changes:
- Add progress events carrying analyzer output and gathered context summary:
  - `AnalysisReady { markdown }`
  - `SystemContextReady { markdown }`
- Render these blocks before first round output.

Acceptance criteria:
- User sees analysis and system context sections before agent round transcripts.
- Sections are persisted to `review_report.log`.

### P4: Rich Agent Transcript Streaming

Changes:
- Extend per-agent review event payload:
  - `narrative` (plain text/markdown snippet)
  - `issues_json` (optional structured payload)
- In TTY:
  - show agent transcript blocks incrementally.
- In non-TTY:
  - print bounded transcript snippet + structured highlights.

Acceptance criteria:
- User can read substantive agent reasoning while review is running.
- Output remains bounded (no unbounded flooding).

### P5: Convergence and Round-End UX

Changes:
- Emit convergence events:
  - `ConvergenceCheck { round, verdict, rationale }`
- Render:
  - convergence panel
  - `Round N/M complete` divider

Acceptance criteria:
- Every round ends with an explicit convergence statement.
- Final round reports whether convergence was reached.

### P6: Deterministic Completion and Exit Policy

Changes:
- Add explicit mode policy:
  - default: auto-exit on completion after final summary render and log flush
  - future optional flag: `--interactive` to remain open
- Ensure TUI teardown always restores terminal.

Acceptance criteria:
- In default mode, TTY run returns to shell automatically at completion.
- Exit code matches verdict semantics (`0`, `1`, `130`).

### P7: Final Report UX Table

Changes:
- Print final deduped issue table with:
  - severity
  - title/message
  - location
  - agents supporting
- Add optional token/cost section behind `--verbose` or config toggle.

Acceptance criteria:
- Final output is readable as a summary without scanning full transcript.

## Testing Strategy (BDD-first)

Add/update BDD scenarios:
- `startup header is shown`
- `round status line includes per-agent durations`
- `analysis and system context sections appear before round output`
- `agent transcript is streamed`
- `convergence result printed each round`
- `tty mode auto exits when review completes`
- `interactive mode (future) stays open until quit`

Non-BDD checks:
- unit tests for duration formatting and line truncation
- golden snapshot tests for TTY render sections (where practical)

## Rollout Sequence

Recommended implementation order:
1. P6 (auto-exit policy) + minimal tests
2. P1 (startup/phase events)
3. P2 (round status with timings)
4. P3 (analysis/system context blocks)
5. P4 (richer transcript)
6. P5 (convergence judge UX)
7. P7 (final table and optional cost stats)

This order first fixes the "hang perception", then layers readability and depth.

## Definition of Done

Crucible UX is parity-ready when:
- Users can track phase, round, and agent progress in real time.
- Users can read meaningful agent reasoning during execution.
- Users get a concise final summary and immediate shell return on completion.
- `review_report.log` captures all major runtime artifacts for audit/debug.
