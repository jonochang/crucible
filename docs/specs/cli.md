# Crucible CLI UX Specification

## Goals

- Make review progress observable in both TUI and non-TUI modes.
- Provide per-agent and per-round visibility similar to Magpie.
- Allow fast, safe interruption via `Ctrl+C` and `Ctrl+D`.
- Keep outputs stable enough for tests and CI logs.

## Non-TUI (`crucible review` when stdout is not a TTY or `--json` is set)

### Progress Output

Crucible prints progress lines to stderr (stdout reserved for JSON or final report).
It also appends the same progress lines to `review_report.log` in the current working directory.

Phases:
- `analyzer` start/end
- review `round` start/end
- per-agent start/end within a round
- auto-fix ready

Required line formats (exact prefixes):

```
[progress] analyzer:start
[progress] analyzer:done
[progress] round:1 start (agents: claude-code,codex,gemini)
[progress] agent:start round=1 id=claude-code
[progress] agent:done round=1 id=claude-code
[progress] agent:start round=1 id=codex
[progress] agent:done round=1 id=codex
[progress] round:1 done
[progress] round:2 start (agents: claude-code,codex,gemini)
[progress] round:2 done
[progress] autofix:ready
```

Notes:
- Agent start/done lines may interleave.
- If a review is canceled, emit:
  `"[progress] canceled"` then exit `130`.

### Report Log

At completion, Crucible appends a JSON report to `review_report.log`:

```
[report]
{ ... pretty JSON ... }
```

### Exit Codes

- `0` success
- `1` failures (including hook verdict block)
- `130` user interrupt (Ctrl+C or Ctrl+D)

## TUI (`crucible review` with TTY)

### Status Panel

The main screen displays a live status panel:

```
Round 1/2  (Analyzer: done)
claude-code  [running]
codex        [done]
gemini       [queued]
```

Statuses:
- `queued` (not started)
- `running` (in progress)
- `done` (completed)
- `error` (agent failed)

### Interrupts

- `Ctrl+C` exits immediately, aborts any in-flight agent runs, restores terminal, exit code `130`.
- `Ctrl+D` exits immediately with same behavior.

### Additional Feedback

At the end of each round, the TUI prints a short summary block before moving to the final report view:

```
Round 1 complete — 3 findings (1 Critical, 1 Warning, 1 Info)
```

### Report Log

The TUI also writes progress lines and the final JSON report to `review_report.log`.

## Test Requirements (BDD)

### Non-TUI progress output

- Running `crucible review` with a mock agent and stdout redirected must emit progress lines to stderr.
- Expected lines include `analyzer:start`, `round:1 start`, `agent:start`, `agent:done`, `round:1 done`.

### Interrupt handling

- When running in TUI mode, sending `Ctrl+C` should terminate the process with exit code `130`.
- When running in non-TUI mode, sending `Ctrl+C` should terminate the process with exit code `130`.
