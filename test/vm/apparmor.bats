#!/usr/bin/env bats
# AppArmor-specific tests -- run on ubuntu VM where AppArmor is active
#
# Usage:
#   vagrant ssh ubuntu -c "cd /vagrant && bats test/vm/apparmor.bats"

load "../helpers/common"

setup() {
    source_morloc_manager
    setup_isolated_home
}

teardown() {
    teardown_isolated_home
}

@test "AppArmor is active" {
    require_apparmor_active
    run aa-status
    assert_success
}

@test "menv script runs morloc --version under AppArmor" {
    require_apparmor_active
    detect_available_engine

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="$DETECTED_ENGINE"
    script_menv "$menv_path" "edge"

    assert_file_exists "$menv_path"

    run sh "$menv_path" morloc --version
    echo "menv exit code: $status"
    echo "menv output: $output"
}

@test "no AppArmor denials after container run" {
    require_apparmor_active
    detect_available_engine

    local menv_path="$HOME/.local/bin/menv"
    CONTAINER_ENGINE="$DETECTED_ENGINE"
    script_menv "$menv_path" "edge"

    # Run menv to trigger any potential denials
    sh "$menv_path" morloc --version 2>/dev/null || true

    # Check dmesg for AppArmor denials
    run dmesg 2>/dev/null
    if [ "$status" -eq 0 ]; then
        if echo "$output" | grep -qi "apparmor.*denied.*morloc"; then
            echo "AppArmor denials found:" >&2
            echo "$output" | grep -i "apparmor.*denied" >&2
            fail "AppArmor denials detected for morloc operations"
        fi
    fi

    # Also check journalctl
    run journalctl -k --no-pager -n 50 2>/dev/null
    if [ "$status" -eq 0 ]; then
        if echo "$output" | grep -qi "apparmor.*denied.*morloc"; then
            fail "AppArmor denials found in journal"
        fi
    fi
}

@test "bind mount permissions work under AppArmor" {
    require_apparmor_active
    detect_available_engine

    local test_dir="$HOME/apparmor-test"
    mkdir -p "$test_dir"
    echo "test-content" > "$test_dir/input.txt"

    run "$DETECTED_ENGINE" run --rm \
        -v "$test_dir:/mnt/test" \
        alpine cat /mnt/test/input.txt
    assert_success
    assert_output "test-content"
}

@test "container can write to bind-mounted directory under AppArmor" {
    require_apparmor_active
    detect_available_engine

    local test_dir="$HOME/apparmor-write-test"
    mkdir -p "$test_dir"

    run "$DETECTED_ENGINE" run --rm \
        -v "$test_dir:/mnt/test" \
        alpine sh -c "echo written > /mnt/test/output.txt"
    assert_success

    [ -f "$test_dir/output.txt" ]
    run cat "$test_dir/output.txt"
    assert_output "written"
}

@test "shm-size allocation works under AppArmor" {
    require_apparmor_active
    detect_available_engine

    run "$DETECTED_ENGINE" run --rm --shm-size=256m \
        alpine sh -c "df -m /dev/shm | tail -1 | awk '{print \$2}'"
    assert_success
    local shm_size="${lines[-1]}"
    [ "$shm_size" -ge 256 ]
}
