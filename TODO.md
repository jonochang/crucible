- [x] P0: Align event model with Magpie parity requirements
- [x] P0.1: Add `RunHeader` progress event
- [x] P0.2: Add `PhaseStart` and `PhaseDone` events
- [x] P0.3: Add `ParallelStatus` event with per-agent status + duration
- [x] P0.4: Add `AnalysisReady` and `SystemContextReady` events
- [x] P0.5: Add `ConvergenceJudgment` event
- [x] P0.6: Add `RoundComplete` event

- [x] P1: Analyzer parity implementation
- [x] P1.1: Implement strict analyzer prompt contract
- [x] P1.2: Emit analyzer markdown summary event
- [x] P1.3: Emit system context event
- [x] P1.4: Persist analyzer outputs to `review_report.log`

- [x] P2: Reviewer prompt parity
- [x] P2.1: Implement exhaustive round-1 reviewer prompt
- [x] P2.2: Implement adversarial round-2+ reviewer prompt
- [x] P2.3: Ensure prior-round-only visibility (no same-round leakage)
- [x] P2.4: Include concise narrative output for live UX

- [x] P3: Judge parity
- [x] P3.1: Add convergence judge prompt and parse strict verdict token
- [x] P3.2: Emit per-round convergence rationale
- [x] P3.3: Stop early on convergence
- [x] P3.4: Implement final synthesis prompt and output

- [x] P4: Convenor non-TTY UX
- [x] P4.1: Print startup header (config/diff/reviewers/round settings)
- [x] P4.2: Render phase transitions in deterministic order
- [x] P4.3: Render compact round status line with durations
- [x] P4.4: Render analysis and system context sections
- [x] P4.5: Render convergence section and round completion divider
- [x] P4.6: Render final issue table and verdict

- [x] P5: Convenor TTY UX
- [x] P5.1: Refactor TUI into explicit phase sections
- [x] P5.2: Add analysis/system context panels
- [x] P5.3: Add compact parallel status panel
- [x] P5.4: Add convergence panel
- [x] P5.5: Add final summary + issues panel
- [x] P5.6: Auto-exit by default after completion/log flush

- [x] P6: Interaction mode controls
- [x] P6.1: Add `--interactive` CLI flag
- [x] P6.2: Keep final TUI screen open only when interactive
- [x] P6.3: Preserve Ctrl+C/Ctrl+D exit code 130

- [x] P7: Issue list/export hardening
- [x] P7.1: Keep deduped issue list canonical schema
- [x] P7.2: Normalize dedup key matching
- [x] P7.3: Preserve `.json` and `.md` export support
- [x] P7.4: Include `raised_by` and location pointers in all exports

- [x] P8: BDD scenarios
- [x] P8.1: Add startup header scenario
- [x] P8.2: Add round status + durations scenario
- [x] P8.3: Add analysis section scenario
- [x] P8.4: Add system context section scenario
- [x] P8.5: Add convergence output scenario
- [x] P8.6: Add default auto-exit completion scenario
- [x] P8.7: Keep issue export-with-location scenario passing
- [x] P8.8: Keep Ctrl+C scenario passing

- [x] P9: Unit tests
- [x] P9.1: Duration formatter tests
- [x] P9.2: Convergence verdict parser tests
- [x] P9.3: Prompt contract snapshot tests
- [x] P9.4: Dedup key normalization tests

- [x] P10: Documentation updates
- [x] P10.1: Update `docs/specs/cli.md` event/output contracts
- [x] P10.2: Keep `docs/specs/magpie_parity.md` synchronized with implementation
- [x] P10.3: Update `README.md` examples and flags

- [x] RELEASE: Build and test gates
- [x] RELEASE.1: `cargo build -p crucible-cli`
- [x] RELEASE.2: `cargo test -p crucible-cli --test bdd`
- [x] RELEASE.3: Record release notes/changelog entry

## UX/API Review Findings (2026-03-10)

