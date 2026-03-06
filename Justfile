set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

crucible-pre-push:
  if [[ -z "$(git status --porcelain --untracked-files=all)" ]]; then
    echo "crucible: no local diff detected; skipping review"
    exit 0
  fi
  exec crucible review --local --hook --reviewer claude-code --max-rounds 1
