# Workflow

Crucible runs in three broad phases.

## 1. Deterministic gate

Before agent review starts, Crucible gathers the diff and runs deterministic checks. This can include:

- `untangle` for structural regressions
- linters
- type checks
- targeted tests

These results are injected into the reviewer context so the agents start from concrete signals instead of pure speculation.

## 2. Multi-agent review

Crucible asks the configured analyzer to produce focus areas, then sends the diff and context to the reviewer council.

- Round 1 is independent analysis.
- Later rounds are adversarial and comparative.
- A judge determines whether the review has converged or whether another round is required.

## 3. Finalization

At the end of the run, Crucible emits:

- a deduplicated issue list
- a final narrative summary
- an action plan
- a GitHub review draft
- an optional unified diff for auto-fix

If the run is launched in the TUI, the patch can be applied directly from the terminal.
