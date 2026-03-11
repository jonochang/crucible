# Architecture

Crucible is split into a reusable Rust core library, a CLI wrapper, and pluggable agent adapters.

## Major components

- `crates/libcrucible`: configuration, context gathering, coordination, reporting, and review data structures
- `crates/crucible-cli`: command-line entrypoints and the terminal UI
- `crates/plugins/*`: agent-specific adapters

## Review pipeline

1. Load configuration and determine the review target.
2. Build the diff and gather references, history, and docs context.
3. Run deterministic prechecks.
4. Run analyzer and reviewer agents.
5. Cluster findings, judge convergence, and synthesize the final report.
6. Optionally generate and apply an auto-fix patch.

## Design goals

- Strongly typed, structured outputs instead of heuristic parsing
- Parallel context gathering to keep startup latency low
- A pluggable agent layer so different CLIs can participate in the same review process
- Explicit progress reporting for both TTY and non-TTY execution modes
