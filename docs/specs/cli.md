# Crucible CLI UX Specification

## Goals

- Make runtime progress explicit (startup -> analysis -> rounds -> convergence -> finalization).
- Show per-agent status and durations while reviews are running.
- Show analyzer/system-context output before reviewer rounds.
- Exit cleanly by default when complete; keep screen open only with `--interactive`.

## `crucible review`

```bash
crucible review [--hook] [--json] [--verbose] [--interactive] [--export-issues <path>]
```

Flags:
- `--interactive`: keep TUI open at completion (default is auto-exit).
- `--json`: print report JSON to stdout.
- `--hook`: return hook-friendly exit code from verdict.
- `--verbose`: forward richer agent CLI debug output.
- `--export-issues`: write deduped issue list (`.json` or `.md`).

## Progress/Event Contract

Crucible emits and logs these lifecycle events:

- `RunHeader`
- `PhaseStart`
- `AnalyzerStart`
- `AnalysisReady`
- `SystemContextReady`
- `AnalyzerDone`
- `RoundStart`
- `ParallelStatus`
- `AgentStart`
- `AgentReview`
- `AgentDone` / `AgentError`
- `RoundDone`
- `ConvergenceJudgment`
- `RoundComplete`
- `PhaseDone`
- `AutoFixReady`
- `Completed`
- `Canceled`

Non-TTY output prints deterministic lines to stderr and appends the same lifecycle to `review_report.log`.

## Non-TTY Output Shape

Crucible prints:

- Startup header:
  - `Configuration loaded`
  - `Found local changes (<n> lines)`
  - `Reviewers: ...`
  - `Max rounds: ...`
- Analysis block (`--- Analysis ---`)
- System context block (`--- System Context ---`)
- Round lifecycle (`round:start`, `round:status`, per-agent status)
- Convergence line (`verdict=CONVERGED|NOT_CONVERGED`)
- Round divider (`-- Round N/M complete --`)
- Final issue table + verdict

## TUI Behavior

- Shows phase, round, analyzer status, compact parallel agent status, analysis/system-context snippets, and convergence status.
- `Ctrl+C` and `Ctrl+D` exit with code `130` and restore terminal.
- Default behavior auto-exits at completion; `--interactive` keeps final screen open.

## Issue Export Contract

Deduped issue schema:

- `severity`
- `file`
- `line_start`
- `line_end`
- `location`
- `message`
- `raised_by`

Supported outputs:
- `.json`: structured array
- `.md`: numbered markdown list

Dedup key normalization:
- case-insensitive
- whitespace-normalized
- keyed by severity + file + span + message

## Exit Codes

- `0`: success
- `1`: failure / blocked hook verdict
- `130`: user interrupt (`Ctrl+C`, `Ctrl+D`)
