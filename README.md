# crucible

An autonomous, multi-agent code review swarm that tests and hardens your code before it ever reaches production.

Crucible runs locally, gathers rich context, asks a council of CLI-driven agents to review your diff, and can apply an auto-fix diff in a terminal UI.

## Quick Start

```bash
# Install (Nix)
nix profile install github:jonochang/crucible

# Generate a local config
crucible config init

# Run a review (TUI if stdout is a terminal)
crucible review

# Build
cargo build --release

# JSON output for scripting/CI
crucible review --json

# Install a git pre-push hook
crucible hook install
```

## Requirements

- Rust toolchain (or `nix develop`)
- Git repository (Crucible reviews the working tree diff vs `HEAD`)
- Agent CLIs on `PATH`: `claude`, `codex`, `gemini`, `opencode`

Crucible talks to these tools via stdin/stdout. Each CLI must return strict JSON as documented below.

## What It Does

- Builds a unified diff from your working tree and index.
- Collects context in parallel: symbol references, recent commit history, and docs snippets.
- Runs a pre-analysis phase to generate focus areas.
- Asks each configured agent to produce structured findings.
- Clusters findings by location/message similarity and computes consensus.
- Optionally asks the judge agent for an auto-fix unified diff.
- Presents results in a TUI with `[Enter]` to apply the patch.

## Example Output

```
Crucible Review — 3 findings (1 Critical, 1 Warning, 1 Info)

  [CRITICAL]  src/auth.rs:47        Token unwrap without validation
  [WARNING ]  src/auth.rs:83        Missing error propagation
  [INFO    ]  src/auth.rs:12        Unused import

Auto-fix available. Run with TUI to apply: crucible review

Verdict: BLOCK
```

## Configuration

Run `crucible config init` to generate `.crucible.toml` in your repo. Crucible searches the current directory and parents for `.crucible.toml`, then falls back to `~/.config/crucible/config.toml`, otherwise defaults are used.

Example (default values):

```toml
[crucible]
version = "1"

[gate]
enabled = true
untangle_bin = "untangle"

[context]
reference_max_depth = 2
reference_max_files = 30
history_max_commits = 20
history_max_days = 30
docs_patterns = ["docs/**/*.md", "README.md", "ARCHITECTURE.md"]
docs_max_bytes = 50000

[coordinator]
max_rounds = 2
quorum_threshold = 0.75
agent_timeout_secs = 90
devil_advocate = false

[verdict]
block_on = "Critical"

[rate_limits]
anthropic_rpm = 50
google_rpm = 60
openai_rpm = 60

[plugins]
agents = ["claude-code", "codex", "gemini"]
judge = "claude-code"
analyzer = "claude-code"
paths = []

[plugins.claude-code]
command = "claude"
args = []
persona = "Security Auditor"
role_weight = 2.0

[plugins.codex]
command = "codex"
args = []
persona = "Architecture Lead"
role_weight = 1.5

[plugins.gemini]
command = "gemini"
args = []
persona = "Performance Optimizer"
role_weight = 1.5

[plugins.open-code]
command = "opencode"
args = []
persona = "Correctness Reviewer"
role_weight = 1.0
```

### CLI Agent Contract

Each agent is invoked with a prompt on stdin. The response **must** be valid JSON on stdout:

```json
{
  "findings": [
    {
      "severity": "Critical | Warning | Info",
      "file": "<relative path or null>",
      "line_start": 1,
      "line_end": 1,
      "message": "<concise, actionable description>",
      "confidence": "High | Medium | Low"
    }
  ]
}
```

For auto-fix requests, agents must return:

```json
{ "unified_diff": "...", "explanation": "..." }
```

## Commands

### `crucible review`

Runs a multi-agent review of your working tree diff vs `HEAD`.

```bash
crucible review [--hook] [--json] [--verbose]
```

Behavior:
- If stdout is a TTY and `--hook` is not set, it launches the TUI.
- `--json` prints the full report as JSON (no TUI).
- `--hook` sets the exit code based on the verdict (see Exit Codes).
- `--verbose` streams agent stdout/stderr to help debug CLI integrations.
- Progress and the final JSON report are appended to `review_report.log` in the current directory.

### `crucible hook`

Manage a managed `pre-push` hook that runs `crucible review --hook`.

```bash
crucible hook install [--force]
crucible hook uninstall
crucible hook status
```

Behavior:
- `install` writes `.git/hooks/pre-push`. If the hook exists and is not managed by Crucible, use `--force` to overwrite.
- `uninstall` only removes hooks managed by Crucible.
- `status` prints whether the hook is installed and whether `crucible` is on `PATH`.

### `crucible config`

Manage configuration files.

```bash
crucible config init
crucible config validate
```

Behavior:
- `init` writes `.crucible.toml` in the current repo. It fails if the file already exists.
- `validate` loads config (local or global) and prints `Config OK` on success.

### `crucible session`

Session management is reserved for future releases.

```bash
crucible session list
crucible session resume <id>
crucible session delete <id>
```

### `crucible version`

Prints the CLI version.

```bash
crucible version
crucible --version
```

## Hook Integration

`crucible hook install` writes a managed `.git/hooks/pre-push` that runs `crucible review --hook`.

| Verdict | Exit code |
|---------|-----------|
| Pass | `0` |
| Warn | `0` |
| Block | `1` |

## Design Docs

- `docs/specs/brief.md`
- `docs/specs/design.md`
- `docs/specs/roadmap.md`
- `docs/usage.md`

## License

MIT
