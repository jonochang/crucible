# Troubleshooting

## Agent command not found

Ensure the configured agent CLIs are installed and available on `PATH`.

## Hook does not run

Check `crucible hook status` and verify the repository hook path has not been overridden.

## No TUI appears

Crucible only launches the TUI when stdout is a terminal and `--hook` is not set.

## Review output is too sparse

Run with `--verbose` to stream agent diagnostics and `--debug` to capture prompts and raw agent output.