- Finding: Startup progress is incomplete. Context gathering and prechecks happen before progress events begin, so the longest startup phase is silent.
- Finding: Progress events are too orchestration-centric. The API exposes analyzer/round lifecycle, but not context/precheck sub-phases, counts, or durations.
- Finding: Logging is fragmented. `review_report.log` captures progress/report output while `crucible.log` captures plugin debug I/O, with no shared run identifier or unified artifact model.
- Finding: Final assessment export is awkward. `--json` prints to stdout and `--export-issues` writes only the deduplicated issue list; there is no first-class `ReviewReport` artifact path.
- Finding: PR integration stops at generating `pr_comment_markdown`. There is no structured review-comment draft model, no GitHub publishing command, and no hunk/commit mapping layer for inline comments.
- Finding: The current report schema is not rich enough for PR publishing. `CanonicalIssue` carries file and line metadata, but not the GitHub-specific fields needed to reliably post threaded review comments.

## Detailed Implementation Plan

- [ ] P11: Introduce run-scoped artifact model
- [ ] P11.1: Add `run_id` to `ReviewReport` and all progress/log events so progress, debug, and output artifacts can be correlated across a single review run.
- [ ] P11.2: Replace the fixed implicit log-file behavior with an explicit artifact layout concept, e.g. `review_report.log`, `debug.log`, `report.json`, `issues.json`, keyed by `run_id`.
- [ ] P11.3: Define a small Rust API surface for review artifacts in `libcrucible`, so the CLI/TUI and future GitHub publisher use the same artifact metadata instead of hard-coded filenames.
- [ ] P11.4: Preserve backward compatibility by continuing to write `review_report.log` by default, but route it through the new artifact layer.

- [ ] P12: Make startup progress explicit end-to-end
- [ ] P12.1: Move progress emission earlier so it starts before `ReviewContext::from_push` / `from_diff` performs expensive work.
- [ ] P12.2: Extend `ProgressEvent` with explicit startup events for diff resolution, reference collection, history collection, docs collection, and prechecks.
- [ ] P12.3: Add payloads for item counts and timings, e.g. changed file count, references found, docs loaded, precheck tool count, phase duration.
- [ ] P12.4: Update non-TTY stderr rendering to print these startup phases in deterministic order with concise, human-readable summaries.
- [ ] P12.5: Update the TUI analyzing screen to show startup sub-phases and their latest status instead of only a generic analyzer spinner.
- [ ] P12.6: Ensure failures during startup phases emit terminal-visible progress/error events before aborting, so users can distinguish precheck failures from agent failures.

- [ ] P13: Unify debug and progress logging
- [ ] P13.1: Introduce a single logging abstraction used by CLI command code, TUI code, coordinator progress emission, and plugin adapter code.
- [ ] P13.2: Route `--debug` through that abstraction so orchestration transitions, context collection, prompt dispatch, raw agent I/O, and final report persistence all land in one coherent run-scoped debug stream.
- [ ] P13.3: Keep `--verbose` as terminal streaming behavior only; do not make it responsible for what is persisted to disk.
- [ ] P13.4: Add log records for key state transitions that are currently missing, especially diff target resolution, PR checkout/diff fetch, issue export, and final artifact writes.
- [ ] P13.5: Document the distinction between progress logs, debug logs, and final report artifacts so users know which file to inspect for what.

- [ ] P14: Add first-class final assessment export
- [ ] P14.1: Add a CLI flag such as `--output-report <path>` that writes the full serialized `ReviewReport` to a chosen file path.
- [ ] P14.2: Keep `--export-issues <path>` focused on issue-list export only; avoid overloading it with full-report responsibilities.
- [ ] P14.3: Support at least `.json` for full report export; decide whether `.md` is also supported via a rendered summary view rather than raw schema.
- [ ] P14.4: Ensure both TUI and non-TTY paths write the same report artifact shape, with no divergence in fields or timing.
- [ ] P14.5: Include artifact paths in the final terminal output so users can immediately find the report, issue export, and debug log.

