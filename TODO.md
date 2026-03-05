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
