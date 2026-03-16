#!/usr/bin/env bats
# Tests for detect_selinux() function

load "../helpers/common"

setup() {
    source_morloc_manager
    SELINUX_SUFFIX=""
}

teardown() {
    teardown_mock_getenforce
}

@test "detect_selinux: sets SELINUX_SUFFIX when getenforce returns Enforcing" {
    setup_mock_getenforce "Enforcing"
    detect_selinux
    [ "$SELINUX_SUFFIX" = ":z" ]
}

@test "detect_selinux: sets SELINUX_SUFFIX when getenforce returns Permissive" {
    setup_mock_getenforce "Permissive"
    detect_selinux
    [ "$SELINUX_SUFFIX" = ":z" ]
}

@test "detect_selinux: SELINUX_SUFFIX empty when getenforce returns Disabled" {
    setup_mock_getenforce "Disabled"
    detect_selinux
    [ -z "$SELINUX_SUFFIX" ]
}

@test "detect_selinux: SELINUX_SUFFIX empty when getenforce not found" {
    # Ensure no getenforce on PATH (don't add a mock)
    detect_selinux
    [ -z "$SELINUX_SUFFIX" ]
}
