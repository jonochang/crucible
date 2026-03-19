set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

docs-serve:
  exec mdbook serve docs --open

docs-build:
  exec mdbook build docs

mutants:
  exec cargo mutants --workspace

mutants-fast:
  exec cargo mutants --workspace --package libcrucible --file 'crates/libcrucible/src/*.rs' --file 'crates/libcrucible/src/**/*.rs'

crucible-pre-push:
  if [[ -z "$(git status --porcelain --untracked-files=all)" ]]; then
    echo "crucible: no local diff detected; skipping review"
    exit 0
  fi
  exec crucible review --local --hook --reviewer claude-code --max-rounds 1
