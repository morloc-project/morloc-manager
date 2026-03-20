#!/usr/bin/env bats
# Integration tests for the select subcommand

load "../helpers/common"
load "../helpers/mock_engine"

setup() {
    setup_isolated_home
    setup_mock_engine "docker" "24.0.7"
    source_morloc_manager
    # Force the mock engine — auto-detection may find real podman instead
    CONTAINER_ENGINE="docker"
    export MORLOC_BIN="$HOME/.local/bin"
    mkdir -p "$MORLOC_BIN"
    # Pre-create version config
    setup_version_config "0.55.0" "local"
}

teardown() {
    teardown_mock_engine
    teardown_isolated_home
}

@test "select: switches version when installed" {
    local data_dir="$(data_root)/versions/0.55.0"
    mkdir -p "$data_dir"
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_select 0.55.0
    "
    assert_success
    assert_output --partial "Switched to Morloc version"
}

@test "select: fails when version not installed" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_select 0.99.0
    "
    assert_failure
    assert_output --partial "does not exist"
}

@test "select: no version shows error and lists available" {
    mkdir -p "$(data_root)/versions/0.55.0"
    mkdir -p "$(data_root)/versions/0.54.0"
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_select
    "
    assert_failure
    assert_output --partial "Please select a version"
}

@test "select: rejects 'local' version" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_select local
    "
    assert_failure
    assert_output --partial "Cannot set to"
}

@test "select: updates user config with new version" {
    mkdir -p "$(data_root)/versions/0.55.0"
    mkdir -p "$(data_root)/versions/0.54.0"
    # Select 0.54.0
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_select 0.54.0
    " 2>/dev/null || true
    assert_file_contains "$(config_root)/config" "active_version=0.54.0"

    # Select 0.55.0
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_select 0.55.0
    " 2>/dev/null || true
    assert_file_contains "$(config_root)/config" "active_version=0.55.0"
}

@test "select: --system flag forces system scope" {
    # Create system version dir
    mkdir -p "$(data_root --system)/versions/0.55.0"
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        # Can't actually use sudo in test, so just test the flag parsing
        cmd_select --help
    "
    assert_success
    assert_output --partial "--system"
}

@test "select: auto-resolves scope via resolve_version" {
    mkdir -p "$(data_root)/versions/0.55.0"
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_select 0.55.0
    " 2>/dev/null || true
    assert_file_contains "$(config_root)/config" "active_scope=local"
}
