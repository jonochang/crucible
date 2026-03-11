# Quick Start

## First run

```bash
# Generate local config
crucible config init

# Review local changes
crucible review
```

## Common invocations

```bash
# Keep the final TUI screen open
crucible review --interactive

# Emit JSON instead of running the TUI
crucible review --json

# Review a pull request without publishing comments
crucible review 123 --github-dry-run

# Publish a structured GitHub review
crucible review 123 --publish-github

# Install the managed pre-push hook
crucible hook install
```

## Docs development

```bash
just docs-serve
```
