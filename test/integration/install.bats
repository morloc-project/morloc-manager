#!/usr/bin/env bats
# Integration tests for the install subcommand (compose-based)
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

@test "install: generate_compose_file creates compose file" {
    generate_compose_file
    assert_file_exists "$MORLOC_DATA_HOME/docker-compose.yml"
}

@test "install: generate_env_file creates env file with version" {
    generate_env_file "0.55.0"
    assert_file_exists "$MORLOC_DATA_HOME/.env"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_VERSION=0.55.0"
}

@test "install: generate_menv_script creates executable menv" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_exists "$MORLOC_BIN/menv"
    [ -x "$MORLOC_BIN/menv" ]
}

@test "install: menv script uses compose run" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "compose"
    assert_file_contains "$MORLOC_BIN/menv" "run"
}

@test "install: menv script has --rm flag" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "--rm"
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

@test "install: env file records podman when engine is podman" {
    teardown_mock_engine
    setup_mock_engine "podman" "4.7.2"
    CONTAINER_ENGINE="podman"
    generate_env_file "0.55.0"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_CONTAINER_ENGINE=podman"
}

@test "install: detect_compose_command succeeds with mock docker" {
    run detect_compose_command
    assert_success
}
