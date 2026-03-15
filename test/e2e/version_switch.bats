#!/usr/bin/env bats
# End-to-end tests: version switching
# Tests the select subcommand's ability to switch between installed versions.

load "../helpers/common"
load "../helpers/mock_engine"

setup() {
    setup_isolated_home
    setup_mock_engine "docker" "24.0.7"
    export MORLOC_BIN="$HOME/.local/bin"
    mkdir -p "$MORLOC_BIN"
    export PATH="$MORLOC_BIN:$PATH"
    # Pre-populate .bashrc so add_morloc_bin_to_path doesn't prompt
    echo "export PATH=\"$HOME/.local/bin:\$PATH\"" > "$HOME/.bashrc"
}

teardown() {
    teardown_mock_engine
    teardown_isolated_home
}

@test "e2e: select switches menv to target version" {
    # Pre-create two version directories
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        mkdir -p \"\$HOME/\${MORLOC_INSTALL_DIR}/0.54.0\"
        mkdir -p \"\$HOME/\${MORLOC_INSTALL_DIR}/0.55.0\"
        script_menv \"\$MORLOC_BIN/menv\" \"0.54.0\"
    " 2>/dev/null

    assert_file_contains "$MORLOC_BIN/menv" "0.54.0"

    # Switch to 0.55.0
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        cmd_select 0.55.0
    " 2>/dev/null

    assert_file_contains "$MORLOC_BIN/menv" "0.55.0"
    assert_file_not_contains "$MORLOC_BIN/menv" "0.54.0"
}

@test "e2e: select also updates morloc-shell" {
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        mkdir -p \"\$HOME/\${MORLOC_INSTALL_DIR}/0.55.0\"
        cmd_select 0.55.0
    " 2>/dev/null

    assert_file_exists "$MORLOC_BIN/morloc-shell"
    assert_file_contains "$MORLOC_BIN/morloc-shell" "0.55.0"
}

@test "e2e: select fails for non-existent version" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        cmd_select 0.99.99
    "
    assert_failure
    assert_output --partial "does not exist"
}
