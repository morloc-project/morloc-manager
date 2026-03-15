#!/usr/bin/env bash
# Common test helpers for morloc-manager BATS tests

# Resolve paths relative to the test directory
TEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECT_DIR="$(cd "$TEST_DIR/.." && pwd)"
SCRIPT_PATH="$PROJECT_DIR/morloc-manager.sh"
BATS_LIB="$TEST_DIR/lib"

# Load BATS helper libraries
load "$BATS_LIB/bats-support/load"
load "$BATS_LIB/bats-assert/load"
load "$BATS_LIB/bats-file/load"

# Source morloc-manager.sh in testing mode (does not run main)
source_morloc_manager() {
    export MORLOC_MANAGER_TESTING=1
    # shellcheck disable=SC1090
    source "$SCRIPT_PATH"
}

# Create an isolated HOME directory for testing
# This prevents tests from modifying the real user's HOME
setup_isolated_home() {
    export ORIGINAL_HOME="$HOME"
    export HOME="$(mktemp -d "${BATS_TMPDIR:-/tmp}/morloc-test-home.XXXXXX")"
    mkdir -p "$HOME/.local/bin"
    mkdir -p "$HOME/.local/share/morloc"
    mkdir -p "$HOME/.config/morloc"
    mkdir -p "$HOME/.cache/morloc"
}

# Clean up isolated HOME
teardown_isolated_home() {
    if [ -n "${ORIGINAL_HOME:-}" ]; then
        # Rootless podman may leave root-owned overlay files in container
        # storage under $HOME/.local/share/containers/.  A plain rm -rf
        # cannot remove these; use `podman unshare` to enter the user
        # namespace where they are deletable.
        if ! rm -rf "$HOME" 2>/dev/null; then
            if command -v podman >/dev/null 2>&1; then
                podman unshare rm -rf "$HOME" 2>/dev/null || true
            fi
        fi
        export HOME="$ORIGINAL_HOME"
        unset ORIGINAL_HOME
    fi
}

# Create a minimal shell rc file for testing
create_shell_rc() {
    local shell_name="$1"
    local rc_file
    case "$shell_name" in
        bash)   rc_file="$HOME/.bashrc" ;;
        zsh)    rc_file="$HOME/.zshrc" ;;
        fish)
            mkdir -p "$HOME/.config/fish"
            rc_file="$HOME/.config/fish/config.fish"
            ;;
        ksh)    rc_file="$HOME/.kshrc" ;;
        dash|ash) rc_file="$HOME/.profile" ;;
        tcsh)   rc_file="$HOME/.tcshrc" ;;
        csh)    rc_file="$HOME/.cshrc" ;;
        *)      rc_file="$HOME/.profile" ;;
    esac
    touch "$rc_file"
    echo "$rc_file"
}

# Assert that a file contains a specific string (literal match)
assert_file_contains() {
    local file="$1"
    local pattern="$2"
    if ! grep -qF -- "$pattern" "$file" 2>/dev/null; then
        echo "Expected file '$file' to contain '$pattern'" >&2
        echo "Actual contents:" >&2
        cat "$file" >&2
        return 1
    fi
}

# ---- VM skip guards ----
# These helpers allow VM-specific tests to skip gracefully on the wrong VM.

# Skip unless SELinux is in Enforcing mode
require_selinux_enforcing() {
    if ! command -v getenforce >/dev/null 2>&1; then
        skip "getenforce not available (not an SELinux system)"
    fi
    local mode
    mode="$(getenforce 2>/dev/null)"
    if [ "$mode" != "Enforcing" ]; then
        skip "SELinux is not enforcing (current: $mode)"
    fi
}

# Skip unless AppArmor is active
require_apparmor_active() {
    if ! command -v aa-status >/dev/null 2>&1; then
        skip "aa-status not available (not an AppArmor system)"
    fi
    if ! aa-status >/dev/null 2>&1; then
        skip "AppArmor is not active"
    fi
}

# Skip unless cgroup v1 hierarchy is present
require_cgroup_v1() {
    if [ ! -d /sys/fs/cgroup/cpu ]; then
        skip "cgroup v1 not detected (/sys/fs/cgroup/cpu missing)"
    fi
}

# Skip unless cgroup v2 hierarchy is present
require_cgroup_v2() {
    if [ ! -f /sys/fs/cgroup/cgroup.controllers ]; then
        skip "cgroup v2 not detected (/sys/fs/cgroup/cgroup.controllers missing)"
    fi
}

# Skip always -- placeholder until rootful support is implemented
require_rootful_support() {
    skip "rootful support not yet implemented"
}

# Detect an available container engine for VM tests (real, not mock)
# Sets DETECTED_ENGINE to "docker" or "podman", or skips if neither is found
detect_available_engine() {
    if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
        DETECTED_ENGINE="docker"
    elif command -v podman >/dev/null 2>&1 && podman info >/dev/null 2>&1; then
        DETECTED_ENGINE="podman"
    else
        skip "no container engine available"
    fi
    export DETECTED_ENGINE
}

# Assert that a file does NOT contain a specific string (literal match)
assert_file_not_contains() {
    local file="$1"
    local pattern="$2"
    if grep -qF -- "$pattern" "$file" 2>/dev/null; then
        echo "Expected file '$file' to NOT contain '$pattern'" >&2
        echo "Actual contents:" >&2
        cat "$file" >&2
        return 1
    fi
}
