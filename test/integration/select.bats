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
    # Ensure .local/bin is in PATH for ensure_morloc_bin
    touch "$HOME/.bashrc"
    echo "export PATH=\"$HOME/.local/bin:\$PATH\"" >> "$HOME/.bashrc"
    export PATH="$HOME/.local/bin:$PATH"
    # Pre-create .env file
    mkdir -p "$MORLOC_DATA_HOME"
    cat > "$MORLOC_DATA_HOME/.env" << EOF
MORLOC_VERSION=0.55.0
MORLOC_IMAGE=ghcr.io/morloc-project/morloc/morloc-full:0.55.0
MORLOC_DEV_IMAGE=ghcr.io/morloc-project/morloc/morloc-test:latest
MORLOC_HOST_HOME=$HOME
MORLOC_INSTALL_DIR=$MORLOC_INSTALL_DIR
MORLOC_CONTAINER_ENGINE=docker
EOF
}

teardown() {
    teardown_mock_engine
    teardown_isolated_home
}

@test "select: switches version when installed" {
    local install_dir="$HOME/${MORLOC_INSTALL_DIR}"
    mkdir -p "$install_dir/0.55.0"
    # cmd_select calls exit, test in subshell
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
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
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
        cmd_select 0.99.0
    "
    assert_failure
    assert_output --partial "does not exist"
}

@test "select: no version shows error and lists available" {
    local install_dir="$HOME/${MORLOC_INSTALL_DIR}"
    mkdir -p "$install_dir/0.55.0"
    mkdir -p "$install_dir/0.54.0"
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
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
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
        cmd_select local
    "
    assert_failure
    assert_output --partial "Cannot set to"
}

@test "select: updates .env with new version" {
    local install_dir="$HOME/${MORLOC_INSTALL_DIR}"
    mkdir -p "$install_dir/0.55.0"
    mkdir -p "$install_dir/0.54.0"
    # First select 0.54.0
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
        cmd_select 0.54.0
    " 2>/dev/null || true
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_VERSION=0.54.0"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_IMAGE=ghcr.io/morloc-project/morloc/morloc-full:0.54.0"

    # Then select 0.55.0
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
        cmd_select 0.55.0
    " 2>/dev/null || true
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_VERSION=0.55.0"
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_IMAGE=ghcr.io/morloc-project/morloc/morloc-full:0.55.0"
}

@test "select: does not regenerate menv script" {
    local install_dir="$HOME/${MORLOC_INSTALL_DIR}"
    mkdir -p "$install_dir/0.55.0"
    # Create an menv with a marker
    echo "#!/usr/bin/env sh" > "$MORLOC_BIN/menv"
    echo "# MARKER_DO_NOT_CHANGE" >> "$MORLOC_BIN/menv"
    chmod +x "$MORLOC_BIN/menv"

    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$PATH'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
        cmd_select 0.55.0
    " 2>/dev/null || true

    # menv should still have the marker (not regenerated)
    assert_file_contains "$MORLOC_BIN/menv" "MARKER_DO_NOT_CHANGE"
}
