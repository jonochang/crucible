# Magpie UX Parity Implementation Plan for Crucible

## Summary
Implement Magpie-like review UX parity in Crucible by introducing a role-aware convenor flow (`analyzer -> reviewers -> judge`), richer round/event modeling, strict prompt contracts, convergence judging, and deterministic completion behavior.

## Public Interface / Type Changes
- Expand `ProgressEvent` with run-header, phase, parallel-status, analysis/context-ready, convergence-judgment, and round-complete events.
- Add supporting types: `Phase`, `ReviewerStatus`, `ReviewerState`, `ConvergenceVerdict`.
- Add CLI flag `--interactive` to keep final TUI screen open; default remains auto-exit.
- Keep `--export-issues`, `--json`, `--hook`, `--verbose`.

## Analyzer Design
- Analyzer prompt must produce: `summary`, `focus_items[]`, `trade_offs[]`, `system_context_markdown`.
- Emit `AnalysisReady` and `SystemContextReady`.
- Persist analyzer outputs into `review_report.log`.
- Inject focus/context into reviewer prompts.

## Reviewer Prompt Design
- Round 1 prompt: exhaustive review across all changed files/functions and required dimensions (correctness, security, performance, error handling, edge cases, maintainability).
- Round 2+ prompt: prior-round discussion only, explicit agreement/disagreement with evidence, and missed-issue callouts.
- Return strict findings JSON plus concise narrative.

## Judge Design
- Add convergence judge stage after each non-final round.
- Convergence prompt outputs strict verdict token: `CONVERGED` or `NOT_CONVERGED`.
- Emit `ConvergenceJudgment` with rationale and per-round completion event.
- Final judge stage produces final synthesis and prioritized action items.

## Convenor Design
- Convenor owns event ordering and sectioned UX rendering.
- Ordered output sections: run header, analysis, system context, round status, agent outputs, convergence verdict, final summary, issues table.
- Track per-agent durations and render compact round status line.
- Flush logs and auto-exit on completion in default mode.

## Issue List and Export
- Keep deduplicated issue list with location pointers (`file:line`).
- Standard export schema: `severity`, `file`, `line_start`, `line_end`, `location`, `message`, `raised_by`.
- Support export to `.json` and `.md`.

## Test Plan
- Extend BDD with startup header, durations, analysis/context sections, convergence output, and default TTY auto-exit completion.
- Keep existing issue export with locations and Ctrl+C/130 tests.
- Add unit tests for duration formatting, convergence parsing, and prompt contracts.

## Rollout Plan
1. PR1: Event model + startup/phase output.
2. PR2: Parallel status with durations.
3. PR3: Analysis/system-context rendering.
4. PR4: Convergence judge and early-stop logic.
5. PR5: TUI phase model + default auto-exit + `--interactive`.
6. PR6: Prompt contract hardening + unit tests.
7. PR7: Docs and final polish.

## Acceptance Criteria
- TTY mode shows full sectioned lifecycle and exits automatically when complete.
- Non-TTY mode emits machine-readable lifecycle lines and final summary.
- `review_report.log` captures startup, phases, convergence, and final report.
- Issue export remains deduplicated and location-aware.
- BDD suite passes including new parity scenarios.

## Assumptions / Defaults
- Default mode is non-interactive auto-exit.
- `--interactive` is opt-in.
- Convergence enabled by default when `max_rounds > 1`.
- Duration display precision is one decimal second.
- Analyzer/context output is bounded and truncated with marker when needed.
