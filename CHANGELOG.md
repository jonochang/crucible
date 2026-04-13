# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

## 0.1.32 - 2026-04-13

- Fixed the coordinator’s round comparison logic to use matching integer types, resolving the release build failure introduced by the clean zero-finding early-exit path.

## 0.1.31 - 2026-04-13

- Added explicit analyzer-source progress logging so runs now report the analyzer runtime id, role, plugin, and whether analysis used the configured agent or fallback context generation.
- Surfaced analyzer-source information consistently in stderr progress logs, persisted run logs, and the review TUI.

## 0.1.30 - 2026-04-13

- Added `[task_packs.review]` config overrides for the built-in review pack so analyzer, judge, convergence, structurizer, and autofix plugins can be customized without replacing the whole task pack.
- Added coverage for partial review task-pack override parsing and for applying configured finalization plugin overrides to the built-in `review` pack.
- Improved agent launch failure diagnostics to preserve the full error chain and include command, args, cwd, and `PATH` when CLI process spawning fails.

## 0.1.29 - 2026-04-10

- Hardened token-optimization safeguards in the multi-agent review pipeline.
- Added an early exit for clean zero-finding rounds to skip unnecessary convergence work and extra rounds.
- Reduced round-N prompt size by replacing the repeated full diff with a changed-files summary while preserving review context.

## 0.1.28 - 2026-04-10

- Reworked Crucible around task-pack-defined roles and declarative rounds, decoupling execution plugins from reviewer personas and focus areas.
- Added built-in role plans and finalization assignments for the `review`, `requirements-review`, `design-review`, and `test-plan-review` packs.
- Changed review and generic consensus execution to instantiate role-specific agents from `role + plugin` assignments, with role-aware provenance in progress, reports, and PR review output.
- Removed top-level plugin persona/focus/judge/analyzer config fields in favor of plugin execution config plus task-pack-owned roles, rounds, and finalization.
- Expanded BDD coverage to verify custom multi-round role reassignment and final-judge prompt generation for consensus task packs.

## 0.1.27 - 2026-04-08

- Replaced hardcoded agent fields in `PluginsConfig` with a dynamic `BTreeMap` using `serde(flatten)`, allowing any number of named agents via `[plugins.<agent-id>]` TOML sections.
- Added configurable `reviewer_focus` per agent so each reviewer's focus area is set from config.
- Added default agent configs for `opencode-kimi` (Kimi K2.5 via `moonshot/kimi-k2-5`) and `opencode-glm` (GLM-5.1 via `zai-coding-plan/glm-5.1`).
- Added `crucible config init --full` to include all configured agents in the review list.
- Added `crucible config init --global` to write config to `~/.config/crucible/config.toml`.

## 0.1.26 - 2026-03-20

- Added a generic task-pack consensus engine, built-in `requirements-review`, `design-review`, and `test-plan-review` packs, plus `crucible consensus run|reply|packs`.
- Changed `crucible review` to use the standard built-in `review` pack for analyzer/reviewer/judge prompt definitions while preserving the existing review report pipeline.
- Added persisted consensus sessions under `.crucible/sessions/<id>/` and CLI session list/resume/delete support.
- Added BDD coverage for built-in consensus packs, custom task-pack loading from explicit paths, and saved-session reply flows.
- Fixed review prechecks to use the current `untangle analyze report` / `untangle quality report` commands, pipe subprocess output cleanly, and avoid pipe-buffer deadlocks while capturing tool output.
- Added new `reference.rs` and `pr_review.rs` unit coverage, mutation-testing `Justfile` targets, and updated the Nix flake to untangle `v0.5.4` plus `llvm-tools-preview`.

## 0.1.25 - 2026-03-18

- Added `opencode` support to the built-in CLI agent adapter, including non-interactive JSON invocation defaults and response parsing.
- Changed review runs to tolerate analyzer/reviewer agent failures and still produce a final report with explicit `agent_failures`.
- Changed progress logs to use human-readable local timestamps instead of raw numeric timestamps.
- Expanded the default reviewer pool to `claude-code`, `codex`, `gemini`, and `open-code`, selecting up to three available agents in that order.
- Added standby reviewer fallback so an available `open-code` reviewer can replace a failed active reviewer such as `gemini` during a round.

## 0.1.24 - 2026-03-11

- Changed the TUI conversation viewer to reserve one latest transcript slot per configured agent instead of a shared rolling window.

## 0.1.23 - 2026-03-11

- Removed the tracked repo `.githooks/pre-push` hook so normal pushes no longer trigger a live local Crucible review by default.
- Hardened Claude Code envelope parsing to extract fenced JSON even when the CLI prefixes it with prose, fixing analyzer/reviewer parse failures on valid Claude responses.

## 0.1.22 - 2026-03-11

- Added an `mdBook` documentation site under `docs/`, with local `just docs-serve` and `just docs-build` commands.
- Fixed reviewer response parsing so malformed agent JSON no longer silently deserializes as an empty finding list.
- Fixed diff chunking to preserve the full change set when the chunk limit is reached.
- Changed reviewer execution to run concurrently instead of serially while preserving per-agent progress reporting.
- Restored the original checkout after PR-targeted reviews instead of leaving the repository on the PR branch.
- Added a compact rolling two-line agent conversation preview to the live review UI and progress logs.

## 0.1.21 - 2026-03-11

- Restored regression coverage for `crucible version` and `crucible --version` in the CLI BDD suite.

## 0.1.20 - 2026-03-10

- Added explicit startup sub-phase progress for references, history, docs, and prechecks, including counts and durations.
- Fixed diff rendering so libgit2-generated patches preserve line prefixes and local reviews no longer skip valid changes.
- Added `run_id` to `ReviewReport` and per-run artifact bundles under `.crucible/runs/<run_id>/`.
- Added `--output-report <path>` to write the full serialized review report to a chosen file.
- Added structured `pr_review_draft` output with mapped inline comments and overview-only fallback comments.
- Added `--github-dry-run` to preview the GitHub review overview and inline comments without posting.
- Added `--publish-github` to publish a GitHub PR review via `gh api`, including duplicate-post protection for the current head SHA.
- Updated the TUI, CLI help, README, and CLI spec to reflect the richer progress/output/GitHub review flow.

## 0.1.19 - 2026-03-06

- Added analyzer retry on parse/schema failures before aborting the run.
- Fixed TUI behavior to detect failed review tasks and exit with an error instead of hanging on analyzer/finalize phases.
- Improved TUI run header formatting for review scope + runtime metadata.

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
