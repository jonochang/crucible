# Installation

## Nix

```bash
nix profile install github:jonochang/crucible
```

## From source

```bash
git clone https://github.com/jonochang/crucible
cd crucible
cargo build --release
```

## Development shell

The repository ships a Nix development shell with Rust tooling, `mdbook`, `gh`, and `untangle`:

```bash
nix develop
```

## Runtime requirements

- Git repository with changes to review
- Rust toolchain if building from source
- Agent CLIs on `PATH`: `claude`, `codex`, `gemini`, `opencode`
- `just` on `PATH` for the managed pre-push hook workflow
