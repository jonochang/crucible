# Cook → Crucible: Borrowed Ideas & Out-of-Distribution Concepts

> Cross-pollination analysis of [cook](~/lib/cook) (multi-agent workflow orchestrator)
> and [cook-cli](~/lib/cook-cli) (portable terminal AI agent) applied to Crucible's
> autonomous code review swarm.

---

## 1. Source Analysis

### Cook (~/lib/cook)

TypeScript CLI for composing multi-agent LLM workflows via a recursive AST:

- **Composable operators**: `work`, `repeat` (xN), `review` (iterative gate loop), `ralph` (task decomposition), `composition` (race/vs/pick/merge/compare)
- **Git worktree isolation**: each composition branch runs in its own worktree on a separate git branch
- **Resolvers**: `pick` (judge selects best), `merge` (judge synthesizes), `compare` (judge writes analysis)
- **Event-driven TUI**: Ink+React, decoupled from executor via EventEmitter
- **Rate-limit retry**: pattern detection + configurable exponential backoff (5 min intervals, 6 hr max)
- **Session logs as markdown**: agents read their own history via log file reference
- **COOK.md templates**: dynamic function-compiled system prompt templates with runtime variables
- **Docker sandboxing**: iptables-restricted containers with per-project images
- **RPI methodology**: Research-Plan-Implement with AI self-review gates

### Cook-cli (~/lib/cook-cli)

Portable Bun-based terminal AI agent built on Vercel AI SDK:

- **Approval flow with guidance**: user can decline a mutation and provide free-text guidance that's injected as a continuation message — the agent learns from rejections
- **Mutation tracking**: every mutating tool call recorded in `mutation_plan: MutationRecord[]`; `--dry-run` previews without executing
- **Path scoping**: `allow_outside_cwd` defaults to false; explicit opt-in for wider access
- **Command aliases**: `/alias-name` resolves to `.cook/commands/alias.md` templates
- **Provider auto-detection**: checks env vars in precedence order, selects first available
- **Session event logging**: append-only JSONL + metadata JSON; visualization via generated HTML
- **Atomic file operations**: edit via temp file + rename; prevents partial writes
- **Pipe-native**: `git diff | cook "review"` — stdin as context
- **Raw output mode**: `isFinal=true` on last bash call → raw stdout passthrough
- **Stateful confirmation**: conversation state preserved across approval loops

---

## 2. Direct Borrowing (high confidence, clear value)

### 2.1 Rate-Limit Retry with Backoff

**Source**: Cook's `retry.ts`

Crucible already has per-provider token-bucket rate limiting, but lacks **reactive retry** when an agent CLI returns a rate-limit error. Cook detects patterns (`rate limit`, `429`, `quota`, `overloaded`, `resource_exhausted`) in stderr and retries with configurable intervals.

**Implementation**:
- Add `RetryPolicy` to `PluginConfig`: `{ enabled: bool, poll_interval: Duration, max_wait: Duration }`
- In `CliAgentPlugin::invoke()`, capture stderr, match against patterns
- Emit `ProgressEvent::RateLimitWait { agent, retry_after, attempt }` for TUI countdown
- Retry up to `max_wait` with jitter

**Effort**: Small. Localized to `cli_agent.rs`.

### 2.2 Approval Flow for AutoFix

**Source**: Cook-cli's `approval-flow.ts`

Crucible currently presents autofix as accept-all-or-nothing. Cook-cli's pattern of per-mutation approval with guidance feedback is directly applicable.

**Implementation**:
- After `AutoFix` is computed, decompose into individual `FixHunk` items
- TUI presents each hunk: `[y] apply / [n] skip / [a] apply all / <text> guidance`
- If user provides guidance text, feed it back to the judge agent as a continuation message for a revised fix
- Track `AppliedFix` / `SkippedFix` / `RevisedFix` per hunk

**Effort**: Medium. Requires TUI state extension + judge re-invocation.

### 2.3 Pipe-Native Review

**Source**: Cook-cli's stdin handling

Enable `git diff HEAD~3 | crucible review` or `cat suspicious.rs | crucible review --files -`.

**Implementation**:
- Detect stdin pipe (non-TTY)
- If `--files -`, read file list from stdin
- If raw diff on stdin, skip git diff gathering, use piped diff directly
- Size threshold: inline if < 64KB, temp file if larger

