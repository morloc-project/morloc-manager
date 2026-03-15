#!/usr/bin/env bats
# Integration tests for the install subcommand
# These use mock container engines - no real Docker/Podman needed

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

@test "install: creates version directory structure" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_exists "$MORLOC_BIN/menv"
}

@test "install: menv script is executable" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    [ -x "$MORLOC_BIN/menv" ]
}

@test "install: menv script contains docker run" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" "docker run"
}

@test "install: menv script contains --shm-size" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" "shm-size=4g"
}

@test "install: menv script contains correct version in mount path" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" "0.55.0"
}

@test "install: morloc-shell script is created and executable" {
    script_morloc_shell "$MORLOC_BIN/morloc-shell" "0.55.0"
    assert_file_exists "$MORLOC_BIN/morloc-shell"
    [ -x "$MORLOC_BIN/morloc-shell" ]
}

@test "install: morloc-shell script has -it flag for interactive" {
    script_morloc_shell "$MORLOC_BIN/morloc-shell" "0.55.0"
    assert_file_contains "$MORLOC_BIN/morloc-shell" "rm -it"
}

@test "install: morloc-shell script runs /bin/bash" {
    script_morloc_shell "$MORLOC_BIN/morloc-shell" "0.55.0"
    assert_file_contains "$MORLOC_BIN/morloc-shell" "/bin/bash"
}

@test "install: menv-dev script is created" {
    script_menv_dev "$MORLOC_BIN/menv-dev"
    assert_file_exists "$MORLOC_BIN/menv-dev"
    [ -x "$MORLOC_BIN/menv-dev" ]
}

@test "install: morloc-shell-dev script is created" {
    script_morloc_dev_shell "$MORLOC_BIN/morloc-shell-dev"
    assert_file_exists "$MORLOC_BIN/morloc-shell-dev"
    [ -x "$MORLOC_BIN/morloc-shell-dev" ]
}

@test "install: --no-init flag is parsed correctly" {
    run show_install_help
    assert_success
    assert_output --partial "--no-init"
}

@test "install: create_directory makes new directory" {
    local test_dir="$HOME/test-new-dir"
    run create_directory "$test_dir"
    assert_success
    assert_dir_exists "$test_dir"
}

@test "install: create_directory handles existing directory" {
    local test_dir="$HOME/test-existing-dir"
    mkdir -p "$test_dir"
    run create_directory "$test_dir"
    assert_success
    assert_output --partial "already exists"
}

@test "install: version data directory structure is correct" {
    local version="0.55.0"
    local morloc_data_home="$HOME/${MORLOC_INSTALL_DIR}/$version"
    create_directory "$morloc_data_home"
    create_directory "$morloc_data_home/include"
    create_directory "$morloc_data_home/lib"
    create_directory "$morloc_data_home/opt"
    create_directory "$morloc_data_home/src/morloc/plane"
    create_directory "$morloc_data_home/tmp"

    assert_dir_exists "$morloc_data_home"
    assert_dir_exists "$morloc_data_home/include"
    assert_dir_exists "$morloc_data_home/lib"
    assert_dir_exists "$morloc_data_home/opt"
    assert_dir_exists "$morloc_data_home/src/morloc/plane"
    assert_dir_exists "$morloc_data_home/tmp"
}

@test "install: script uses podman when engine is podman" {
    teardown_mock_engine
    setup_mock_engine "podman" "4.7.2"
    CONTAINER_ENGINE="podman"
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" "podman run"
}
