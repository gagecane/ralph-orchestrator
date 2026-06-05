#!/usr/bin/env bash
# Setup git hooks for development.
#
# Instead of copying scripts into .git/hooks/ (which must be re-run whenever
# hook contents change and drifts silently between clones), we point git at
# the in-tree `.githooks/` directory via `core.hooksPath`. This makes the
# hooks version-controlled, auto-updating on `git pull`, and consistent with
# the gas-town "hooks-path-all-rigs" doctor check.
#
# Run this once after cloning the repository. Re-running is idempotent.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOKS_DIR="$REPO_ROOT/.githooks"

cd "$REPO_ROOT"

if [ ! -d "$HOOKS_DIR" ]; then
    echo "❌ Expected hooks directory not found: $HOOKS_DIR" >&2
    exit 1
fi

# Ensure all hooks in .githooks/ are executable. Git silently ignores
# non-executable hooks, which produces confusing "why didn't my hook run?"
# bug reports. chmod is idempotent and cheap.
find "$HOOKS_DIR" -maxdepth 1 -type f -exec chmod +x {} \;

# Point git at the in-tree hooks directory. Use a repo-relative path so this
# works across worktrees and on any machine. `git config` is idempotent — it
# overwrites the existing value with the same string on re-run.
current="$(git config --local --get core.hooksPath 2>/dev/null || true)"
if [ "$current" != ".githooks" ]; then
    git config --local core.hooksPath .githooks
    echo "✅ Set core.hooksPath=.githooks"
else
    echo "✓ core.hooksPath already set to .githooks"
fi

echo ""
echo "🎉 Git hooks configured successfully!"
echo ""
echo "Active hooks in .githooks/:"
for hook in "$HOOKS_DIR"/*; do
    [ -f "$hook" ] || continue
    name="$(basename "$hook")"
    # Skip README / test files / anything that isn't a recognised git hook name.
    case "$name" in
        applypatch-msg|pre-applypatch|post-applypatch|pre-commit|pre-merge-commit| \
        prepare-commit-msg|commit-msg|post-commit|pre-rebase|post-checkout|post-merge| \
        pre-push|pre-receive|update|post-receive|post-update|push-to-checkout| \
        pre-auto-gc|post-rewrite|sendemail-validate|fsmonitor-watchman|p4-*)
            echo "  • $name"
            ;;
    esac
done

echo ""
echo "Skip any hook with --no-verify when needed (e.g. git commit --no-verify)."
