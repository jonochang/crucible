# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

## 0.1.18 - 2026-03-06

- Added explicit `[final-analysis]` and `[pr-comment]` sections to `review_report.log` output.
- Fixed TUI-side final report logging to use robust JSON fallback serialization.
- Improved debug logging behavior so full prompts/diffs are logged only with `--verbose`.
- Added timestamps to debug/progress/report log entries.

## 0.1.17 - 2026-03-06

- Enabled TUI for non-local review targets (e.g. `--branch`) when running in a terminal.
- Changed default `crucible review` target to current branch delta plus local worktree changes.
- Added base branch resolution fallback (`origin/HEAD` -> `main` -> `master`) for default branch-aware reviews.

## 0.1.16 - 2026-03-06

- Removed `git` CLI dependency from local/repo/branch/files diff paths by using `libgit2` directly.
- Reworked BDD repo setup to use `git2` APIs instead of spawning `git`.
- Added `gh` runtime dependency to packaged `crucible` binary via wrapper.

## 0.1.15 - 2026-03-06

- Updated untangle prechecks to use supported commands: `untangle analyze` and `untangle quality --metric crap`.
- Added CRAP metric quality signal into review precheck context.
- Avoid running local reviews when there are no changed code lines.
- Added `untangle` to the dev shell in `flake.nix` (with source override to current upstream hash).

## 0.1.14 - 2026-03-06

- Added `--debug` flag to write deep diagnostics to `crucible.log` (prompts + raw agent I/O).
- Expanded live agent output to include full narrative/details while rounds are running.
- Added final analysis markdown to the report model and CLI/TUI rendering.
- Added timeout guards across reviewer/judge phases to prevent finalize hangs.

## 0.1.13 - 2026-03-06

- Release cut for the FINDINGS.md implementation wave.
- Bumped crate/package versions to `0.1.13` for Cargo and Nix installs.

## 0.1.12 - 2026-03-05

- Added `crucible review [PR]` support to checkout PR branches and review PR diffs.
- Added `--local` and `--repo` review target modes similar to Magpie.
- Added `--branch [base]` and `--files <paths...>` review target modes.
- Expanded analyzer schema with `affected_modules`, `call_chain`, `design_patterns`, and reviewer checklist context.
- Added role-specialized reviewer prompting (Claude: correctness/security, Codex: architecture, Gemini: performance/edge-cases).
- Added deterministic precheck fusion (`untangle`, linters, type checks, tests) into reviewer context.
- Added LLM convergence judge phase (`CONVERGED`/`NOT_CONVERGED`) with heuristic fallback.
- Added optional issue structurizer stage and richer canonical issue schema with evidence anchors.
- Added confidence calibration for low-confidence singleton findings.
- Added actionable final outputs: prioritized action plan + ready-to-post PR comment artifact.
- Added adaptive diff chunking controls (`max_diff_lines_per_chunk`, `max_diff_chunks`) to cap prompt cost.
- Added `crucible prompt-eval` golden-set harness for precision/recall drift tracking.

## 0.1.11 - 2026-03-05

- Added spinner + colored status line to the TUI analyzing/reviewing screens.

## 0.1.10 - 2026-03-05

- Hook now executes `just crucible-pre-push` instead of calling `crucible review --hook` directly.
- Added `Justfile` with `crucible-pre-push` target: skip when no diff, otherwise run single-reviewer hook review.
- Added `crucible review --reviewer <id>` and `--max-rounds <n>` overrides for focused hook runs.
- Added colored spinner/status-line rendering for live non-TUI progress updates.

## 0.1.9 - 2026-03-05

- Added Magpie-style run header, phase, analysis, system context, parallel status, convergence, and round-complete progress events.
- Added round-1 exhaustive and round-2+ adversarial reviewer prompt contracts with narrative output.
- Added TUI auto-exit-by-default behavior with `--interactive` opt-in to keep the final screen open.
- Hardened issue dedup normalization and preserved location-rich issue exports (`.json` and `.md`).
- Expanded BDD scenarios and unit tests for parity behavior and prompt/runtime helpers.

## 0.1.8 - 2026-03-05

- Stream per-agent review summaries and top findings during execution.
- Show per-agent findings in the TUI reviewing screen and in `review_report.log`.
- Add BDD assertion coverage for `[agent-review]` progress output.

## 0.1.7 - 2026-03-05

- Write progress and final reports to `review_report.log`.
- Allow quitting the TUI at any phase with `q` or `Esc`.

## 0.1.6 - 2026-03-05

- Default to claude-code, codex, and gemini agents.
- Run two review rounds to reach consensus.

## 0.1.5 - 2026-03-05

- Added CLI progress events, per-agent TUI status, and interrupt handling.
- Added BDD coverage for progress output and Ctrl+C behavior.
- Documented CLI UX expectations.

## 0.1.4 - 2026-03-05

- Build and install the `crucible` binary directly (no `crucible-cli` shim).

## 0.1.1 - 2026-03-04

- Fixed `nix profile install` by aligning `git2` with libgit2 1.9.x and exporting flake package/app outputs.
- Added a comprehensive README with usage, configuration, and CLI agent contract.
- Generated `Cargo.lock` for reproducible builds.

## 0.1.2 - 2026-03-05

- Added Cucumber BDD scenarios with mock agents and opt-in real agent runs.
- Aligned Gemini CLI invocation and parsing with CLI-based workflows.

## 0.1.3 - 2026-03-05

- Simplified Nix installs by building `crucible-cli` and exposing a `crucible` shim.

## 0.1.0 - 2026-03-03

- Initial MVP release.
