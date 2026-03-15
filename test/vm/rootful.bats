#!/usr/bin/env bats
# Rootful container tests -- ALL skip until rootful support is implemented
#
# These tests define the acceptance criteria for rootful support.
# When rootful support is added to morloc-manager.sh, remove the skip lines.
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
    run sudo docker run --rm alpine echo "rootful-ok"
    assert_success
    assert_output "rootful-ok"
}

@test "rootful: sudo podman run basic execution" {
    require_rootful_support
    run sudo podman run --rm alpine echo "rootful-ok"
    assert_success
    assert_output "rootful-ok"
}

@test "rootful: menv script generation with --rootful flag" {
    require_rootful_support

    local menv_path="$HOME/.local/bin/menv"
    # Future API: script_menv with rootful flag
    CONTAINER_ENGINE="docker"
    script_menv "$menv_path" "edge"

    # When implemented, the generated script should use sudo
    assert_file_contains "$menv_path" "sudo"
}

@test "rootful: menv script runs morloc --version" {
    require_rootful_support

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="docker"
    script_menv "$menv_path" "edge"

    run sudo sh "$menv_path" morloc --version
    assert_success
}

@test "rootful: bind mount permissions (no UID mapping issues)" {
    require_rootful_support

    local test_dir="$HOME/rootful-mount-test"
    mkdir -p "$test_dir"
    echo "test" > "$test_dir/input.txt"

    run sudo docker run --rm -v "$test_dir:/mnt/test" alpine cat /mnt/test/input.txt
    assert_success
    assert_output "test"

    # Rootful should not have UID mapping issues
    run sudo docker run --rm -v "$test_dir:/mnt/test" alpine sh -c "echo written > /mnt/test/output.txt"
    assert_success

    # File should be readable by the current user (may be root-owned)
    [ -f "$test_dir/output.txt" ]
}

@test "rootful: shm-size allocation" {
    require_rootful_support

    run sudo docker run --rm --shm-size=4g \
        alpine sh -c "df -m /dev/shm | tail -1 | awk '{print \$2}'"
    assert_success
    local shm_size="${lines[-1]}"
    [ "$shm_size" -ge 4000 ]
}

@test "rootful: auto-detect rootful vs rootless" {
    require_rootful_support
    # Future: morloc-manager should detect if user has rootless access
    # and fall back to rootful if not
}

@test "rootful: generated scripts use correct engine invocation" {
    require_rootful_support

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="docker"
    script_menv "$menv_path" "edge"

    # When rootful is implemented, verify the generated script
    # uses the correct sudo/engine combination
    assert_file_exists "$menv_path"
}
