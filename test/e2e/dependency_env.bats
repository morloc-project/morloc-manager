#!/usr/bin/env bats
# End-to-end tests: custom dependency environments
# Requires a real container engine with morloc images available.

load "../helpers/common"

FIXTURES_DIR="$TEST_DIR/fixtures"

setup() {
    if [ -n "${MORLOC_CONTAINER_ENGINE:-}" ]; then
        ENGINE="$MORLOC_CONTAINER_ENGINE"
    elif command -v docker >/dev/null 2>&1; then
        ENGINE="docker"
    elif command -v podman >/dev/null 2>&1; then
        ENGINE="podman"
    else
        skip "No container engine available"
    fi

    setup_isolated_home
    export MORLOC_CONTAINER_ENGINE="$ENGINE"
    export MORLOC_DEPENDENCY_DIR="$HOME/.local/share/morloc/deps"
}

teardown() {
    teardown_isolated_home
}

@test "e2e: env --init creates correct Dockerfile structure" {
    run bash "$SCRIPT_PATH" env --init testenv
    assert_success

    local envfile="$MORLOC_DEPENDENCY_DIR/testenv.Dockerfile"
    assert_file_exists "$envfile"
    assert_file_contains "$envfile" "ARG CONTAINER_BASE"
    assert_file_contains "$envfile" 'FROM ${CONTAINER_BASE}'
    assert_file_contains "$envfile" "testenv"
}

@test "e2e: env --list shows created environment" {
    # First create an env
    bash "$SCRIPT_PATH" env --init myenv 2>/dev/null

    run bash "$SCRIPT_PATH" env --list
    assert_success
    assert_output --partial "myenv"
}

@test "e2e: env --init refuses to overwrite existing" {
    bash "$SCRIPT_PATH" env --init dupenv 2>/dev/null

    run bash "$SCRIPT_PATH" env --init dupenv
    assert_failure
    assert_output --partial "already exists"
}
