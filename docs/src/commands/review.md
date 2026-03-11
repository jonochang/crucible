# `crucible review`

Runs a multi-agent review of the selected diff target.

```bash
crucible review [PR] [--local] [--repo] [--branch [base]] [--files <paths...>] [--hook] [--json] [--verbose] [--debug] [--interactive] [--reviewer <id>] [--max-rounds <n>] [--export-issues <path>] [--output-report <path>] [--github-dry-run] [--publish-github]
```

## Behavior

- `PR` reviews a pull request via `gh`.
- `--local` reviews local uncommitted changes.
- `--repo` reviews the current branch against the remote default branch.
- `--branch [base]` reviews against a chosen base branch.
- `--files <paths...>` restricts the diff to selected files.
- TTY output launches the TUI unless `--hook` or `--json` is used.
- `--interactive` keeps the final TUI screen open after completion.
- `--json` emits the serialized report.
- `--hook` makes the exit code suitable for git hooks.
- `--debug` writes prompts and raw agent I/O under `.crucible/runs/<run_id>/debug.log`.

## Artifacts

Each run writes scoped output under `.crucible/runs/<run_id>/`, including progress logs and the final report.
