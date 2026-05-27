# Review Pack Roles

The `review` pack has two variants: **long** (default) and **short** (`--short` flag).

## Long Review Pack (default)

Orchestrates 15 roles across a pre-round context extractor, a gate round, 3 review rounds, and finalization.

The key design principle: **check intent and simplicity first**. If the change is misaligned with its purpose or unnecessarily complex, the review exits early without running expensive detailed agents.

### Pre-Round — Context Extraction (Analyzer)

The Change Intent Analyst runs before any reviewer, producing a shared review brief.

| Role ID | Name | Focus | Default Plugin |
|---|---|---|---|
| `change-intent-extractor` | Change Intent Analyst | PR purpose, affected contracts, changed APIs, hidden assumptions, required invariants, migration/runtime implications | `opencode-glm` |

### Round 1 — Intent and Simplicity Gate (`gate: true`)

Runs first. If either reviewer finds a **Critical**-severity issue, the review stops immediately
with a "simplify before review" verdict — no further rounds execute.

| Role ID | Name | Focus | Weight | Default Plugin |
|---|---|---|---|---|
| `intent-alignment-review` | Intent Alignment Reviewer | Whether the implementation matches the change's stated purpose. Flag scope creep, missing behaviour, over-engineering, misalignment with documented intent | 2.0 | `opencode-glm` |
| `simplicity-review` | Simplicity Reviewer | Whether the implementation is the simplest possible approach. Flag unnecessary abstraction, avoidable indirection, over-generalisation, reinvention of existing utilities, complexity exceeding the problem scope | 2.0 | `codex` |

### Round 2 — Discovery (6 agents)

Six agents independently review the diff from different perspectives.

| Role ID | Name | Focus | Weight | Default Plugin |
|---|---|---|---|---|
| `program-semantics` | Program Semantics Auditor | State transitions, invariants, hidden regressions, control-flow, concurrency | 1.5 | `opencode-glm` |
| `maintainer-review` | Principal Maintainer | API contracts, refactor safety, maintainability, change surface, design coherence, migration safety | 1.5 | `codex` |
| `security-reliability` | Security and Reliability Auditor | Trust boundaries, authorization, secrets, injection, exposure, failure modes, recovery, retry storms, resource leaks | 1.5 | `opencode-kimi` |
| `requirements-contract-review` | Requirements and Contract Reviewer | User-visible behavior, business rules, domain invariants, acceptance criteria, intent-vs-implementation gaps | 1.5 | `codex` |
| `test-evidence-review` | Test Evidence Reviewer | Missing tests, weak assertions, false confidence from over-mocking, missing negative/boundary/authorization/migration tests | 1.25 | `codex` |
| `performance-resource-review` | Performance and Resource Reviewer | N+1 queries, unbounded loops, large allocations, missing indexes, lock contention, handle leaks, blocking I/O | 1.0 | `opencode-glm` |

### Round 3 — Challenge and Verify (2 agents)

| Role ID | Name | Focus | Weight | Default Plugin |
|---|---|---|---|---|
| `contrarian-review` | Contrarian Systems Reviewer | Challenge assumptions, cross-file interactions, integration risk, deployment-order failures, background job/cache/migration interactions | 1.25 | `opencode-glm` |
| `verification-review` | Verification Reviewer | Validate prior findings, prune weak claims, confirm evidence, surface duplicates, verify fixes don't introduce new bugs | 1.25 | `codex` |

### Round 4 — Fix Planning (1 agent)

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

Same as the original 8-role, 2-round pack for fast reviews. No gate round.

| Round | Roles | Agents |
|---|---|---|
| 1 — Initial Review | `program-semantics` (opencode-glm), `maintainer-review` (codex), `security-reliability` (opencode-kimi) | 3 |
| 2 — Challenge & Verify | `contrarian-review` (opencode-glm), `verification-review` (codex) | 2 |
| Finalization | `review-analyzer`, `review-judge`, `convergence-judge` | 3 |

## Usage

```
crucible review --branch          # long pack (default, gate + 7-15 agents)
crucible review --branch --short  # short pack (fast, ~5-8 agents, no gate)
```

## Plugin Configuration

All role-plugin assignments are configurable via `.crucible.toml` under `[task_packs.review]`:

```toml
[task_packs.review]
analyzer_plugin = "opencode-glm"           # change-intent-extractor
intent_alignment_plugin = "opencode-glm"   # gate round
simplicity_review_plugin = "codex"         # gate round
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
