# Configuration

Run `crucible config init` to generate a starter `.crucible.toml`.

Crucible looks for configuration in this order:

1. `.crucible.toml` in the current directory or a parent directory
2. `~/.config/crucible/config.toml`
3. built-in defaults

## Example

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
max_diff_lines_per_chunk = 1200
max_diff_chunks = 6
enable_structurizer = true

[verdict]
block_on = "Critical"

[prechecks]
enabled = true
include_untangle = true
include_linters = true
include_type_checks = true
include_tests = true
timeout_secs = 30

[plugins]
agents = ["claude-code", "codex", "gemini"]
judge = "claude-code"
analyzer = "claude-code"
paths = []
```

## Plugin stanzas

Each plugin can define a command, arguments, persona, and role weighting. The agent command must accept prompt text on stdin and emit strict JSON on stdout.
