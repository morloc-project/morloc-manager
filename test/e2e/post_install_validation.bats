#!/usr/bin/env bats
# End-to-end tests: post-installation validation
# These tests verify that a full installation is healthy.
# Requires real container engine with morloc images pulled.
# Intended for release validation and weekly CI runs.

load "../helpers/common"

setup() {
    if [ -n "${MORLOC_CONTAINER_ENGINE:-}" ]; then
        ENGINE="$MORLOC_CONTAINER_ENGINE"
    elif command -v docker >/dev/null 2>&1; then
        ENGINE="docker"
    elif command -v podman >/dev/null 2>&1; then
        ENGINE="podman"
    else
        skip "No container engine available"
    fi

    # Check if morloc-full image is available
    if ! $ENGINE image inspect ghcr.io/morloc-project/morloc/morloc-full:edge >/dev/null 2>&1; then
        skip "morloc-full:edge image not available"
    fi

    setup_isolated_home
    export MORLOC_CONTAINER_ENGINE="$ENGINE"

    # Set up menv
    bash -c "
        export MORLOC_MANAGER_TESTING=1
        export HOME='$HOME'
        export MORLOC_CONTAINER_ENGINE='$ENGINE'
        source '$SCRIPT_PATH'
        MORLOC_BIN='$HOME/.local/bin'
        mkdir -p \$MORLOC_BIN
        morloc_data_home=\"\$HOME/\${MORLOC_INSTALL_DIR}/edge\"
        create_directory \"\$morloc_data_home\"
        create_directory \"\$morloc_data_home/include\"
        create_directory \"\$morloc_data_home/lib\"
        create_directory \"\$morloc_data_home/opt\"
        create_directory \"\$morloc_data_home/src/morloc/plane\"
        create_directory \"\$morloc_data_home/tmp\"
        script_menv \"\$MORLOC_BIN/menv\" \"edge\"
    " 2>/dev/null

    MENV="$HOME/.local/bin/menv"
}

teardown() {
    teardown_isolated_home
}

@test "e2e: morloc binary exists in container" {
    run "$MENV" which morloc
    assert_success
}

@test "e2e: morloc --version reports valid version" {
    run "$MENV" morloc --version
    assert_success
    assert_output --regexp '^[0-9]+\.[0-9]+\.[0-9]+$'
}

@test "e2e: morloc init succeeds in container" {
    run "$MENV" morloc init -f
    assert_success
}

@test "e2e: container has Python3 available" {
    run "$MENV" python3 --version
    assert_success
    assert_output --partial "Python 3"
}

@test "e2e: container has g++ available" {
    run "$MENV" g++ --version
    assert_success
    assert_output --partial "g++"
}

@test "e2e: container has R available" {
    run "$MENV" R --version
    assert_success
    assert_output --partial "R version"
}
