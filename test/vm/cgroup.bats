#!/usr/bin/env bats
# cgroup version tests -- verify correct hierarchy and shm-size behavior
#
# Usage:
#   vagrant ssh fedora -c "cd /vagrant && bats test/vm/cgroup.bats"  # cgroup v2
#   vagrant ssh debian -c "cd /vagrant && bats test/vm/cgroup.bats"  # cgroup v1 (after reboot)

load "../helpers/common"

setup() {
    source_morloc_manager
    setup_isolated_home
    detect_available_engine
}

teardown() {
    teardown_isolated_home
}

@test "detect cgroup version" {
    if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
        echo "cgroup v2 (unified hierarchy)"
    elif [ -d /sys/fs/cgroup/cpu ]; then
        echo "cgroup v1 (legacy hierarchy)"
    else
        fail "unknown cgroup layout"
    fi
}

@test "shm-size 256m: /dev/shm has expected size" {
    run "$DETECTED_ENGINE" run --rm --shm-size=256m \
        alpine sh -c "df -m /dev/shm | tail -1 | awk '{print \$2}'"
    assert_success
    local shm_size="${lines[-1]}"
    [ "$shm_size" -ge 256 ]
}

@test "shm-size 4g: /dev/shm has expected size" {
    run "$DETECTED_ENGINE" run --rm --shm-size=4g \
        alpine sh -c "df -m /dev/shm | tail -1 | awk '{print \$2}'"
    assert_success
    local shm_size="${lines[-1]}"
    # 4g = 4096 MB (allow some tolerance)
    [ "$shm_size" -ge 4000 ]
}

@test "shm-size on cgroup v1" {
    require_cgroup_v1

    run "$DETECTED_ENGINE" run --rm --shm-size=512m \
        alpine sh -c "df -m /dev/shm | tail -1 | awk '{print \$2}'"
    assert_success
    local shm_size="${lines[-1]}"
    [ "$shm_size" -ge 512 ]
}

@test "shm-size on cgroup v2" {
    require_cgroup_v2

    run "$DETECTED_ENGINE" run --rm --shm-size=512m \
        alpine sh -c "df -m /dev/shm | tail -1 | awk '{print \$2}'"
    assert_success
    local shm_size="${lines[-1]}"
    [ "$shm_size" -ge 512 ]
}

@test "container resource limits work" {
    run "$DETECTED_ENGINE" run --rm --memory=128m \
        alpine sh -c "cat /sys/fs/cgroup/memory.max 2>/dev/null || cat /sys/fs/cgroup/memory/memory.limit_in_bytes 2>/dev/null"
    assert_success
    echo "memory limit reported: $output"
}
