# Review Pack Roles

The `review` pack has two variants: **long** (default) and **short** (`--short` flag).

## Long Review Pack (default)

Orchestrates 13 roles across a pre-round context extractor, 3 rounds, and finalization.

### Pre-Round — Context Extraction (Analyzer)

The Change Intent Analyst runs before any reviewer, producing a shared review brief.

| Role ID | Name | Focus | Default Plugin |
|---|---|---|---|
| `change-intent-extractor` | Change Intent Analyst | PR purpose, affected contracts, changed APIs, hidden assumptions, required invariants, migration/runtime implications | `opencode-glm` |

### Round 1 — Discovery (6 agents)

Six agents independently review the diff from different perspectives.

| Role ID | Name | Focus | Weight | Default Plugin |
|---|---|---|---|---|
| `program-semantics` | Program Semantics Auditor | State transitions, invariants, hidden regressions, control-flow, concurrency | 1.5 | `opencode-glm` |
| `maintainer-review` | Principal Maintainer | API contracts, refactor safety, maintainability, change surface, design coherence, migration safety | 1.5 | `codex` |
| `security-reliability` | Security and Reliability Auditor | Trust boundaries, authorization, secrets, injection, exposure, failure modes, recovery, retry storms, resource leaks | 1.5 | `opencode-kimi` |
| `requirements-contract-review` | Requirements and Contract Reviewer | User-visible behavior, business rules, domain invariants, acceptance criteria, intent-vs-implementation gaps | 1.5 | `codex` |
| `test-evidence-review` | Test Evidence Reviewer | Missing tests, weak assertions, false confidence from over-mocking, missing negative/boundary/authorization/migration tests | 1.25 | `codex` |
| `performance-resource-review` | Performance and Resource Reviewer | N+1 queries, unbounded loops, large allocations, missing indexes, lock contention, handle leaks, blocking I/O in hot paths | 1.0 | `opencode-glm` |

### Round 2 — Challenge and Verify (2 agents)

Cross-pollinated findings from Round 1 are distributed to adversarial reviewers.

| Role ID | Name | Focus | Weight | Default Plugin |
|---|---|---|---|---|
| `contrarian-review` | Contrarian Systems Reviewer | Challenge assumptions, cross-file interactions, integration risk, deployment-order failures, background job/cache/migration interactions | 1.25 | `opencode-glm` |
| `verification-review` | Verification Reviewer | Validate prior findings, prune weak claims, confirm evidence, surface duplicates, verify fixes don't introduce new bugs | 1.25 | `codex` |

### Round 3 — Fix Planning (1 agent)

| Role ID | Name | Focus | Weight | Default Plugin |
|---|---|---|---|---|
| `fix-strategy-review` | Fix Strategy Reviewer | Minimal fixes, risk containment, tests first, migration safety, isolation plan for each canonical issue | 1.0 | `codex` |

### Finalization

| Role ID | Name | Focus | Default Plugin |
|---|---|---|---|
| `review-judge` | Final Review Judge | Synthesize canonical issues, preserve only supported claims, mark merge-blocking issues | `codex` |
| `convergence-judge` | Strict Convergence Judge | Determine whether material disagreement or high-severity risk remains across rounds | `codex` |

---

## Short Review Pack (`--short`)

Same as the original 8-role, 2-round pack for fast reviews.

| Round | Roles | Agents |
|---|---|---|
| 1 — Initial Review | `program-semantics` (opencode-glm), `maintainer-review` (codex), `security-reliability` (opencode-kimi) | 3 |
| 2 — Challenge & Verify | `contrarian-review` (opencode-glm), `verification-review` (codex) | 2 |
| Finalization | `review-analyzer`, `review-judge`, `convergence-judge` | 3 |

## Usage

```
crucible review --branch          # long pack (default, ~6-13 agents)
crucible review --branch --short  # short pack (fast, ~5-8 agents)
```

## Plugin Configuration

All role-plugin assignments are configurable via `.crucible.toml` under `[task_packs.review]`:

```toml
[task_packs.review]
analyzer_plugin = "opencode-glm"           # change-intent-extractor
program_semantics_plugin = "opencode-glm"
maintainer_review_plugin = "codex"
security_reliability_plugin = "opencode-kimi"
requirements_contract_plugin = "codex"
test_evidence_plugin = "codex"
performance_resource_plugin = "opencode-glm"
contrarian_review_plugin = "opencode-glm"
verification_review_plugin = "codex"
fix_strategy_plugin = "codex"
judge_plugin = "codex"                     # review-judge, structurizer, autofix
convergence_plugin = "codex"
short_review = false                       # set true for default short pack
```
