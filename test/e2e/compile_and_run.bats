#!/usr/bin/env bats
# End-to-end tests: compile and run morloc programs
# Requires a real container engine with morloc images available.
# These are slow tests - typically run in CI matrix or manually.

load "../helpers/common"

FIXTURES_DIR="$TEST_DIR/fixtures"

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
        skip "morloc-full:edge image not available (run 'install' first)"
    fi

    setup_isolated_home
    export MORLOC_CONTAINER_ENGINE="$ENGINE"

    # Generate menv script
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

@test "e2e: menv morloc --version returns version" {
    run "$MENV" morloc --version
    assert_success
    assert_output --regexp '^[0-9]+\.[0-9]+\.[0-9]+$'
}

@test "e2e: morloc make compiles hello.loc" {
    local work_dir
    work_dir=$(mktemp -d)
    cp "$FIXTURES_DIR/hello.loc" "$work_dir/"
    cd "$work_dir"

    # Run init + install + make in one container so the nexus binary and
    # installed modules are all available in the same filesystem.
    # morloc init installs morloc-nexus to $HOME/.local/bin/ which is not
    # in the container's PATH (HOME was overridden via -e HOME=...), so we
    # must add it explicitly.
    run "$MENV" sh -c '
        export PATH="$HOME/.local/bin:$PATH"
        morloc init -f >/dev/null 2>&1
        morloc install root-py >/dev/null 2>&1
        morloc make -o hello hello.loc
    '
    assert_success

    rm -rf "$work_dir"
}

@test "e2e: compiled program produces correct output" {
    local work_dir
    work_dir=$(mktemp -d)
    cp "$FIXTURES_DIR/hello.loc" "$work_dir/"
    cd "$work_dir"

    # Run everything in a single container so morloc-nexus is available.
    # $HOME/.local/bin must be in PATH for the compiled program to find
    # morloc-nexus (installed there by morloc init).
    run "$MENV" sh -c '
        export PATH="$HOME/.local/bin:$PATH"
        morloc init -f >/dev/null 2>&1
        morloc install root-py >/dev/null 2>&1
        morloc make -o hello hello.loc >/dev/null 2>&1
        ./hello double 21
    '
    assert_success
    assert_output "42"

    rm -rf "$work_dir"
}
