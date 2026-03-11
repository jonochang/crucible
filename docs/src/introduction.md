# Introduction

Crucible is an autonomous, multi-agent code review system that runs locally before code reaches CI or production.

It combines deterministic repository checks with a council of CLI-driven reviewer agents, then synthesizes their findings into a single report, action plan, and optional auto-fix patch.

## What Crucible does

- Builds a reviewable diff from local changes, a branch comparison, selected files, or a pull request.
- Gathers local context in parallel: symbol references, recent history, and project documentation.
- Runs deterministic prechecks such as `untangle`, linters, type checks, and targeted tests before spending model tokens.
- Invokes multiple reviewer agents with distinct roles, then coordinates convergence across rounds.
- Produces a structured report, a GitHub-ready review draft, and an optional unified diff for auto-remediation.

## Core ideas

- Local-first review: run the review while the developer still has the code in working memory.
- Structured outputs: agents return strict JSON so findings can be clustered and exported reliably.
- Debate over monologue: multiple reviewers analyze the same change and reconcile disagreements.
- Deterministic gate first: fast local checks provide architectural and correctness signals before LLM review.

## Related source docs

This book is the operator-facing documentation site. Deeper design material still lives in the repository under `docs/specs/`.
