# Changelog

All notable changes to this project will be documented in this file.

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
