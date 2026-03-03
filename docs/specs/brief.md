# Product Brief: Crucible

**Tagline:** An autonomous, multi-agent code review swarm that tests, debates, and self-heals your code before it ever reaches production.

**Ecosystem:** Part of the **Silverforge** software factory (paired with **Untangle**).

---

## 1. Vision & Philosophy
In the **Silverforge** ecosystem, if **Untangle** is the tool that sorts the raw ore to ensure structural integrity, **Crucible** is the extreme heat. It is an agentic code review framework that subjects your code to intense, adversarial scrutiny.

Crucible moves beyond standard "AI PR reviewers" that just dump a laundry list of nitpicky comments on GitHub. By combining continuous local interception, multi-agent debate, and proactive auto-remediation, Crucible doesn't just review code—it purifies it.

---

## 2. Core Mechanics: The Best of Magpie & Roborev

Crucible takes the **frictionless, action-oriented workflow of Roborev** and fuses it with the **rigorous, multi-perspective intelligence of Magpie**.

### The Roborev DNA (Frictionless Action)
- **Local & Continuous:** Crucible can run as a background daemon, hooking into local `git` events (pre-commit/pre-push) rather than waiting for CI. It catches bugs while the context is still fresh in the developer's mind.
- **Terminal UI (TUI):** Developers manage a queue of reviews and interact with the agents directly from their terminal without breaking flow.
- **Auto-Remediation:** It doesn't just complain; it generates diffs. Like Roborev's "refine" feature, Crucible offers self-healing auto-fixes that the developer can accept with a single keystroke.

### The Magpie DNA (Rigorous Debate)
- **The Fair Debate Model:** Instead of a single LLM, Crucible uses a "Hub-and-Spoke" orchestrator. It spawns a specialized council of agents (e.g., Security Auditor, Performance Optimizer, Architecture Lead) who review the diff simultaneously.
- **Anti-Sycophancy Orchestration:** Agents do not talk directly to each other. The central orchestrator feeds their findings back to the group in distinct rounds, forcing them to debate disagreements.
- **Judge/Summarizer:** Once consensus is reached (or max rounds hit), a final "Judge" agent compiles the transcript into a single, unified action plan.

---

## 3. The Crucible Workflow

When a developer saves changes or attempts a push, the Crucible workflow triggers:

### Phase 1: The Deterministic Gate (Powered by Untangle)
Before spending a single AI token, Crucible silently triggers **Untangle** in the background.
- Untangle does the fast math: *Did this change introduce a circular dependency (SCC)? Did it cause a massive fan-out regression?*
- If Untangle fails, the process halts immediately. The developer is notified via the TUI to fix their architectural debt.
- If Untangle passes, its dependency graph is serialized and injected into the Crucible's context window, giving the AI deep architectural awareness.

### Phase 2: The Forge (Multi-Agent Debate)
Crucible's orchestrator takes the diff, the Untangle context graph, and the codebase state, then passes it to the Council.
- **Round 1:** The agents independently analyze the code.
- **Round 2:** The orchestrator cross-pollinates their findings. *("The Performance agent suggested caching this, but the Security agent flagged a potential data leak. Debate.")*
- **Round 3:** Convergence is detected. The Judge agent declares a consensus.

### Phase 3: The Quench (Auto-Remediation)
The developer's TUI pings with a notification:
> *"Crucible identified a race condition in `auth.rs`. The Security and Architect agents agreed on a fix. Press [Enter] to apply the patch, or [C] to join the chat session to discuss."*

The developer can interactively chat with the Judge agent to tweak the solution, or simply accept the ready-to-commit diff.

---

## 4. Key Differentiators

| Feature | Standard AI Reviewers | Crucible |
| :--- | :--- | :--- |
| **Location** | PR/Cloud | Local Terminal (Daemon/TUI) & CI |
| **Analysis Level** | Superficial (Regex/Single LLM) | Deep (Structural via Untangle + Multi-LLM Debate) |
| **Communication** | Direct comments on GitHub | Central orchestrator ensures model consensus |
| **Output Type** | Text comments ("Fix this") | Actionable Diffs ("I fixed this, press Y to accept") |
| **Sycophancy** | High (Models blindly agree with code) | Low (Forced adversarial debate rounds) |

---

## 5. Architecture Summary

- **Engine:** Central Node.js, Rust, or Python orchestrator managing state in memory (no direct agent-to-agent file sharing).
- **Tool Calling:** Agents are equipped with tools, most notably the ability to run `untangle diff` to test if their proposed fixes will break the dependency graph.
- **Interface:** A fast, responsive TUI (Terminal User Interface) for managing the local daemon and reviewing proposed patches.
- **Agent Topology:**
  - *N* Debater Agents (distinct prompts/personas, potentially distinct models)
  - 1 Orchestrator (manages state and rounds)
  - 1 Summarizer/Judge (calls the auto-fix tool)
