#!/usr/bin/env bash
# Detect whether a set of changed files contains anything Rust-relevant.
#
# Usage:
#   scripts/detect-rust-changes.sh <mode> [args...]
#
# Modes:
#   staged
#       Inspect `git diff --cached --name-only` (for pre-commit hooks).
#   pushed <local_sha> <remote_sha>
#       Inspect files changed between remote_sha and local_sha (for pre-push
#       hooks). If remote_sha is the all-zero SHA (new branch), fall back to
#       comparing local_sha against the merge-base with origin/main, or against
#       origin/main if no merge-base exists.
#   range <git-rev-range>
#       Inspect files touched in an arbitrary `git diff --name-only` range.
#   files
#       Read newline-separated file paths from stdin.
#
# Exit codes:
#   0  Rust-relevant changes detected (or detection declined; see below).
#   1  No Rust-relevant changes in the inspected set.
#   2  Misuse / internal error.
#
# Policy on ambiguous cases:
#   - If we cannot determine the change set (missing remote, shallow clone,
#     branch with no merge-base), we err on the side of *running the gate*
#     (exit 0). Skipping the gate on ambiguity would silently bypass CI
#     parity, which is worse than running it unnecessarily.
#
# Rust-relevant file patterns:
#   *.rs                   Rust sources
#   Cargo.toml, Cargo.lock Workspace / crate manifests
#   rust-toolchain*        Toolchain pin
#   rustfmt.toml, .rustfmt.toml
#   clippy.toml, .clippy.toml
#   .cargo/**              Cargo config
#   scripts/ci-rust-gate.sh, scripts/hooks-bdd-gate.sh,
#   scripts/sync-embedded-files.sh
#     The gate scripts themselves: if they change, re-run the gate so
#     the change is exercised on the PR that modifies them.
#   (detect-rust-changes.sh is intentionally NOT in this list — its own
#   changes are validated by scripts/tests/test-detect-rust-changes.sh,
#   not the Rust gate.)

set -euo pipefail

ZERO_SHA="0000000000000000000000000000000000000000"

die() {
    echo "detect-rust-changes: $*" >&2
    exit 2
}

is_rust_relevant() {
    local path="$1"

    case "$path" in
        *.rs) return 0 ;;
        Cargo.toml | Cargo.lock) return 0 ;;
        */Cargo.toml | */Cargo.lock) return 0 ;;
        rust-toolchain | rust-toolchain.toml) return 0 ;;
        rustfmt.toml | .rustfmt.toml) return 0 ;;
        clippy.toml | .clippy.toml) return 0 ;;
        .cargo/* | */.cargo/*) return 0 ;;
        scripts/ci-rust-gate.sh) return 0 ;;
        scripts/hooks-bdd-gate.sh) return 0 ;;
        scripts/sync-embedded-files.sh) return 0 ;;
        *) return 1 ;;
    esac
}

any_rust_relevant() {
    local path
    while IFS= read -r path; do
        [[ -z "$path" ]] && continue
        if is_rust_relevant "$path"; then
            return 0
        fi
    done
    return 1
}

resolve_pushed_range() {
    local local_sha="$1"
    local remote_sha="$2"

    if [[ -z "$local_sha" || "$local_sha" == "$ZERO_SHA" ]]; then
        # Branch deletion: nothing to check.
        echo ""
        return 0
    fi

    if [[ "$remote_sha" != "$ZERO_SHA" ]]; then
        echo "${remote_sha}..${local_sha}"
        return 0
    fi

    # New branch on remote: fall back to merge-base with origin/main.
    local base=""
    if git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null 2>&1; then
        base="$(git merge-base refs/remotes/origin/main "$local_sha" 2>/dev/null || true)"
        if [[ -z "$base" ]]; then
            base="refs/remotes/origin/main"
        fi
    fi

    if [[ -z "$base" ]]; then
        # Cannot determine a base (new repo, no origin/main). Signal unknown.
        echo "UNKNOWN"
        return 0
    fi

    echo "${base}..${local_sha}"
}

mode="${1:-}"
if [[ -z "$mode" ]]; then
    die "missing mode (staged|pushed|range|files)"
fi
shift

case "$mode" in
    staged)
        files="$(git diff --cached --name-only --diff-filter=ACMRTUB 2>/dev/null || true)"
        if [[ -z "$files" ]]; then
            exit 1
        fi
        if printf '%s\n' "$files" | any_rust_relevant; then
            exit 0
        fi
        exit 1
        ;;
    pushed)
        if [[ "$#" -lt 2 ]]; then
            die "pushed mode requires <local_sha> <remote_sha>"
        fi
        local_sha="$1"
        remote_sha="$2"
        range="$(resolve_pushed_range "$local_sha" "$remote_sha")"
        if [[ -z "$range" ]]; then
            # Nothing to inspect (e.g. branch deletion).
            exit 1
        fi
        if [[ "$range" == "UNKNOWN" ]]; then
            # Be conservative: run the gate when we cannot determine the range.
            exit 0
        fi
        files="$(git diff --name-only "$range" 2>/dev/null || true)"
        if [[ -z "$files" ]]; then
            exit 1
        fi
        if printf '%s\n' "$files" | any_rust_relevant; then
            exit 0
        fi
        exit 1
        ;;
    range)
        if [[ "$#" -lt 1 ]]; then
            die "range mode requires <git-rev-range>"
        fi
        range="$1"
        files="$(git diff --name-only "$range" 2>/dev/null || true)"
        if [[ -z "$files" ]]; then
            exit 1
        fi
        if printf '%s\n' "$files" | any_rust_relevant; then
            exit 0
        fi
        exit 1
        ;;
    files)
        if any_rust_relevant; then
            exit 0
        fi
        exit 1
        ;;
    *)
        die "unknown mode: $mode"
        ;;
esac
