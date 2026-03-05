# Magpie Parity Spec (Implemented)

## Scope

This spec tracks Crucible parity against Magpie review UX for:

- analyzer
- reviewer prompts
- judge/convergence
- convenor runtime UX
- issue list/export

## Analyzer Design

Crucible analyzer prompt contract:

- role: senior architect producing pre-review context
- required JSON fields:
  - `summary`
  - `focus_items[]`
  - `trade_offs[]`

Convenor emits:

- `AnalysisReady` (markdown rendered in UX)
- `SystemContextReady` (changed files + gathered context counts)

Analyzer output is logged to `review_report.log`.

## Reviewer Prompt Design

### Round 1 (exhaustive)

Prompt requires:

- exhaustive changed-file/function review
- dimensions: correctness, security, performance, error handling, edge cases, maintainability
- strict JSON output:
  - `narrative`
  - `findings[]`

### Round 2+ (adversarial)

Prompt requires:

- prior-round-only discussion input
- explicit agreement/disagreement behavior
- explicit missed-issue callouts
- strict JSON output with `narrative` + `findings[]`

No same-round leakage is included in prompt instructions.

## Judge Design

Crucible emits convergence decisions each round via `ConvergenceJudgment`:

- verdict token: `CONVERGED` or `NOT_CONVERGED`
- rationale string included in event/log output

Early stop behavior:

- if convergence is reached before max rounds, review loop exits early

Finalization stage:

- judge auto-fix synthesis still runs when warning/critical findings exist
- `Completed` report emitted and logged

## Convenor Design

Convenor event ordering:

1. run header
2. analyzer phase start
3. analysis + system context sections
4. per-round start/status/agent outputs
5. convergence + round completion
6. finalize phase + report

UX parity behaviors implemented:

- startup header with reviewers and round settings
- compact parallel status with durations
- analysis/system context sections before review rounds
- explicit round completion divider
- deterministic completion with default auto-exit
- `--interactive` opt-in to keep TUI open
- Ctrl+C/Ctrl+D exits with `130`

## Issue List + Export

Crucible keeps a deduped canonical issue list with code location pointers.

Export schema:

- `severity`
- `file`
- `line_start`
- `line_end`
- `location`
- `message`
- `raised_by`

Formats:

- `.json`
- `.md`

Dedup normalization:

- case-insensitive keying
- whitespace-normalized message/path matching

## Test Coverage

BDD coverage includes:

- startup header
- round status + durations
- analysis section
- system context section
- convergence output
- default completion behavior
- issue export with locations
- Ctrl+C exit 130

Unit coverage includes:

- duration formatter
- convergence token parser
- reviewer prompt contract checks
- dedup normalization
