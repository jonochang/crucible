# Prompt/Orchestration Findings: Magpie vs Crucible

## Executive Summary

Magpie currently has a more advanced prompt-orchestration pipeline than Crucible, especially in:

- LLM-based convergence judging
- Dedicated issue structurization
- Rich analyzer context (modules/call-chain/patterns)

Crucible is stronger in strict JSON contracts and simpler deterministic runtime behavior, but is less mature in judge/structurizer depth.

## Comparison Findings

### 1) Analyzer Design

Magpie:
- Rich analyzer prompt (`affectedModules`, `callChain`, `designPatterns`, long summary)
- Strong system-context framing for reviewers

Crucible:
- Compact analyzer contract (`summary`, `focus_items`, `trade_offs`)
- Good for reliability, but less architectural depth

Finding:
- Crucible should expand analyzer schema to include module impact and call-chain context.

### 2) Reviewer Prompting

Magpie:
- Explicit exhaustive Round 1
- Explicit adversarial Round 2+ with prior-round-only visibility

Crucible:
- Similar round framing and prior-round constraints
- Stronger strict JSON response requirement (`narrative + findings[]`)

Finding:
- Crucible reviewer prompting is directionally good; role specialization can be improved per agent.

### 3) Convergence/Judge

Magpie:
- Uses explicit LLM consensus judge prompt
- Requires strict terminal verdict token (`CONVERGED` / `NOT_CONVERGED`)

Crucible:
- Uses heuristic convergence logic (net-new findings)
- Has token parser helper, but no active LLM convergence judge stage

Finding:
- This is the biggest gap. Crucible should add a real judge-prompt convergence phase.

### 4) Issue Structurization

Magpie:
- Dedicated structurizer prompt extracts normalized issue schema from review text

Crucible:
- Directly consumes reviewer findings; dedups afterward
- No separate LLM structurization pass

Finding:
- Add optional structurizer stage for richer normalized issues and better downstream outputs.

### 5) UX/Runtime

Magpie:
- Strong spinner/status-driven flow and sectioned progress

Crucible:
- Recent parity improvements are good (status lines, convergence events, TUI spinner/color)

Finding:
- Crucible UX is catching up; the remaining gap is mostly prompt/judge sophistication.

## Recommended Alignment Plan

1. Implement LLM convergence judge stage.
- Prompt with strict criteria and final verdict token.

2. Implement dedicated issue structurizer stage.
- Produce canonical issue objects:
  - `severity, category, file, line_start, line_end, title, description, suggested_fix, raised_by`

3. Expand analyzer schema.
- Add:
  - `affected_modules`
  - `call_chain`
  - `design_patterns`
  - richer reviewer focus checklist

4. Strengthen role-specialized reviewer prompts.
- Claude: correctness/security
- Codex: architecture/maintainability
- Gemini: performance/edge-cases

## Improvements Beyond Magpie

1. Evidence-backed findings.
- Require each issue to reference exact diff locations and quote anchors.

2. Confidence calibration.
- Downweight low-confidence singleton findings unless corroborated.

3. Deterministic pre-check fusion.
- Feed local static/tool signals into reviewer context, including:
  - `untangle`
  - language linters/formatters
  - type checks
  - targeted test runs

4. Prompt evaluation harness.
- Golden PR set + expected issues to track quality drift over time.

5. Adaptive cost/latency control.
- Chunk large diffs and early-stop when converged with no new high-severity issues.

6. Actionable final mode.
- Emit prioritized fix plan and ready-to-post PR comment artifacts.

## Priority Order

1. Actionable final mode (item 6) for high-value end-user output first.
2. LLM convergence judge (item 1) to improve review quality/control.
3. Structurizer stage (item 2).
4. Deterministic pre-check fusion with `untangle` + linters/type/tests (item 3).
5. Analyzer schema expansion.
6. Role-specialized reviewer prompts.
7. Prompt evaluation harness (item 4) - later phase.
8. Adaptive cost/latency controls (item 5) - later phase.
