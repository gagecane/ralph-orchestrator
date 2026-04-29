#!/usr/bin/env bash
# Refinery pre-merge gate for ralph.
#
# Runs as the merge_queue gate from ~/gt/ralph/config.json. Invoked by the
# Gas Town refinery (via `sh -c <command>`) in the MR's rebased worktree.
#
# Subcommands:
#   build  - cargo build + npm ci + npm run build (Rust + TypeScript)
#   test   - cargo test (with pre-existing-failure skips) + npm tests
#   lint   - cargo clippy (workspace, warnings-as-errors) — currently disabled
#            by default because the Rust 1.95 toolchain introduces the
#            `duration_suboptimal_units` lint which fires 30+ times on
#            mainline. Enable after that's fixed (tracked separately).
#
# The test subcommand skips three pre-existing failures:
#   - acp_executor::tests::test_create_terminal_and_output (matches CI skip in
#     .github/workflows/ci.yml via scripts/ci-rust-gate.sh)
#   - loop_registry::tests::test_registry_different_pids_coexist (flaky:
#     hard-codes PID 99999 which collides with real processes on busy hosts;
#     tracked in ro-aiav)
#   - pty_executor::tests::test_run_observe_large_stdin_backend_does_not_deadlock
#     (flaky under parallel load due to a tight timeout; tracked in ro-d8rt)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Unset inherited git env vars so nested `git` calls in tests operate against
# their own repos (mirrors scripts/ci-rust-gate.sh).
while IFS= read -r git_env_var; do
  unset "$git_env_var"
done < <(git rev-parse --local-env-vars 2>/dev/null || true)

# Pre-existing test failures to skip. Keep this list narrow and well-documented.
CARGO_TEST_SKIPS=(
  # Matches CI skip in scripts/ci-rust-gate.sh
  --skip acp_executor::tests::test_create_terminal_and_output
  # Flaky: hard-coded PID 99999 exists on busy hosts (ro-aiav)
  --skip loop_registry::tests::test_registry_different_pids_coexist
  # Flaky: tight timeout under parallel load (ro-d8rt)
  --skip pty_executor::tests::test_run_observe_large_stdin_backend_does_not_deadlock
)

log() {
  printf '\n[refinery-gate] %s\n' "$*"
}

gate_build() {
  log "cargo build --workspace --locked"
  cargo build --workspace --locked

  log "npm ci"
  npm ci

  log "npm run build"
  npm run build
}

gate_test() {
  log "cargo test --workspace --locked (skipping pre-existing failures)"
  cargo test --workspace --locked -- "${CARGO_TEST_SKIPS[@]}"

  # `npm ci` is normally part of the build gate, but the refinery may invoke
  # test in isolation. Install deps if they're missing.
  if [[ ! -d node_modules ]]; then
    log "npm ci (node_modules missing)"
    npm ci
  fi

  log "npm run test:server"
  npm run test:server

  log "npm run test -w @ralph-web/dashboard"
  npm run test -w @ralph-web/dashboard
}

gate_lint() {
  log "cargo clippy --all-targets --all-features -- -D warnings"
  cargo clippy --all-targets --all-features --locked -- -D warnings
}

case "${1:-}" in
  build)
    gate_build
    ;;
  test)
    gate_test
    ;;
  lint)
    gate_lint
    ;;
  all)
    gate_build
    gate_lint
    gate_test
    ;;
  *)
    echo "usage: $0 {build|test|lint|all}" >&2
    echo "" >&2
    echo "Refinery pre-merge gate. See ~/gt/ralph/config.json merge_queue block." >&2
    exit 2
    ;;
esac

log "OK"