**Effort**: Small. CLI parsing + context pipeline entry point.

### 2.4 Provider Auto-Detection

**Source**: Cook-cli's `portable-default.ts`

When no `.crucible.toml` exists, detect available API keys and auto-configure agents.

**Implementation**:
- Check env vars: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`, `GROQ_API_KEY`
- Map to agent plugins: anthropic → claude-code, openai → codex, etc.
- Auto-populate `PluginRegistry` with detected providers
- Emit `ProgressEvent::AutoDetected { providers: Vec<String> }`

**Effort**: Small. Config fallback path.

### 2.5 Session Event Streaming (JSONL)

**Source**: Cook-cli's `session-logger.ts`

Crucible has `ProgressEvent` but no persistent structured event log. JSONL is ideal for append-only event streams that can be replayed or analyzed.

**Implementation**:
- `SessionLogger` writes to `.crucible/runs/<run_id>/events.jsonl`
- Each `ProgressEvent` serialized with timestamp, seq number, event type, payload
- `session.json` metadata: config snapshot, agents, start time, git ref
- Future: `crucible session replay <run_id>` to re-render TUI from events

**Effort**: Small. Parallel write alongside existing progress channel.

### 2.6 Command Aliases / Review Profiles

**Source**: Cook-cli's command aliases + Cook's COOK.md templates

Allow saved review profiles: `.crucible/profiles/security.toml`, `.crucible/profiles/perf.toml`.

**Implementation**:
- `crucible review --profile security` loads `.crucible/profiles/security.toml`
- Profile overrides: agent selection, persona prompts, quorum threshold, severity gates
- `crucible review /security` shorthand (Cook-style slash syntax)
- Ship default profiles: `security`, `performance`, `correctness`, `architecture`

**Effort**: Small. Config layering.

### 2.7 Dry-Run Mode

**Source**: Cook-cli's `--dry-run`

Preview what a review will do without calling LLMs.

**Implementation**:
- `crucible review --dry-run` outputs:
  - Resolved config (agents, rounds, quorum)
  - Gathered context summary (N files changed, N references found, N history commits)
  - Estimated token usage per agent
  - Precheck results (these are free — run them even in dry-run)
- Exit before agent dispatch

**Effort**: Small. Early return in coordinator.

---

## 3. Architectural Transplants (medium confidence, requires adaptation)

### 3.1 Composable Review Operators (AST-based)

**Source**: Cook's recursive AST execution model

Cook's killer feature is composability: `cook "task" x3 review v3 pick "best"`. This maps surprisingly well to code review:

```
crucible review --strategy "analyze x2 debate v3 pick security"
```

Meaning: run analysis twice, then 3 independent debate tracks in parallel, judge picks the one with best security coverage.

**Operators**:
| Operator | Review Semantics |
|----------|-----------------|
| `xN` (repeat) | Run N independent review passes, merge findings |
| `vN` (version) | N parallel review tracks in isolation |
| `pick <criteria>` | Judge selects best review track |
| `merge` | Judge synthesizes findings from all tracks |
| `compare` | Side-by-side comparison report |
| `review` (iterative) | Debate rounds until convergence (already exists) |

**AST Node types**:
```rust
enum ReviewNode {
    Analyze,
    Debate { agents: Vec<AgentId> },
    Repeat { inner: Box<ReviewNode>, count: u8 },
    Parallel { branches: Vec<ReviewNode>, resolver: Resolver },
    Sequence(Vec<ReviewNode>),
}

enum Resolver { Pick(String), Merge, Compare }
```

**Implementation**: This is a generalization of the existing Coordinator. The current fixed pipeline (analyze → debate rounds → judge) becomes one possible AST: `Sequence([Analyze, Repeat(Debate, max_rounds)])`. The composable version lets users express more complex strategies.

**Effort**: Large. Refactors Coordinator into recursive executor.

### 3.2 Git Worktree Racing for Reviews

**Source**: Cook's worktree-based composition

Run multiple review strategies in parallel, each in its own worktree, to prevent cross-contamination of autofix proposals:

```
crucible review v3 merge
```

Each branch:
1. Gets its own worktree
2. Runs full review pipeline independently
3. Applies its own autofix to the worktree
4. Tests pass? Record as candidate

Judge merges the best fixes from all branches.

**Why worktrees for reviews?**: When multiple agents propose conflicting autofixes, they can't all be applied to the same working tree. Worktree isolation lets each fix be validated independently, then the best combination is cherry-picked.

**Effort**: Medium. Git worktree management + parallel coordinator instances.

### 3.3 Template-Driven Personas

**Source**: Cook's COOK.md dynamic template system

Replace hardcoded persona prompts with per-project templates:

```
.crucible/
  prompts/
    analyzer.md      # "You are reviewing ${repo_name}. Focus on ${focus_areas}..."
    debater.md       # "Round ${round}/${max_rounds}. Prior findings: ${prior_count}..."
    judge.md         # "Synthesize ${agent_count} reviewers' findings..."
    security.md      # Override for security-focused reviews
