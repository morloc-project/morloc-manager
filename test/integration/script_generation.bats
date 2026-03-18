#!/usr/bin/env bats
# Integration tests for compose-based file generation

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

# --- docker-compose.yml content ---

@test "compose: generate_compose_file creates docker-compose.yml" {
    generate_compose_file
    assert_file_exists "$MORLOC_DATA_HOME/docker-compose.yml"
}

@test "compose: docker-compose.yml has DO NOT EDIT header" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" "DO NOT EDIT"
}

@test "compose: docker-compose.yml has morloc service" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" "morloc:"
}

@test "compose: docker-compose.yml has morloc-dev service" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" "morloc-dev:"
}

@test "compose: docker-compose.yml references MORLOC_IMAGE variable" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" 'MORLOC_IMAGE'
}

@test "compose: docker-compose.yml references MORLOC_DEV_IMAGE variable" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" 'MORLOC_DEV_IMAGE'
}

@test "compose: docker-compose.yml has shm_size" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" "shm_size"
}

@test "compose: docker-compose.yml has MORLOC_WORK_DIR volume" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" 'MORLOC_WORK_DIR'
}

@test "compose: docker-compose.yml has work directory" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" "work"
}

@test "compose: docker-compose.yml dev has ghcup in PATH" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" "ghcup"
}

@test "compose: docker-compose.yml dev has .stack volume" {
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" ".stack"
}

@test "compose: docker-compose.yml has :z when SELinux detected" {
    SELINUX_SUFFIX=":z"
    generate_compose_file
    assert_file_contains "$MORLOC_DATA_HOME/docker-compose.yml" ":z"
}

@test "compose: docker-compose.yml lacks :z when no SELinux" {
    SELINUX_SUFFIX=""
    generate_compose_file
    assert_file_not_contains "$MORLOC_DATA_HOME/docker-compose.yml" ":z"
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

@test "env_file: .env has host home" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_HOST_HOME=$HOME"
}

@test "env_file: .env has install dir" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_INSTALL_DIR="
}

@test "env_file: .env has container engine" {
    generate_env_file "0.58.3"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_CONTAINER_ENGINE=docker"
}

# --- menv script ---

@test "menv: generate_menv_script creates menv" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_exists "$MORLOC_BIN/menv"
}

@test "menv: has correct shebang" {
    generate_menv_script "$MORLOC_BIN/menv"
    local first_line
    first_line=$(head -n1 "$MORLOC_BIN/menv")
    [ "$first_line" = "#!/usr/bin/env sh" ]
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

@test "menv: uses compose run" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "compose"
    assert_file_contains "$MORLOC_BIN/menv" "run"
}

@test "menv: has --rm flag" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "--rm"
}

@test "menv: references docker-compose.yml" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "docker-compose.yml"
}

@test "menv: references .env file" {
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

@test "menv: non-interactive mode uses -T" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" " -T "
}

@test "menv: creates dev directories when --dev" {
    generate_menv_script "$MORLOC_BIN/menv"
    assert_file_contains "$MORLOC_BIN/menv" "mkdir -p"
}

# --- detect_compose_command ---

@test "compose_detect: finds docker compose plugin" {
    run detect_compose_command
    assert_success
}

@test "compose_detect: sets COMPOSE_COMMAND" {
    detect_compose_command
    [ -n "$COMPOSE_COMMAND" ]
}
