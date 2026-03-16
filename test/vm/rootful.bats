#!/usr/bin/env bats
# Rootful container tests -- require sudo access
#
# Usage:
#   vagrant ssh fedora -c "cd /vagrant && bats test/vm/rootful.bats"

load "../helpers/common"

setup() {
    source_morloc_manager
    setup_isolated_home
}

teardown() {
    teardown_isolated_home
}

@test "rootful: sudo docker run basic execution" {
    require_rootful_support
    if ! command -v docker >/dev/null 2>&1; then
        skip "docker not installed"
    fi
    run sudo docker run --rm alpine echo "rootful-ok"
    assert_success
    assert_output "rootful-ok"
}

@test "rootful: sudo podman run basic execution" {
    require_rootful_support
    if ! command -v podman >/dev/null 2>&1; then
        skip "podman not installed"
    fi
    run sudo podman run --rm alpine echo "rootful-ok"
    assert_success
    assert_output "rootful-ok"
}

@test "rootful: menv script generation with --rootful flag" {
    require_rootful_support
    detect_available_engine

    SUDO_PREFIX="sudo "
    CONTAINER_ENGINE="$DETECTED_ENGINE"
    local menv_path="$HOME/.local/bin/menv"
    script_menv "$menv_path" "edge"

    assert_file_contains "$menv_path" "sudo"
    assert_file_contains "$menv_path" "$DETECTED_ENGINE run"
}

@test "rootful: menv script runs morloc --version" {
    require_rootful_support
    detect_available_engine

    SUDO_PREFIX="sudo "
    CONTAINER_ENGINE="$DETECTED_ENGINE"
    local menv_path="$HOME/.local/bin/menv"
    script_menv "$menv_path" "edge"

    run sh "$menv_path" morloc --version
    assert_success
}

@test "rootful: bind mount permissions (no UID mapping issues)" {
    require_rootful_support
    detect_available_engine

    local test_dir="$HOME/rootful-mount-test"
    mkdir -p "$test_dir"
    echo "test" > "$test_dir/input.txt"

    run sudo "$DETECTED_ENGINE" run --rm -v "$test_dir:/mnt/test" alpine cat /mnt/test/input.txt
    assert_success
    assert_output "test"

    # Rootful should not have UID mapping issues
    run sudo "$DETECTED_ENGINE" run --rm -v "$test_dir:/mnt/test" alpine sh -c "echo written > /mnt/test/output.txt"
    assert_success

    # File should be readable by the current user (may be root-owned)
    [ -f "$test_dir/output.txt" ]
}

@test "rootful: shm-size allocation" {
    require_rootful_support
    detect_available_engine

    run sudo "$DETECTED_ENGINE" run --rm --shm-size=4g \
        alpine sh -c "df -m /dev/shm | tail -1 | awk '{print \$2}'"
    assert_success
    local shm_size="${lines[-1]}"
    [ "$shm_size" -ge 4000 ]
}

@test "rootful: generated scripts use correct engine invocation" {
    require_rootful_support
    detect_available_engine

    SUDO_PREFIX="sudo "
    CONTAINER_ENGINE="$DETECTED_ENGINE"
    local menv_path="$HOME/.local/bin/menv"
    script_menv "$menv_path" "edge"

    assert_file_exists "$menv_path"
    assert_file_contains "$menv_path" "sudo $DETECTED_ENGINE run"
}