- [ ] P15: Add structured PR review draft model
- [ ] P15.1: Extend `ReviewReport` with a structured PR review payload in addition to `pr_comment_markdown`.
- [ ] P15.2: Define types for review overview comment plus inline comment drafts, including:
- [ ] P15.2.a: source issue id or finding id
- [ ] P15.2.b: source agents / `raised_by`
- [ ] P15.2.c: repository-relative file path
- [ ] P15.2.d: start/end line
- [ ] P15.2.e: side (`RIGHT`/`LEFT`) and line semantics needed for GitHub review APIs
- [ ] P15.2.f: rendered body text for the inline comment
- [ ] P15.2.g: fallback mode when an issue cannot be mapped to a diff hunk
- [ ] P15.3: Keep the overview/final summary comment separate from inline comments so publish flows can decide whether to post a pending review, standalone comments, or both.
- [ ] P15.4: Preserve attribution to original agents such as `codex`, `gemini`, and `claude-code` in the structured draft model and the rendered body.

- [ ] P16: Build GitHub diff-position mapping layer
- [ ] P16.1: Add a PR diff parser that can map `file + line_start/line_end` onto the actual changed hunks returned by GitHub for the reviewed PR.
- [ ] P16.2: Decide on the publishing strategy: use GitHub review API with file/line/side semantics rather than legacy diff-position APIs where possible.
- [ ] P16.3: Handle unmappable findings explicitly by downgrading them to the overview comment instead of silently dropping them.
- [ ] P16.4: Record why a finding could not be mapped, so debug logs and tests can verify the fallback behavior.
- [ ] P16.5: Ensure mapping logic works for added, deleted, and modified lines, multi-line spans, and comments anchored near the end of a hunk.

- [ ] P17: Add GitHub publishing command
- [ ] P17.1: Add an explicit CLI subcommand or flag for publication, e.g. `crucible review <PR> --publish-github` or `crucible github publish-review`.
- [ ] P17.2: Separate review generation from review publication so users can inspect artifacts before posting to GitHub.
- [ ] P17.3: Use `gh` if present for auth/environment reuse, or call GitHub APIs directly if that is simpler and more stable; choose one path and document it clearly.
- [ ] P17.4: Post inline comments with original-agent attribution and a final overview comment from the structured PR review payload.
- [ ] P17.5: Add dry-run mode that renders the exact overview and inline comments without posting them.
- [ ] P17.6: Add idempotency or duplicate-post protection for reruns on the same PR/head SHA.
- [ ] P17.7: Surface the published review URL / PR URL in final output.

- [ ] P18: Tighten CLI/API UX around outputs
- [ ] P18.1: Revisit `crucible review` help text so progress, debug logs, issue export, full report export, and GitHub publish behavior are clearly separated.
- [ ] P18.2: Ensure JSON mode remains script-friendly by keeping stdout reserved for the main machine-readable report and pushing human guidance to stderr.
- [ ] P18.3: Standardize terminology: `issues`, `findings`, `assessment`, `report`, and `review draft` should each mean one specific artifact type.
- [ ] P18.4: Add examples to `README.md` for:
- [ ] P18.4.a: watching detailed progress locally
- [ ] P18.4.b: capturing debug logs
- [ ] P18.4.c: exporting the final report to file
- [ ] P18.4.d: generating and publishing a GitHub PR review

- [ ] P19: Tests and verification
- [ ] P19.1: Add unit tests for new progress events, duration formatting, and startup-phase rendering.
- [ ] P19.2: Add tests for artifact path selection and full report export.
- [ ] P19.3: Add tests for structured PR review draft serialization.
- [ ] P19.4: Add tests for GitHub diff-to-comment mapping, including unmappable findings and multi-line comments.
- [ ] P19.5: Add CLI integration tests for `--output-report`, debug log generation, and GitHub dry-run mode.
- [ ] P19.6: Extend BDD coverage for startup progress visibility, run-scoped logs, exported report artifacts, and PR publish flows.

- [ ] P20: Rollout order
- [ ] P20.1: Land artifact model + run id first, because logging, export, and GitHub publishing all depend on stable run-scoped outputs.
- [ ] P20.2: Land startup progress next, because it improves UX immediately and validates the extended progress API.
- [ ] P20.3: Land full report export after the artifact model, so downstream tooling can consume a stable file contract.
- [ ] P20.4: Land structured PR review draft schema before any GitHub publishing command, so publish behavior is built on typed data rather than markdown scraping.
- [ ] P20.5: Land GitHub mapping + dry-run before live publish, then enable actual publication only once mapping and duplicate-post safeguards are tested.