```

**Template variables**: `${repo_name}`, `${diff_stats}`, `${round}`, `${max_rounds}`, `${agent_id}`, `${persona}`, `${focus_areas}`, `${prior_findings_count}`, `${precheck_summary}`

**Implementation**:
- `PromptTemplate` struct with `render(ctx: &TemplateContext) -> String`
- Handlebars or simple `${var}` expansion (no arbitrary code like Cook's `new Function`)
- Fallback to built-in prompts when templates don't exist
- `crucible config init` generates default templates

**Effort**: Medium. Prompt construction refactor.

### 3.4 Iterative Fix-Validate Loop

**Source**: Cook's `agentLoop` (work → review → gate cycle)

After autofix is proposed, run a validation loop:

```
propose fix → apply to worktree → run tests → pass? done : iterate
```

```rust
struct FixLoop {
    max_iterations: u8,     // default 3
    validation: Vec<String>, // commands: ["cargo check", "cargo test"]
    gate: GateVerdict,       // PASS keywords
}
```

**Flow**:
1. Judge proposes `AutoFix`
2. Apply patch to temp worktree
3. Run validation commands
4. If all pass → `Verdict::FixValidated`
5. If fail → feed error output back to judge as context → iterate
6. Max iterations reached → `Verdict::FixUnvalidated` (still present fix, mark as unvalidated)

**Effort**: Medium. Worktree management + judge re-invocation loop.

### 3.5 Ralph-Style Task Decomposition for Large PRs

**Source**: Cook's `ralph` operator (task-by-task with gate)

For PRs touching 50+ files, decompose into reviewable chunks:

1. Analyzer produces `FocusAreas` as before
2. New: Analyzer also produces `ReviewPlan`: ordered list of review tasks
3. Coordinator processes tasks sequentially: review chunk → gate → next chunk
4. Gate decides: `NEXT` (move to next chunk) or `DEEPER` (re-review with more agents)
5. Final synthesis across all chunks

```rust
struct ReviewPlan {
    tasks: Vec<ReviewTask>,
}

