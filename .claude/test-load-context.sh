#!/usr/bin/env bash
# Offline integration test of the real hook, with local bare remotes only.
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
hook="${1:-$repo_root/.claude/load-context.sh}"
mode="${2:-repository}"
case "$hook" in /*) ;; *) hook="$repo_root/$hook" ;; esac
scratch="$(mktemp -d)"
trap 'rm -rf -- "$scratch"' EXIT
export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL="$scratch/gitconfig"
export GIT_ALLOW_PROTOCOL=file GIT_TERMINAL_PROMPT=0
unset GIT_DIR GIT_WORK_TREE GIT_CONFIG_COUNT GIT_CONFIG_PARAMETERS
git config --global user.name 'Context hook fixture'
git config --global user.email 'fixture@example.invalid'
git config --global commit.gpgsign false
git config --global init.defaultBranch main
mkdir -p "$scratch/remotes" "$scratch/nonrepo" "$scratch/workspace"
for name in context agent-workflows; do
  git init --quiet --bare "$scratch/remotes/$name.git"
  git init --quiet "$scratch/seed-$name"
  printf '%s\n' "$name fixture" > "$scratch/seed-$name/README.md"
  if [ "$name" = context ]; then
    printf '%s\n' 'fixture constitution' > "$scratch/seed-$name/CONSTITUTION.md"
    printf '%s\n' 'fixture INDEX' > "$scratch/seed-$name/INDEX.md"
  fi
  git -C "$scratch/seed-$name" add .
  git -C "$scratch/seed-$name" commit --quiet -m fixture
  git -C "$scratch/seed-$name" push --quiet "$scratch/remotes/$name.git" main
  # Keep the hook's canonical origin URL and validation intact. Only Git's
  # transport is redirected; file-only protocol admission forbids network use.
  git config --global "url.file://$scratch/remotes/$name.git.insteadOf" "https://github.com/oyzuai/$name.git"
done
export CLAUDE_PROJECT_DIR="$scratch/workspace/$(basename -- "$repo_root")"
export XDG_CACHE_HOME="$scratch/cache"
git init --quiet "$CLAUDE_PROJECT_DIR"
git -C "$CLAUDE_PROJECT_DIR" commit --quiet --allow-empty -m project
git -C "$CLAUDE_PROJECT_DIR" remote add origin "https://github.com/oyzuai/$(basename -- "$repo_root").git"
if [ "$mode" = cloud ]; then unset CLAUDE_PROJECT_DIR; fi
run_hook() {
  if (cd -- "$scratch/nonrepo" && bash "$hook") > "$scratch/stdout" 2> "$scratch/stderr"; then
    hook_status=0
  else
    hook_status=$?
  fi
}
fail() {
  printf 'FAIL: %s\n' "$*" >&2
  cat "$scratch/stdout" "$scratch/stderr" >&2
  exit 1
}
assert_success() {
  [ "$hook_status" = 0 ] || fail "hook exited $hook_status"
  grep -Fxq "context commit $expected" "$scratch/stdout" || fail 'missing exact context SHA'
  grep -Fxq "$constitution" "$scratch/stdout" || fail 'missing committed constitution'
  grep -Fxq 'fixture INDEX' "$scratch/stdout" || fail 'missing committed INDEX'
  if grep -q '^ERROR:' "$scratch/stdout"; then fail 'unexpected ERROR on success'; fi
}
assert_failure() {
  grep -q '^ERROR: shared oyzuai/context' "$scratch/stdout" || fail 'no visible session ERROR'
  grep -q '^ERROR: shared oyzuai/context' "$scratch/stderr" || fail 'no stderr ERROR'
  # A nonzero exit OR this explicit marker is required. Repository work can
  # remain usable offline, but loading failure must never be silent.
  if [ "$hook_status" = 0 ]; then
    grep -q 'context load FAILED' "$scratch/stdout" || fail 'silent successful exit'
  fi
  if grep -q '^context commit ' "$scratch/stdout"; then fail 'failure advertised a loaded snapshot'; fi
}
constitution='fixture constitution'
expected="$(git -C "$scratch/seed-context" rev-parse HEAD)"
run_hook
assert_success
printf 'PASS: clone prints constitution, INDEX and exact SHA (%s)\n' "$mode"
constitution='fixture constitution updated'
printf '%s\n' "$constitution" > "$scratch/seed-context/CONSTITUTION.md"
git -C "$scratch/seed-context" commit --quiet -am refresh
git -C "$scratch/seed-context" push --quiet "$scratch/remotes/context.git" main
expected="$(git -C "$scratch/seed-context" rev-parse HEAD)"
run_hook
assert_success
printf 'PASS: refresh prints the updated committed snapshot\n'
mv "$scratch/remotes/context.git" "$scratch/remotes/context-unavailable.git"
run_hook
assert_failure
printf 'PASS: unavailable cached remote is visible on both streams\n'
rm -rf -- "$scratch/workspace" "$scratch/cache"
mkdir -p "$scratch/workspace"
if [ "$mode" = repository ]; then
  git init --quiet "$CLAUDE_PROJECT_DIR"
  git -C "$CLAUDE_PROJECT_DIR" commit --quiet --allow-empty -m project
fi
run_hook
assert_failure
printf 'PASS: missing remote on a fresh session is visible on both streams\n'
mv "$scratch/remotes/context-unavailable.git" "$scratch/remotes/context.git"
git -C "$scratch/seed-context" rm --quiet INDEX.md
git -C "$scratch/seed-context" commit --quiet -m 'missing required document'
git -C "$scratch/seed-context" push --quiet "$scratch/remotes/context.git" main
run_hook
assert_failure
printf 'PASS: missing committed INDEX is visible and prints no success SHA\n'
