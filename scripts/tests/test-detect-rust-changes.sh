#!/usr/bin/env bash
# Tests for scripts/detect-rust-changes.sh
#
# Exercises each mode (staged, pushed, range, files) against a disposable
# git repository. No live network, no cargo, no outside state.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HELPER="$REPO_ROOT/scripts/detect-rust-changes.sh"

if [[ ! -x "$HELPER" ]]; then
    echo "FAIL: $HELPER is missing or not executable" >&2
    exit 1
fi

TMPDIR_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_ROOT"' EXIT

pass_count=0
fail_count=0

record_pass() {
    pass_count=$((pass_count + 1))
    echo "  ok: $1"
}

record_fail() {
    fail_count=$((fail_count + 1))
    echo "  FAIL: $1" >&2
}

setup_repo() {
    local dir="$1"
    mkdir -p "$dir"
    (
        cd "$dir"
        git init -q -b main
        git config user.email test@example.com
        git config user.name Test
        git config commit.gpgsign false
        echo "init" > README.md
        git add README.md
        git commit -q -m "init"
    )
}

in_repo() {
    local dir="$1"
    shift
    (cd "$dir" && "$@")
}

# ---------------------------------------------------------------------------
# staged mode
# ---------------------------------------------------------------------------

test_staged_rust() {
    local name="staged: rust file staged -> 0"
    local dir="$TMPDIR_ROOT/staged_rust"
    setup_repo "$dir"
    echo "fn main() {}" > "$dir/main.rs"
    in_repo "$dir" git add main.rs
    if in_repo "$dir" "$HELPER" staged; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

test_staged_cargo_toml() {
    local name="staged: Cargo.toml staged -> 0"
    local dir="$TMPDIR_ROOT/staged_cargo"
    setup_repo "$dir"
    echo "[package]" > "$dir/Cargo.toml"
    in_repo "$dir" git add Cargo.toml
    if in_repo "$dir" "$HELPER" staged; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

test_staged_non_rust() {
    local name="staged: only markdown staged -> 1"
    local dir="$TMPDIR_ROOT/staged_md"
    setup_repo "$dir"
    echo "hello" > "$dir/notes.md"
    in_repo "$dir" git add notes.md
    local rc=0
    in_repo "$dir" "$HELPER" staged || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

test_staged_empty() {
    local name="staged: nothing staged -> 1"
    local dir="$TMPDIR_ROOT/staged_empty"
    setup_repo "$dir"
    local rc=0
    in_repo "$dir" "$HELPER" staged || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

test_staged_gate_script() {
    local name="staged: scripts/ci-rust-gate.sh staged -> 0"
    local dir="$TMPDIR_ROOT/staged_gate"
    setup_repo "$dir"
    mkdir -p "$dir/scripts"
    echo "#!/bin/sh" > "$dir/scripts/ci-rust-gate.sh"
    in_repo "$dir" git add scripts/ci-rust-gate.sh
    if in_repo "$dir" "$HELPER" staged; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

test_staged_detect_script_alone() {
    local name="staged: only scripts/detect-rust-changes.sh -> 1"
    local dir="$TMPDIR_ROOT/staged_detect"
    setup_repo "$dir"
    mkdir -p "$dir/scripts"
    echo "#!/bin/sh" > "$dir/scripts/detect-rust-changes.sh"
    in_repo "$dir" git add scripts/detect-rust-changes.sh
    local rc=0
    in_repo "$dir" "$HELPER" staged || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

test_staged_nested_rust() {
    local name="staged: nested crate src/lib.rs -> 0"
    local dir="$TMPDIR_ROOT/staged_nested"
    setup_repo "$dir"
    mkdir -p "$dir/crates/foo/src"
    echo "pub fn x() {}" > "$dir/crates/foo/src/lib.rs"
    in_repo "$dir" git add crates/foo/src/lib.rs
    if in_repo "$dir" "$HELPER" staged; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

test_staged_nested_cargo() {
    local name="staged: nested crates/foo/Cargo.toml -> 0"
    local dir="$TMPDIR_ROOT/staged_nested_cargo"
    setup_repo "$dir"
    mkdir -p "$dir/crates/foo"
    echo "[package]" > "$dir/crates/foo/Cargo.toml"
    in_repo "$dir" git add crates/foo/Cargo.toml
    if in_repo "$dir" "$HELPER" staged; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

test_staged_shell_script() {
    local name="staged: only shell script (non-gate) staged -> 1"
    local dir="$TMPDIR_ROOT/staged_shell"
    setup_repo "$dir"
    mkdir -p "$dir/scripts"
    echo "#!/bin/sh" > "$dir/scripts/setup.sh"
    in_repo "$dir" git add scripts/setup.sh
    local rc=0
    in_repo "$dir" "$HELPER" staged || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

# ---------------------------------------------------------------------------
# pushed mode
# ---------------------------------------------------------------------------

test_pushed_rust_diff() {
    local name="pushed: commit with .rs between shas -> 0"
    local dir="$TMPDIR_ROOT/pushed_rust"
    setup_repo "$dir"
    local base_sha
    base_sha="$(in_repo "$dir" git rev-parse HEAD)"
    echo "fn main() {}" > "$dir/main.rs"
    in_repo "$dir" git add main.rs
    in_repo "$dir" git commit -q -m "add main.rs"
    local head_sha
    head_sha="$(in_repo "$dir" git rev-parse HEAD)"
    if in_repo "$dir" "$HELPER" pushed "$head_sha" "$base_sha"; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

test_pushed_non_rust_diff() {
    local name="pushed: commit with only docs -> 1"
    local dir="$TMPDIR_ROOT/pushed_docs"
    setup_repo "$dir"
    local base_sha
    base_sha="$(in_repo "$dir" git rev-parse HEAD)"
    echo "note" > "$dir/CHANGELOG.md"
    in_repo "$dir" git add CHANGELOG.md
    in_repo "$dir" git commit -q -m "docs"
    local head_sha
    head_sha="$(in_repo "$dir" git rev-parse HEAD)"
    local rc=0
    in_repo "$dir" "$HELPER" pushed "$head_sha" "$base_sha" || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

test_pushed_delete_branch() {
    local name="pushed: branch delete (local sha zero) -> 1"
    local dir="$TMPDIR_ROOT/pushed_delete"
    setup_repo "$dir"
    local remote_sha
    remote_sha="$(in_repo "$dir" git rev-parse HEAD)"
    local rc=0
    in_repo "$dir" "$HELPER" pushed "0000000000000000000000000000000000000000" "$remote_sha" || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

test_pushed_new_branch_no_origin_main() {
    # Helper has no origin/main to fall back to -> returns UNKNOWN -> exit 0.
    local name="pushed: new branch with no origin/main -> 0 (conservative)"
    local dir="$TMPDIR_ROOT/pushed_new_noorigin"
    setup_repo "$dir"
    local head_sha
    head_sha="$(in_repo "$dir" git rev-parse HEAD)"
    if in_repo "$dir" "$HELPER" pushed "$head_sha" "0000000000000000000000000000000000000000"; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

test_pushed_new_branch_with_origin_main() {
    # With origin/main present and branch has non-rust commit on top,
    # helper should compute merge-base and report 1.
    local name="pushed: new branch (origin/main present, only docs) -> 1"
    local dir="$TMPDIR_ROOT/pushed_new_origin"
    setup_repo "$dir"
    # Simulate origin/main by creating a remote-tracking ref at current HEAD.
    local base_sha
    base_sha="$(in_repo "$dir" git rev-parse HEAD)"
    in_repo "$dir" git update-ref refs/remotes/origin/main "$base_sha"
    in_repo "$dir" git checkout -q -b feature
    echo "note" > "$dir/CHANGELOG.md"
    in_repo "$dir" git add CHANGELOG.md
    in_repo "$dir" git commit -q -m "docs"
    local head_sha
    head_sha="$(in_repo "$dir" git rev-parse HEAD)"
    local rc=0
    in_repo "$dir" "$HELPER" pushed "$head_sha" "0000000000000000000000000000000000000000" || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

test_pushed_new_branch_with_origin_main_rust() {
    local name="pushed: new branch (origin/main present, has .rs) -> 0"
    local dir="$TMPDIR_ROOT/pushed_new_origin_rust"
    setup_repo "$dir"
    local base_sha
    base_sha="$(in_repo "$dir" git rev-parse HEAD)"
    in_repo "$dir" git update-ref refs/remotes/origin/main "$base_sha"
    in_repo "$dir" git checkout -q -b feature
    echo "fn main() {}" > "$dir/main.rs"
    in_repo "$dir" git add main.rs
    in_repo "$dir" git commit -q -m "add main.rs"
    local head_sha
    head_sha="$(in_repo "$dir" git rev-parse HEAD)"
    if in_repo "$dir" "$HELPER" pushed "$head_sha" "0000000000000000000000000000000000000000"; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

# ---------------------------------------------------------------------------
# range mode
# ---------------------------------------------------------------------------

test_range_rust() {
    local name="range: explicit range with .rs -> 0"
    local dir="$TMPDIR_ROOT/range_rust"
    setup_repo "$dir"
    local base_sha
    base_sha="$(in_repo "$dir" git rev-parse HEAD)"
    echo "fn main() {}" > "$dir/main.rs"
    in_repo "$dir" git add main.rs
    in_repo "$dir" git commit -q -m "add main.rs"
    if in_repo "$dir" "$HELPER" range "${base_sha}..HEAD"; then
        record_pass "$name"
    else
        record_fail "$name"
    fi
}

test_range_non_rust() {
    local name="range: explicit range with only docs -> 1"
    local dir="$TMPDIR_ROOT/range_non_rust"
    setup_repo "$dir"
    local base_sha
    base_sha="$(in_repo "$dir" git rev-parse HEAD)"
    echo "note" > "$dir/CHANGELOG.md"
    in_repo "$dir" git add CHANGELOG.md
    in_repo "$dir" git commit -q -m "docs"
    local rc=0
    in_repo "$dir" "$HELPER" range "${base_sha}..HEAD" || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

# ---------------------------------------------------------------------------
# files mode (stdin)
# ---------------------------------------------------------------------------

test_files_mixed() {
    local name="files: mixed list with .rs -> 0"
    local rc=0
    printf 'docs/x.md\nsrc/main.rs\n' | "$HELPER" files || rc=$?
    if [[ "$rc" -eq 0 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 0, got $rc)"
    fi
}

test_files_non_rust() {
    local name="files: only non-rust paths -> 1"
    local rc=0
    printf 'docs/x.md\nAGENTS.md\nscripts/setup-hooks.sh\n' | "$HELPER" files || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

test_files_empty() {
    local name="files: empty stdin -> 1"
    local rc=0
    : | "$HELPER" files || rc=$?
    if [[ "$rc" -eq 1 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 1, got $rc)"
    fi
}

test_files_cargo_lock() {
    local name="files: Cargo.lock -> 0"
    local rc=0
    printf 'Cargo.lock\n' | "$HELPER" files || rc=$?
    if [[ "$rc" -eq 0 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 0, got $rc)"
    fi
}

test_files_toolchain() {
    local name="files: rust-toolchain.toml -> 0"
    local rc=0
    printf 'rust-toolchain.toml\n' | "$HELPER" files || rc=$?
    if [[ "$rc" -eq 0 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 0, got $rc)"
    fi
}

# ---------------------------------------------------------------------------
# misuse
# ---------------------------------------------------------------------------

test_misuse_no_args() {
    local name="misuse: no mode -> 2"
    local rc=0
    "$HELPER" 2>/dev/null || rc=$?
    if [[ "$rc" -eq 2 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 2, got $rc)"
    fi
}

test_misuse_unknown_mode() {
    local name="misuse: unknown mode -> 2"
    local rc=0
    "$HELPER" bogus 2>/dev/null || rc=$?
    if [[ "$rc" -eq 2 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 2, got $rc)"
    fi
}

test_misuse_pushed_missing_args() {
    local name="misuse: pushed without shas -> 2"
    local rc=0
    "$HELPER" pushed 2>/dev/null || rc=$?
    if [[ "$rc" -eq 2 ]]; then
        record_pass "$name"
    else
        record_fail "$name (expected 2, got $rc)"
    fi
}

echo "Running detect-rust-changes tests..."

test_staged_rust
test_staged_cargo_toml
test_staged_non_rust
test_staged_empty
test_staged_gate_script
test_staged_detect_script_alone
test_staged_nested_rust
test_staged_nested_cargo
test_staged_shell_script

test_pushed_rust_diff
test_pushed_non_rust_diff
test_pushed_delete_branch
test_pushed_new_branch_no_origin_main
test_pushed_new_branch_with_origin_main
test_pushed_new_branch_with_origin_main_rust

test_range_rust
test_range_non_rust

test_files_mixed
test_files_non_rust
test_files_empty
test_files_cargo_lock
test_files_toolchain

test_misuse_no_args
test_misuse_unknown_mode
test_misuse_pushed_missing_args

echo
echo "Passed: $pass_count"
echo "Failed: $fail_count"

if [[ "$fail_count" -ne 0 ]]; then
    exit 1
fi