struct ReviewTask {
    files: Vec<PathBuf>,
    focus: String,        // "Review auth middleware changes"
    priority: Priority,
}
```

**Effort**: Medium. Extends Analyzer output + Coordinator loop.

---

## 4. Out-of-Distribution Ideas (novel combinations)

### 4.1 Adversarial Red Team / Blue Team Reviews

Combine Cook's `vs` composition with Crucible's multi-agent debate:

- **Red team**: agents tasked with finding every possible issue (maximize findings)
- **Blue team**: agents tasked with defending the code (argue why findings are false positives)
- **Judge**: evaluates which red team findings survived blue team challenge

```
crucible review --strategy "red-blue"
```

This creates a structured adversarial dynamic rather than Crucible's current cooperative consensus. Findings that survive adversarial challenge have much higher confidence.

**Key insight**: Current debate rounds are cooperative — agents refine toward agreement. Red/blue forces genuine disagreement, surfacing edge cases that cooperative review misses.

### 4.2 Review Replay & Regression Testing

Combine Cook-cli's session JSONL with Crucible's prompt-eval harness:

- Record full review sessions as structured events
- Replay against new prompt versions to detect regressions
- Compare: did the new prompt find the same critical issues?
- Scoring: precision/recall against known-good review outcomes

```
crucible prompt-eval --replay .crucible/runs/<golden_run_id>/ --prompt-version v2
```

This turns past reviews into a test suite for prompt engineering.

### 4.3 Guidance-Aware Debate Rounds

Combine Cook-cli's approval-with-guidance with Crucible's debate rounds:

- After round 1, human can inject guidance: "focus more on SQL injection, less on style"
- Guidance is injected as a synthetic `FocusArea` override for round 2
- Agents see: "Human reviewer has directed attention to: SQL injection vectors"
- This creates a human-in-the-loop steering mechanism without the human doing the review

```
crucible review --interactive
# After round 1:
# [g] provide guidance / [c] continue / [s] stop
```

### 4.4 Cascading Review Depth

Combine Cook's `repeat` operator with dynamic resource allocation:

- Round 1: fast model (Haiku), all files, broad sweep
- If Haiku flags issues → Round 2: medium model (Sonnet), flagged files only
- If Sonnet confirms → Round 3: heavy model (Opus), confirmed issues only

```toml
[coordinator.cascade]
levels = [
    { model = "haiku", scope = "all", threshold = "any" },
    { model = "sonnet", scope = "flagged", threshold = "medium+" },
    { model = "opus", scope = "confirmed", threshold = "high+" },
]
```

**Why**: Most files in a PR are fine. Spending Opus tokens on trivial changes is wasteful. Cascading focuses expensive models where they matter.

### 4.5 Review Composition with External Tools

Combine Cook's composition resolvers with Crucible's precheck system:

```
crucible review --strategy "prechecks + (static-analysis vs llm-review) merge"
```

- Prechecks run first (free, deterministic)
- Static analysis (clippy, semgrep) runs in parallel with LLM review
- Merge resolver combines machine findings with LLM findings
- Deduplication: if clippy and Claude both flag the same issue, boost confidence

This positions Crucible as an orchestrator of *all* review signals, not just LLM outputs.

### 4.6 Differential Review Memory

Combine Cook-cli's session persistence with Crucible's temporal debate memory (P8):

- Store review outcomes per (file, function, issue-type) triple
- On re-review of same code: "This function was flagged for unchecked error handling in review #42. Was it fixed?"
- Agents receive historical review context for files they're reviewing
- Enables: "This PR introduces the same pattern that caused incident X"

```rust
struct ReviewMemory {
    entries: BTreeMap<(PathBuf, String), Vec<HistoricalFinding>>,
}

struct HistoricalFinding {
    run_id: Uuid,
    date: DateTime<Utc>,
    severity: Severity,
    message: String,
    was_fixed: Option<bool>,
}
```

### 4.7 Review Contracts (Specification-Driven Review)

Combine Cook's COOK.md templates with formal review specifications:

```markdown
<!-- .crucible/contracts/auth.md -->
# Auth Module Review Contract

## Invariants
- All endpoints must check authentication
- Tokens must be validated before use
- No plaintext secrets in logs

## Required Checks
- [ ] SQL injection in query parameters
- [ ] CSRF protection on state-changing endpoints
- [ ] Rate limiting on login attempts

