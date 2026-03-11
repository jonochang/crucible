# `crucible hook`

Managed git hook workflow.

```bash
crucible hook install [--force]
crucible hook uninstall
crucible hook status
```

## Behavior

- `install` writes a managed pre-push hook.
- `uninstall` removes only hooks that Crucible manages.
- `status` reports whether the hook is installed and whether `crucible` is available on `PATH`.

The repository also includes a `just` target for a managed pre-push review path.
