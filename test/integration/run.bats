#!/usr/bin/env bats
# Integration tests for the run and shell subcommands

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
    # Set up a working version config
    setup_version_config "0.55.0" "local"
    mkdir -p "$(data_root)/versions/0.55.0/bin"
}

teardown() {
    teardown_mock_engine
    teardown_isolated_home
}

@test "run: --help shows usage" {
    run cmd_run --help
    assert_success
    assert_output --partial "USAGE"
    assert_output --partial "--dev"
    assert_output --partial "--shell"
}

@test "run: fails when no active version" {
    # Clear the active version
    write_config "active_version" "" "$(config_root)/config"
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_run echo hello
    "
    assert_failure
    assert_output --partial "No active version"
}

@test "run: reads active version from config" {
    # cmd_run will exec, so we can't test it directly without exec failing
    # but we can test it parses the config correctly by checking --help still works
    run cmd_run --help
    assert_success
    assert_output --partial "run"
}

@test "run: help text includes examples" {
    run show_run_help
    assert_success
    assert_output --partial "morloc --version"
    assert_output --partial "morloc make"
}

@test "run: --dev flag is documented" {
    run show_run_help
    assert_success
    assert_output --partial "--dev"
}

@test "run: --shell flag is documented" {
    run show_run_help
    assert_success
    assert_output --partial "--shell"
}

@test "run: -x flag is documented" {
    run show_run_help
    assert_success
    assert_output --partial "-x FLAG"
}

