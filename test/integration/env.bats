#!/usr/bin/env bats
# Integration tests for the env subcommand

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
    export MORLOC_DEPENDENCY_DIR="$HOME/.local/share/morloc/deps"
}

teardown() {
    teardown_mock_engine
    teardown_isolated_home
}

# --- init ---

@test "env: --init creates Dockerfile stub" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env --init ml
    "
    assert_success
    assert_file_exists "$MORLOC_DEPENDENCY_DIR/ml.Dockerfile"
}

@test "env: --init stub contains ARG CONTAINER_BASE" {
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env --init testenv
    " 2>/dev/null || true
    assert_file_contains "$MORLOC_DEPENDENCY_DIR/testenv.Dockerfile" "ARG CONTAINER_BASE"
}

@test "env: --init stub contains FROM with base arg" {
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env --init testenv2
    " 2>/dev/null || true
    assert_file_contains "$MORLOC_DEPENDENCY_DIR/testenv2.Dockerfile" 'FROM ${CONTAINER_BASE}'
}

@test "env: --init fails if env already exists" {
    mkdir -p "$MORLOC_DEPENDENCY_DIR"
    touch "$MORLOC_DEPENDENCY_DIR/existing.Dockerfile"
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env --init existing
    "
    assert_failure
    assert_output --partial "already exists"
}

# --- list ---

@test "env: --list with no environments" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env --list
    "
    assert_success
    assert_output --partial "No dependency environments"
}

@test "env: --list shows environments" {
    mkdir -p "$MORLOC_DEPENDENCY_DIR"
    touch "$MORLOC_DEPENDENCY_DIR/ml.Dockerfile"
    touch "$MORLOC_DEPENDENCY_DIR/bio.Dockerfile"
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        export MORLOC_BIN='$MORLOC_BIN'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env --list
    "
    assert_success
    assert_output --partial "ml"
    assert_output --partial "bio"
}

# --- reset ---

@test "env: --reset produces success message" {
    # Create mock menv that returns a version
    cat > "$MORLOC_BIN/menv" << 'EOF'
#!/bin/sh
if [ "$1" = "morloc" ] && [ "$2" = "--version" ]; then
    echo "0.55.0"
    exit 0
fi
echo "mock-menv: $*"
exit 0
EOF
    chmod +x "$MORLOC_BIN/menv"

    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$MOCK_ENGINE_DIR:$HOME/.local/bin:/usr/bin:/bin'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        cmd_env --reset
    "
    assert_success
    assert_output --partial "reset"
}

# --- no env specified ---

@test "env: no env name shows error message" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env
    "
    # cmd_env exits 0 even on error (it prints error + help then exits 0)
    assert_output --partial "No environment specified"
}
