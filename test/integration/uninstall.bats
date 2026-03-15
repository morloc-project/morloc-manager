#!/usr/bin/env bats
# Integration tests for the uninstall subcommand

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
}

teardown() {
    teardown_mock_engine
    teardown_isolated_home
}

@test "uninstall: --all removes version directory" {
    local install_dir="$HOME/${MORLOC_INSTALL_DIR}"
    mkdir -p "$install_dir/0.55.0"
    # cmd_uninstall --all calls exit, so test in subshell
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        cmd_uninstall --all
    "
    assert_success
    assert_dir_not_exists "$install_dir"
}

@test "uninstall: specific version removes only that version" {
    local install_dir="$HOME/${MORLOC_INSTALL_DIR}"
    mkdir -p "$install_dir/0.55.0"
    mkdir -p "$install_dir/0.54.0"
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_uninstall 0.55.0
    "
    assert_success
    assert_dir_not_exists "$install_dir/0.55.0"
    assert_dir_exists "$install_dir/0.54.0"
}

@test "uninstall: graceful when nothing installed" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_uninstall 0.99.0
    "
    assert_success
    assert_output --partial "does not exist"
}

@test "uninstall: no version given shows error" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_uninstall
    "
    assert_failure
    assert_output --partial "No version given"
}

@test "uninstall: --help shows usage" {
    run cmd_uninstall --help
    assert_success
    assert_output --partial "USAGE"
}
