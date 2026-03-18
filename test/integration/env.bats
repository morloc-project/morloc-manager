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
    # Pre-create a .env file so env commands can read version
    mkdir -p "$MORLOC_DATA_HOME"
    cat > "$MORLOC_DATA_HOME/.env" << EOF
MORLOC_VERSION=0.55.0
MORLOC_IMAGE=ghcr.io/morloc-project/morloc/morloc-full:0.55.0
MORLOC_DEV_IMAGE=ghcr.io/morloc-project/morloc/morloc-test:latest
MORLOC_HOST_HOME=$HOME
MORLOC_INSTALL_DIR=$MORLOC_INSTALL_DIR
MORLOC_CONTAINER_ENGINE=docker
MORLOC_ENV_FLAGS=
EOF
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

@test "env: --reset updates .env to default image" {
    # Set a custom image first
    sed -i 's|^MORLOC_IMAGE=.*|MORLOC_IMAGE=morloc-env:0.55.0-ml|' "$MORLOC_DATA_HOME/.env"

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
    # Verify .env was reset to default base image
    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_IMAGE=ghcr.io/morloc-project/morloc/morloc-full:0.55.0"
}

# --- env switching via .env ---

@test "env: activating env updates MORLOC_IMAGE in .env" {
    mkdir -p "$MORLOC_DEPENDENCY_DIR"
    cat > "$MORLOC_DEPENDENCY_DIR/ml.Dockerfile" << 'EOF'
ARG CONTAINER_BASE
FROM ${CONTAINER_BASE}
RUN pip install numpy
EOF

    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$MOCK_ENGINE_DIR:$HOME/.local/bin:/usr/bin:/bin'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
        cmd_env ml
    " 2>/dev/null || true

    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_IMAGE=morloc-env:0.55.0-ml"
}

# --- no env specified ---

@test "env: --init creates flags file stub" {
    run bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env --init ml
    "
    assert_success
    assert_file_exists "$MORLOC_DEPENDENCY_DIR/ml.flags"
}

@test "env: --init flags stub contains example comments" {
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        cmd_env --init flagtest
    " 2>/dev/null || true
    assert_file_contains "$MORLOC_DEPENDENCY_DIR/flagtest.flags" "gpus"
}

@test "env: --list shows flags status" {
    mkdir -p "$MORLOC_DEPENDENCY_DIR"
    touch "$MORLOC_DEPENDENCY_DIR/ml.Dockerfile"
    touch "$MORLOC_DEPENDENCY_DIR/ml.flags"
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
    assert_output --partial "flags:yes"
    assert_output --partial "flags:no"
}

@test "env: --reset clears MORLOC_ENV_FLAGS in .env" {
    # Set a custom env flags path first
    sed -i 's|^MORLOC_ENV_FLAGS=.*|MORLOC_ENV_FLAGS=/some/path/ml.flags|' "$MORLOC_DATA_HOME/.env"

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
    # Verify MORLOC_ENV_FLAGS was cleared
    run grep '^MORLOC_ENV_FLAGS=' "$MORLOC_DATA_HOME/.env"
    assert_output "MORLOC_ENV_FLAGS="
}

@test "env: activating env sets MORLOC_ENV_FLAGS when flags file exists" {
    mkdir -p "$MORLOC_DEPENDENCY_DIR"
    cat > "$MORLOC_DEPENDENCY_DIR/ml.Dockerfile" << 'EOF'
ARG CONTAINER_BASE
FROM ${CONTAINER_BASE}
RUN pip install numpy
EOF
    echo "--gpus all" > "$MORLOC_DEPENDENCY_DIR/ml.flags"

    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export PATH='$MOCK_ENGINE_DIR:$HOME/.local/bin:/usr/bin:/bin'
        source '$SCRIPT_PATH'
        CONTAINER_ENGINE=docker
        MORLOC_BIN='$MORLOC_BIN'
        MORLOC_DEPENDENCY_DIR='$MORLOC_DEPENDENCY_DIR'
        MORLOC_DATA_HOME='$MORLOC_DATA_HOME'
        cmd_env ml
    " 2>/dev/null || true

    assert_file_contains "$MORLOC_DATA_HOME/.env" "MORLOC_ENV_FLAGS=$MORLOC_DEPENDENCY_DIR/ml.flags"
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
