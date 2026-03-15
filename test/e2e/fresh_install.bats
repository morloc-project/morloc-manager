#!/usr/bin/env bats
# End-to-end tests: fresh installation experience
# These tests require a real container engine (Docker or Podman).
# Skip if no engine is available.

load "../helpers/common"

setup() {
    # Detect container engine
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
}

teardown() {
    teardown_isolated_home
}

@test "e2e: fresh install creates all four wrapper scripts" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_CONTAINER_ENGINE='$ENGINE'
        source '$SCRIPT_PATH'
        MORLOC_BIN='$HOME/.local/bin'
        mkdir -p \$MORLOC_BIN
        script_menv         \"\$MORLOC_BIN/menv\" \"0.55.0\"
        script_morloc_shell \"\$MORLOC_BIN/morloc-shell\" \"0.55.0\"
        script_menv_dev     \"\$MORLOC_BIN/menv-dev\"
        script_morloc_dev_shell \"\$MORLOC_BIN/morloc-shell-dev\"
    "
    assert_success
    assert_file_exists "$HOME/.local/bin/menv"
    assert_file_exists "$HOME/.local/bin/morloc-shell"
    assert_file_exists "$HOME/.local/bin/menv-dev"
    assert_file_exists "$HOME/.local/bin/morloc-shell-dev"
}

@test "e2e: all wrapper scripts are executable" {
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_CONTAINER_ENGINE='$ENGINE'
        source '$SCRIPT_PATH'
        MORLOC_BIN='$HOME/.local/bin'
        mkdir -p \$MORLOC_BIN
        script_menv         \"\$MORLOC_BIN/menv\" \"0.55.0\"
        script_morloc_shell \"\$MORLOC_BIN/morloc-shell\" \"0.55.0\"
        script_menv_dev     \"\$MORLOC_BIN/menv-dev\"
        script_morloc_dev_shell \"\$MORLOC_BIN/morloc-shell-dev\"
    " 2>/dev/null

    [ -x "$HOME/.local/bin/menv" ]
    [ -x "$HOME/.local/bin/morloc-shell" ]
    [ -x "$HOME/.local/bin/menv-dev" ]
    [ -x "$HOME/.local/bin/morloc-shell-dev" ]
}

@test "e2e: wrapper scripts reference the correct engine" {
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_CONTAINER_ENGINE='$ENGINE'
        source '$SCRIPT_PATH'
        MORLOC_BIN='$HOME/.local/bin'
        mkdir -p \$MORLOC_BIN
        script_menv \"\$MORLOC_BIN/menv\" \"0.55.0\"
    " 2>/dev/null

    assert_file_contains "$HOME/.local/bin/menv" "$ENGINE run"
}

@test "e2e: version directory structure is created correctly" {
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_CONTAINER_ENGINE='$ENGINE'
        source '$SCRIPT_PATH'
        morloc_data_home=\"\$HOME/\${MORLOC_INSTALL_DIR}/0.55.0\"
        create_directory \"\$morloc_data_home\"
        create_directory \"\$morloc_data_home/include\"
        create_directory \"\$morloc_data_home/lib\"
        create_directory \"\$morloc_data_home/opt\"
        create_directory \"\$morloc_data_home/src/morloc/plane\"
        create_directory \"\$morloc_data_home/tmp\"
    " 2>/dev/null

    assert_dir_exists "$HOME/.local/share/morloc/versions/0.55.0"
    assert_dir_exists "$HOME/.local/share/morloc/versions/0.55.0/include"
    assert_dir_exists "$HOME/.local/share/morloc/versions/0.55.0/lib"
    assert_dir_exists "$HOME/.local/share/morloc/versions/0.55.0/opt"
    assert_dir_exists "$HOME/.local/share/morloc/versions/0.55.0/src/morloc/plane"
    assert_dir_exists "$HOME/.local/share/morloc/versions/0.55.0/tmp"
}

@test "e2e: --version flag works" {
    run bash "$SCRIPT_PATH" --version
    assert_success
    # Should output a version string like X.Y.Z
    assert_output --regexp '^[0-9]+\.[0-9]+\.[0-9]+$'
}

@test "e2e: --help flag works" {
    run bash "$SCRIPT_PATH" --help
    assert_success
    assert_output --partial "USAGE"
    assert_output --partial "install"
    assert_output --partial "uninstall"
}
