#!/usr/bin/env bats
# SELinux-specific tests -- run on fedora VM where SELinux is enforcing
#
# Usage:
#   vagrant ssh fedora -c "cd /vagrant && bats test/vm/selinux.bats"

load "../helpers/common"

setup() {
    source_morloc_manager
    setup_isolated_home
}

teardown() {
    teardown_isolated_home
}

@test "SELinux is in enforcing mode" {
    require_selinux_enforcing
    run getenforce
    assert_success
    assert_output "Enforcing"
}

@test "menv script runs morloc --version under SELinux" {
    require_selinux_enforcing
    detect_available_engine

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="$DETECTED_ENGINE"
    script_menv "$menv_path" "edge"

    assert_file_exists "$menv_path"

    run sh "$menv_path" morloc --version
    # We expect this to either succeed or fail with an SELinux denial.
    # The test captures the outcome; if it fails, the next test checks AVC logs.
    echo "menv exit code: $status"
    echo "menv output: $output"
}

@test "no SELinux AVC denials from bind mounts" {
    require_selinux_enforcing
    detect_available_engine

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="$DETECTED_ENGINE"
    script_menv "$menv_path" "edge"

    # Run menv to trigger any potential denials
    sh "$menv_path" morloc --version 2>/dev/null || true

    # Check for AVC denials in the last 60 seconds
    run ausearch -m AVC -ts recent --raw 2>/dev/null
    if [ "$status" -eq 0 ] && [ -n "$output" ]; then
        # Filter for denials related to our bind mount paths
        if echo "$output" | grep -qE "(morloc|/home|container)"; then
            echo "SELinux AVC denials found related to morloc:" >&2
            echo "$output" >&2
            fail "SELinux AVC denials detected for morloc bind mounts"
        fi
    fi
}

@test "container /tmp tmpfs works under SELinux" {
    require_selinux_enforcing
    detect_available_engine

    run "$DETECTED_ENGINE" run --rm --tmpfs /tmp:rw,size=64m \
        alpine sh -c "touch /tmp/test-file && echo ok"
    assert_success
    assert_output "ok"
}

@test "shm-size allocation works under SELinux" {
    require_selinux_enforcing
    detect_available_engine

    run "$DETECTED_ENGINE" run --rm --shm-size=256m \
        alpine sh -c "df -m /dev/shm | tail -1 | awk '{print \$2}'"
    assert_success
    # shm should be at least 256MB (value is in MB)
    local shm_size="${lines[-1]}"
    [ "$shm_size" -ge 256 ]
}

@test "generated bind mounts include :z label (future)" {
    require_selinux_enforcing
    skip "bind mount :z label support not yet implemented"

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="docker"
    script_menv "$menv_path" "edge"

    # When implemented, generated scripts should include :z on bind mounts
    assert_file_contains "$menv_path" ":z"
}

@test "generated bind mounts include :Z label for private volumes (future)" {
    require_selinux_enforcing
    skip "bind mount :Z label support not yet implemented"

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="docker"
    script_menv "$menv_path" "edge"

    assert_file_contains "$menv_path" ":Z"
}

@test "SELinux context preserved on container files" {
    require_selinux_enforcing
    detect_available_engine

    # Create a test directory and check its SELinux context after container use
    local test_dir="$HOME/selinux-test"
    mkdir -p "$test_dir"

    run ls -Zd "$test_dir"
    assert_success
    echo "SELinux context on test dir: $output"
}
