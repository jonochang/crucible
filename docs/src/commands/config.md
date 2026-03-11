# `crucible config`

Configuration management commands.

```bash
crucible config init
crucible config validate
```

## Behavior

- `init` writes `.crucible.toml` for the current repository and fails if the file already exists.
- `validate` loads the active configuration and reports whether it is valid.