## Severity Overrides
- Missing auth check → Critical (not Warning)
- Logging secrets → Critical
```

Agents receive the contract as additional context. Judge validates findings against the checklist. Report shows contract compliance: "3/5 required checks verified, 2 not applicable to this diff."

### 4.8 Live Fix Collaboration

Combine Cook-cli's guidance continuation with Crucible's autofix:

1. Agent proposes fix
2. Human edits the fix in their editor
3. Crucible detects the edit (filesystem watch)
4. Re-runs validation on the human-edited version
5. If tests pass → commit. If fail → agent sees human's attempt + error, proposes improvement

This creates a tight human-AI collaboration loop for complex fixes where neither human nor AI alone gets it right.

### 4.9 Multi-Repo Review Propagation

Combine Cook's worktree isolation with Crucible's cross-repo pattern detection (P12):

When a fix is applied in repo A, scan dependent repos for the same pattern:

```
crucible review --propagate ~/lib/repo-b ~/lib/repo-c
```

- Review repo A normally
- For each confirmed finding, search repos B and C for same pattern
- Generate propagation report: "Same vulnerable pattern found in repo-b/src/auth.rs:42"
- Optionally: auto-generate PRs for dependent repos

### 4.10 Review Tournament

Combine Cook's `pick` resolver with iterative elimination:

- Round 1: 4 agents review independently
- Round 2: Judge eliminates weakest 2 agents (by finding quality/precision)
- Round 3: Surviving 2 agents debate
- Final: Judge synthesizes

Tournament structure naturally surfaces the strongest reviewers for this specific codebase/change type. Over time, track which agents win tournaments for which file types → adaptive agent selection.

### 4.11 Sandboxed Fix Validation

**Source**: Cook's Docker sandboxing

Run proposed autofixes in an isolated container:

```toml
[coordinator.fix_validation]
sandbox = "docker"
image = "rust:1.80-slim"
commands = ["cargo check", "cargo test"]
timeout = 120
```

- Apply autofix patch in container
- Run validation commands
- Container has no network access (pure build/test)
- Results fed back to judge

**Why sandbox?**: Proposed fixes might introduce build breaks, test failures, or even malicious code (if agents are compromised). Sandboxed validation catches this before any code reaches the working tree.

### 4.12 Review Cost Estimation & Budgeting

Combine Cook-cli's dry-run with token economics:

```
crucible review --budget 50000  # max 50k tokens total
```

- Dry-run phase estimates token usage per agent per round
- If estimated cost exceeds budget: reduce agents, rounds, or context window
- Real-time token tracking during execution
- Report includes: `Total tokens: 42,381 / 50,000 budget`
- Historical cost tracking per repo/PR size for planning

---

## 5. Implementation Priority Matrix

| Idea | Value | Effort | Priority |
|------|-------|--------|----------|
| 2.1 Rate-limit retry | High | Small | P1 |
| 2.3 Pipe-native review | High | Small | P1 |
| 2.4 Provider auto-detect | High | Small | P1 |
| 2.7 Dry-run mode | High | Small | P1 |
| 2.5 JSONL event log | Medium | Small | P2 |
| 2.6 Review profiles | Medium | Small | P2 |
| 2.2 Per-hunk approval | High | Medium | P2 |
| 4.3 Guidance-aware rounds | High | Medium | P2 |
| 4.4 Cascading review depth | High | Medium | P3 |
| 3.3 Template personas | Medium | Medium | P3 |
| 3.4 Fix-validate loop | High | Medium | P3 |
| 4.7 Review contracts | High | Medium | P3 |
| 4.12 Cost budgeting | Medium | Small | P3 |
| 3.5 Ralph task decomp | Medium | Medium | P4 |
| 4.1 Red/blue adversarial | High | Large | P4 |
| 4.5 External tool composition | Medium | Medium | P4 |
| 4.2 Replay regression | Medium | Medium | P4 |
| 3.1 Composable AST | High | Large | P5 |
| 3.2 Worktree racing | Medium | Medium | P5 |
| 4.6 Differential memory | Medium | Large | P5 |
| 4.10 Review tournament | Medium | Large | P5 |
| 4.8 Live fix collab | Medium | Large | P6 |
| 4.9 Multi-repo propagation | Medium | Large | P6 |
| 4.11 Sandboxed validation | Medium | Large | P6 |

---

## 6. Key Takeaways

### What Cook does better than Crucible

1. **Composability**: Cook's recursive AST lets users express arbitrarily complex workflows. Crucible's fixed pipeline (analyze → debate → judge) is powerful but rigid.

2. **Resilience**: Cook's rate-limit retry is production-essential. Crucible's proactive rate limiting helps but doesn't handle reactive failures.

3. **Human-in-the-loop**: Cook-cli's guidance-aware approval flow is more sophisticated than Crucible's accept/reject. The agent learning from rejections is a powerful pattern.

4. **Zero-config start**: Cook-cli's provider auto-detection means `cook "review this"` works immediately. Crucible requires `.crucible.toml`.

### What Crucible does better

1. **Review-specific architecture**: Consensus tracking, per-issue clustering, evidence anchoring — these are domain-specific and Cook has no equivalent.

2. **Fairness guarantees**: MessageSnapshotter is a compile-time enforcement that Cook doesn't attempt.

3. **Pre-analysis phase**: FocusAreas as shared context prevents divergent framing. Cook agents operate independently.

4. **Structured outputs**: JSON schema enforcement vs Cook's keyword-based verdict parsing.

### The synthesis

Crucible's strength is depth (domain-specific review intelligence). Cook's strength is breadth (composable workflow orchestration). The highest-value ideas transfer Cook's workflow flexibility into Crucible's domain-specific framework — especially composable review strategies, human guidance loops, and resilience patterns.
