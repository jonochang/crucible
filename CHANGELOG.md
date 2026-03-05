# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

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
