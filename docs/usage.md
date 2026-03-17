# Crucible CLI Usage

This document mirrors the CLI behavior and is kept short and operational.

## Quick Start

```bash
# Install (Nix)
nix profile install github:jonochang/crucible

# Generate config
crucible config init

# Run a review (defaults to branch+local against the remote default branch when available)
crucible review
```

## Commands

### `crucible review`

```bash
crucible review [PR] [--local] [--repo] [--branch [base]] [--files <paths...>] [--hook] [--json] [--verbose] [--interactive] [--export-issues <path>] [--reviewer <id>] [--max-rounds <n>] [--git-remote <name>]
```

Behavior:
- Default mode reviews current branch plus local changes against the remote default branch when available; otherwise it falls back to local uncommitted changes.
- If stdout is a TTY and `--hook` is not set, it launches the TUI.
- `--json` prints the full report as JSON (no TUI).
- `--hook` sets the exit code based on the verdict.
- `--verbose` streams agent stdout/stderr for debugging.

### `crucible hook`

```bash
crucible hook install [--force]
crucible hook uninstall
crucible hook status
```

Behavior:
- `install` writes `.git/hooks/pre-push` and runs `just crucible-pre-push`.
- The default `crucible-pre-push` recipe runs `crucible review --local --hook --reviewer claude-code --max-rounds 1`.
- If a pre-push hook exists and is not managed by Crucible, use `--force` to overwrite.
- `uninstall` only removes hooks managed by Crucible.
- `status` prints whether the hook is installed and whether `crucible` and `just` are on `PATH`.

### `crucible config`

```bash
crucible config init
crucible config validate
```

Behavior:
- `init` writes `.crucible.toml` in the current repo and fails if it already exists.
- `validate` loads config (local or global) and prints `Config OK` on success.

### `crucible session`

```bash
crucible session list
crucible session resume <id>
crucible session delete <id>
```

Behavior:
- Session commands currently return an error because they are not implemented yet.

### `crucible version`

```bash
crucible version
crucible --version
```

## Exit Codes

| Command | Exit code | Meaning |
|--------|----------|---------|
| `crucible review --hook` | `0` | Verdict is Pass or Warn |
| `crucible review --hook` | `1` | Verdict is Block |

Other commands exit with `0` on success and non-zero on error.

## Configuration Loading

Crucible loads config in this order:
- `.crucible.toml` in the current directory or any parent directory (first match wins)
- `~/.config/crucible/config.toml`
- Built-in defaults if no config file is found

Run `crucible config init` to generate a local config with defaults.
