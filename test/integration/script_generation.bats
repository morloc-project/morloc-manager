#!/usr/bin/env bats
# Integration tests for file generation (direct docker run, no compose)

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

# --- .env content ---

@test "env_file: generate_env_file creates .env" {
    generate_env_file "0.58.3"
    assert_file_exists "$MORLOC_DATA_HOME/.env"
}

@test "env_file: .env has correct version" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_VERSION=0.58.3"
}

@test "env_file: .env has correct image" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_IMAGE=ghcr.io/morloc-project/morloc/morloc-full:0.58.3"
}

@test "env_file: .env has dev image" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_DEV_IMAGE=ghcr.io/morloc-project/morloc/morloc-test:latest"
}

@test "env_file: .env has host version dir" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_HOST_VERSION_DIR=$MORLOC_HOST_VERSION_DIR"
}

@test "env_file: .env has container home" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_CONTAINER_HOME=$MORLOC_CONTAINER_HOME"
}

@test "env_file: .env has rootful field" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_ROOTFUL="
}

@test "env_file: .env has container engine" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_CONTAINER_ENGINE=docker"
}

@test "env_file: .env has MORLOC_ENV_FLAGS field" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_ENV_FLAGS="
}

# --- menv script ---

@test "menv: generate_menv_script creates menv" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_exists "$MORLOC_BIN/menv"
}

@test "menv: has bash shebang" {
    generate_menv_script "$MORLOC_BIN/menv"
    local first_line
    first_line=$(head -n1 "$MORLOC_BIN/menv")
    [ "$first_line" = "#!/usr/bin/env bash" ]
}

@test "menv: is executable" {
    generate_menv_script "$MORLOC_BIN/menv"
    [ -x "$MORLOC_BIN/menv" ]
}

@test "menv: has --dev flag handler" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "--dev"
}

@test "menv: has --shell flag handler" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "--shell"
}

@test "menv: passes through arguments" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" '"$@"'
}

@test "menv: uses docker run directly (no compose)" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "run --rm"
    assert_file_not_contains "$MORLOC_BIN/menv" "compose"
    assert_file_not_contains "$MORLOC_BIN/menv" "docker-compose"
}

@test "menv: has --rm flag" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "--rm"
}

@test "menv: has --shm-size flag" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "--shm-size"
}

@test "menv: sources .env file" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" ".env"
}

@test "menv: sets MORLOC_WORK_DIR to PWD" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" 'MORLOC_WORK_DIR="$PWD"'
}

@test "menv: shell mode runs /bin/bash" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "/bin/bash"
}

@test "menv: creates dev directories when --dev" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "mkdir -p"
}

@test "menv: reads global morloc.flags" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "morloc.flags"
}

@test "menv: reads MORLOC_ENV_FLAGS" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "MORLOC_ENV_FLAGS"
}

@test "menv: has read_flags function" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "read_flags"
}

@test "menv: has -h/--help handler" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "show_menv_help"
    assert_file_contains "$MORLOC_BIN/menv" -- "--help"
}

@test "menv: has -v/--version handler" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "show_menv_version"
    assert_file_contains "$MORLOC_BIN/menv" -- "--version"
}

@test "menv: help text includes usage examples" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "morloc make"
    assert_file_contains "$MORLOC_BIN/menv" "stack build"
}

@test "menv: embeds morloc-manager version" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "MENV_MANAGER_VERSION="
}

@test "menv: embeds generation date" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "MENV_GENERATED="
}

@test "menv: has SELinux suffix when detected" {
    SELINUX_SUFFIX=":z"
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" ':z'
}

@test "menv: no SELinux suffix when not detected" {
    SELINUX_SUFFIX=""
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_not_contains "$MORLOC_BIN/menv" ':z'
}
