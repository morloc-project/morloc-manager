#!/usr/bin/env bats
# Tests for container engine detection and set_container_engine

load "../helpers/common"
load "../helpers/mock_engine"

setup() {
    setup_isolated_home
    # Save original PATH and CONTAINER_ENGINE
    SAVED_PATH="$PATH"
    SAVED_CONTAINER_ENGINE="${CONTAINER_ENGINE:-}"
    SAVED_MORLOC_CONTAINER_ENGINE="${MORLOC_CONTAINER_ENGINE:-}"
    unset MORLOC_CONTAINER_ENGINE
}

teardown() {
    # Restore PATH first so that rm, sed, etc. are available for cleanup
    export PATH="$SAVED_PATH"
    teardown_isolated_home
    teardown_mock_engine
    if [ -n "${SHADOW_PATH_DIR:-}" ]; then
        rm -rf "$SHADOW_PATH_DIR"
        unset SHADOW_PATH_DIR
    fi
    export CONTAINER_ENGINE="${SAVED_CONTAINER_ENGINE}"
    if [ -n "$SAVED_MORLOC_CONTAINER_ENGINE" ]; then
        export MORLOC_CONTAINER_ENGINE="$SAVED_MORLOC_CONTAINER_ENGINE"
    else
        unset MORLOC_CONTAINER_ENGINE
    fi
}

@test "set_container_engine: sets docker when available" {
    setup_mock_engine "docker" "24.0.7"
    source_morloc_manager
    run set_container_engine "docker"
    assert_success
}

@test "set_container_engine: sets podman when available" {
    setup_mock_engine "podman" "4.7.2"
    source_morloc_manager
    run set_container_engine "podman"
    assert_success
}

@test "set_container_engine: fails when engine not found" {
    source_morloc_manager
    run set_container_engine "nonexistent-engine"
    assert_failure
    assert_output --partial "not found"
}

@test "container engine: auto-detects mock docker when only docker in PATH" {
    setup_mock_engine "docker" "24.0.7"
    # We need podman to not be found by `command -v` so docker wins auto-detection.
    # We can't just drop directories containing podman from PATH because they
    # also contain essential tools (sed, rm, etc.).  Instead, for any directory
    # that has podman, create a shadow copy with symlinks to everything EXCEPT
    # podman.
    SHADOW_PATH_DIR="$(mktemp -d "${BATS_TMPDIR:-/tmp}/shadow-path.XXXXXX")"
    local new_path="$MOCK_ENGINE_DIR"
    local IFS=':'
    for dir in $SAVED_PATH; do
        if [ -x "$dir/podman" ]; then
            local shadow="$SHADOW_PATH_DIR/$(echo "$dir" | tr '/' '_')"
            mkdir -p "$shadow"
            for f in "$dir"/*; do
                [ -f "$f" ] && [ -x "$f" ] || continue
                [ "${f##*/}" = "podman" ] && continue
                ln -sf "$f" "$shadow/${f##*/}"
            done
            new_path="${new_path}:${shadow}"
        else
            new_path="${new_path}:${dir}"
        fi
    done
    export PATH="$new_path"
    if command -v podman >/dev/null 2>&1; then
        skip "podman is still reachable, cannot isolate PATH"
    fi
    source_morloc_manager
    [ "$CONTAINER_ENGINE" = "docker" ]
}

@test "container engine: MORLOC_CONTAINER_ENGINE overrides auto-detection" {
    setup_mock_engine "docker" "24.0.7"
    export MORLOC_CONTAINER_ENGINE="docker"
    source_morloc_manager
    [ "$CONTAINER_ENGINE" = "docker" ]
}
