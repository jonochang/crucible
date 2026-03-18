# Troubleshooting

## Agent command not found

Ensure the configured agent CLIs are installed and available on `PATH`.

## Hook does not run

Check `crucible hook status` and verify the repository hook path has not been overridden.

## No TUI appears

Crucible only launches the TUI when stdout is a terminal and `--hook` is not set.

## Review output is too sparse

Run with `--verbose` to stream live agent diagnostics. Use `--debug` when you need the raw prompt/response exchange and parser traces in `.crucible/runs/<run_id>/debug.log`. For phase-by-phase review flow and the final report sections, inspect `review_report.log` or `.crucible/runs/<run_id>/progress.log`.
