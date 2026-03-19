# Generalize Crucible into a task-pack-driven consensus engine

## Summary
- Keep `crucible review` working as-is while extracting the round/debate/convergence machinery from `crates/libcrucible/src/coordinator/mod.rs` into a generic consensus engine.
- Ship a standard set of task packs with the `crucible` binary, while also supporting repo-local task packs under `.crucible/tasks/<pack-id>/` and additional arbitrary pack directories via CLI/config. Packs are declarative: manifest, prompt templates, allowed inputs/providers, role definitions, and a final JSON schema.
- Add a new generic request model that accepts a user prompt plus attachments and supports both single-shot and interactive sessions; keep the first CLI surface thin and non-TUI.
- Preserve review-specific PR/GitHub/auto-fix behavior in the existing review path for v1, but move shared clustering, convergence, transport, and artifact plumbing into reusable layers.

## Key Changes

### Engine/API
- Add `ConsensusTaskRequest { pack_id, prompt, attachments, task_paths, mode }`, `Attachment { id, kind, path_or_inline }`, `ConsensusReport { run_id, pack_id, agreed_items, unresolved_items, result_json, summary_markdown, session_id }`, and `SessionState`.
- Split the review-shaped agent contract in `crates/libcrucible/src/plugin.rs` into:
  - a generic prompt/JSON transport layer used by all tasks
  - a consensus-task layer that renders pack prompts and parses a shared internal `ConsensusItem` schema
  - the existing review adapter, which keeps `ReviewReport` untouched in `crates/libcrucible/src/report.rs`.
- Generalize the round engine in `crates/libcrucible/src/coordinator/mod.rs` to cluster `ConsensusItem`s instead of `Finding`s; keep overlap/text-similarity clustering, add item `kind`, `importance`, `confidence`, and attachment anchors so non-code tasks can converge on risks, options, decisions, or questions.
- Keep pack-specific final outputs separate from round items: the judge must emit validated `result_json` against the pack’s `schema.json`, plus `summary_markdown` and `unresolved_questions`.

### Task Packs
- The `crucible` binary ships with built-in standard packs for common consensus workflows; these are available even outside a repo and serve as the default baseline experience.
- Standard pack layout: `.crucible/tasks/<pack-id>/pack.toml`, `analyzer.md`, `reviewer.md`, `judge.md`, `schema.json`, optional `render.md`.
- `pack.toml` must define: `id`, `version`, `title`, `description`, `allowed_attachment_kinds`, `context_providers`, `roles[]`, `rounds`, `quorum`, `result_schema`, and whether the pack may ask clarification questions.
- Discovery order: explicit `--task-path <dir>` entries first, then repo `.crucible/tasks`, then configured extra directories from `.crucible.toml`, then built-in packs shipped with the binary; first matching pack id wins. Direct path execution is also allowed for ad hoc packs.
- v1 packs are declarative only: no scripts, no arbitrary hooks, no executable post-processors. Reuse comes from copying/versioning packs across repos or pointing Crucible at any folder containing compatible pack directories.

### Inputs, Context, and Interaction
- Replace the diff-only `ReviewContext` shape in `crates/libcrucible/src/context/mod.rs` with a parallel `TaskContext` that always starts from `prompt + attachments`, then optionally enriches with built-in providers: `repo_docs`, `selected_files`, `git_diff`, `git_history`, `prechecks`, `prompt_only`.
- Support attachment kinds `markdown`, `text`, `source_file`, and `diff` in v1; attachments are normalized into named context blocks with provenance so every consensus item cites its source.
- Add a thin CLI surface in `crates/crucible-cli/src/main.rs`: `crucible consensus run`, `crucible consensus reply`, `crucible consensus packs`, and reuse `session` for list/resume/delete. Keep it JSON/markdown-first; defer TUI until the generic engine stabilizes.
- Implement interaction via persisted session folders under `.crucible/sessions/<session_id>/` containing request, transcript, state, and latest report. When a pack allows clarifications, the judge can emit `clarification_requests[]`; `consensus reply` appends the user answer and resumes from the current session state rather than starting over.

### Built-in Packs and General Utility
- Ship three standard built-in starter packs besides review: `requirements-review`, `design-review`, and `test-plan-review`. Repos can override these by defining packs with the same ids locally or via explicit task paths.
- Give each starter pack a concrete final schema:
  - requirements: `accepted_requirements`, `ambiguities`, `missing_acceptance_criteria`, `risks`, `recommended_next_steps`
  - design: `decision_summary`, `tradeoffs`, `open_questions`, `failure_modes`, `recommended_changes`
  - test-plan: `coverage_gaps`, `high-risk scenarios`, `missing fixtures`, `recommended_test_matrix`, `release_blockers`
- General-purpose additions that make Crucible useful beyond review:
  - preserve unresolved disagreements instead of collapsing everything into one answer
  - add pack-specific renderers so results can be emitted as JSON plus Markdown checklists/briefs
  - generalize `prompt_eval` into a pack-aware regression harness with golden cases
  - allow packs to declare “ask before verdict” so the council can request missing context instead of hallucinating.

## Test Plan
- Unit tests for pack discovery precedence, manifest validation, JSON-schema validation, attachment normalization, and session resume behavior.
- Consensus-engine tests for item clustering across non-file anchors, convergence with unresolved disagreements, and review adapter compatibility with existing `Finding` semantics.
- Integration tests for:
  - `consensus run --pack requirements-review --prompt ... --attach spec.md`
  - loading a pack from `--task-path`
  - clarification flow: run → emit question → reply → resumed report
  - invalid pack/schema errors with actionable diagnostics
  - existing `crucible review` behavior and GitHub draft output unchanged.
- Generalize the current `prompt_eval` dataset runner so each case includes `pack_id`, `prompt`, `attachments`, and expected result fragments; add one golden dataset per starter pack plus a regression dataset for review.

## Assumptions
- `crucible review` stays as the stable review-specific entrypoint in v1; PR draft generation, inline comments, and auto-fix diffs remain review-only.
- The first generic CLI is intentionally thin and non-TUI to stay library-first, even though sessions are supported immediately.
- Repo-local packs are the standard default; arbitrary folder support is opt-in via CLI/config, not a separate registry or global skills system.
- Built-in packs provide the out-of-the-box baseline; repo-local or explicit-path packs may override them for project-specific workflows.
- Review remains bespoke during the first phase, but new shared transport/context/consensus primitives should be written so review can migrate onto them later with minimal churn.
- v1 pack reuse is directory-based, not inheritance-based; no `extends`, scripting, or remote fetching yet.
