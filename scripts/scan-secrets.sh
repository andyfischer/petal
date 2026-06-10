#!/usr/bin/env bash
#
# scan-secrets.sh — scan the full git commit history for leaked credentials.
#
# Mirrors what the "Secret scan" GitHub Action does, for local use before a
# push or a public release. Uses gitleaks with the project config
# (.gitleaks.toml). Resolution order:
#
#   1. a `gitleaks` binary already on PATH
#   2. docker (pulls the official zricethezav/gitleaks image)
#
# Exits non-zero if any secret is found.
#
# Usage:
#   ./scripts/scan-secrets.sh            # scan entire history
#   ./scripts/scan-secrets.sh --staged   # scan only staged changes (pre-commit)

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

config="$repo_root/.gitleaks.toml"
mode="detect"          # scan full history by default
extra_args=()

if [[ "${1:-}" == "--staged" ]]; then
  mode="protect"
  extra_args+=(--staged)
fi

run_native() {
  echo "==> Running gitleaks ($mode) via local binary"
  gitleaks "$mode" --source "$repo_root" --config "$config" --redact -v "${extra_args[@]}"
}

run_docker() {
  echo "==> Running gitleaks ($mode) via docker"
  docker run --rm -v "$repo_root:/repo" -w /repo \
    zricethezav/gitleaks:latest \
    "$mode" --source /repo --config /repo/.gitleaks.toml --redact -v "${extra_args[@]}"
}

if command -v gitleaks >/dev/null 2>&1; then
  run_native
elif command -v docker >/dev/null 2>&1; then
  run_docker
else
  echo "ERROR: neither 'gitleaks' nor 'docker' is available." >&2
  echo "Install gitleaks (https://github.com/gitleaks/gitleaks) or Docker, then re-run." >&2
  exit 127
fi

echo "==> No leaks found."
