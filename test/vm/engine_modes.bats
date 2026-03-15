#!/usr/bin/env bats
# Engine x mode matrix tests -- run on any VM with both Docker and Podman
#
# Tests rootless docker, rootless podman, and (future) rootful modes.
#
# Usage:
#   vagrant ssh fedora -c "cd /vagrant && bats test/vm/engine_modes.bats"

load "../helpers/common"

setup() {
    source_morloc_manager
    setup_isolated_home
}

teardown() {
    teardown_isolated_home
}

# ---- Rootless Docker ----

@test "rootless docker: engine is available" {
    if ! command -v docker >/dev/null 2>&1; then
        skip "docker not installed"
    fi
    run docker info
    assert_success
}

@test "rootless docker: menv runs morloc --version" {
    if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
        skip "docker not available"
    fi

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="docker"
    script_menv "$menv_path" "edge"

    assert_file_exists "$menv_path"
    run sh "$menv_path" morloc --version
    echo "exit=$status output=$output"
}

@test "rootless docker: bind mount read/write works" {
    if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
        skip "docker not available"
    fi

    local test_dir="$HOME/docker-mount-test"
    mkdir -p "$test_dir"
    echo "hello" > "$test_dir/in.txt"

    run docker run --rm -v "$test_dir:/mnt/t" alpine sh -c "cat /mnt/t/in.txt && echo world > /mnt/t/out.txt"
    assert_success

    run cat "$test_dir/out.txt"
    assert_output "world"
}

# ---- Rootless Podman ----

@test "rootless podman: engine is available" {
    if ! command -v podman >/dev/null 2>&1; then
        skip "podman not installed"
    fi
    run podman info
    assert_success
}

@test "rootless podman: menv runs morloc --version" {
    if ! command -v podman >/dev/null 2>&1 || ! podman info >/dev/null 2>&1; then
        skip "podman not available"
    fi

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="podman"
    script_menv "$menv_path" "edge"

    assert_file_exists "$menv_path"
    run sh "$menv_path" morloc --version
    echo "exit=$status output=$output"
}

@test "rootless podman: bind mount read/write works" {
    if ! command -v podman >/dev/null 2>&1 || ! podman info >/dev/null 2>&1; then
        skip "podman not available"
    fi

    local test_dir="$HOME/podman-mount-test"
    mkdir -p "$test_dir"
    echo "hello" > "$test_dir/in.txt"

    run podman run --rm -v "$test_dir:/mnt/t" alpine sh -c "cat /mnt/t/in.txt && echo world > /mnt/t/out.txt"
    assert_success

    run cat "$test_dir/out.txt"
    assert_output "world"
}

@test "rootless podman (as testuser): engine is available" {
    if ! id testuser >/dev/null 2>&1; then
        skip "testuser does not exist"
    fi
    run su - testuser -c "podman info" 2>/dev/null
    if [ "$status" -ne 0 ]; then
        skip "podman not available for testuser"
    fi
    assert_success
}

# ---- Rootful Docker ----

@test "rootful docker: sudo docker run" {
    require_rootful_support
    run sudo docker run --rm alpine echo "rootful-docker-ok"
    assert_success
    assert_output "rootful-docker-ok"
}

@test "rootful docker: menv script generation" {
    require_rootful_support
    # Future: CONTAINER_ENGINE="sudo docker" or --rootful flag
}

# ---- Rootful Podman ----

@test "rootful podman: sudo podman run" {
    require_rootful_support
    run sudo podman run --rm alpine echo "rootful-podman-ok"
    assert_success
    assert_output "rootful-podman-ok"
}

@test "rootful podman: menv script generation" {
    require_rootful_support
    # Future: CONTAINER_ENGINE="sudo podman" or --rootful flag
}
