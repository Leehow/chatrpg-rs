#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 3 ]]; then
  echo "usage: $0 <repo-root> <work-id> <base-ref>" >&2
  exit 2
fi

repo_root="$(cd "$1" && pwd)"
work_id="$2"
base_ref="$3"
branch="codex/${work_id}-integration"
worktree="${repo_root}/.worktrees/${work_id}/integration"

cd "$repo_root"
git rev-parse --show-toplevel >/dev/null

if [[ -e "$worktree" ]]; then
  echo "integration worktree already exists: $worktree"
  git -C "$worktree" status --short
  exit 0
fi

mkdir -p "$(dirname "$worktree")"

if git show-ref --verify --quiet "refs/heads/$branch"; then
  git worktree add "$worktree" "$branch"
else
  git worktree add -b "$branch" "$worktree" "$base_ref"
fi

echo "created clean integration worktree"
echo "branch:   $branch"
echo "worktree: $worktree"
git -C "$worktree" status --short
