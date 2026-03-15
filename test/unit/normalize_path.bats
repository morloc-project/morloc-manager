#!/usr/bin/env bats
# Tests for normalize_path function

load "../helpers/common"

setup() {
    source_morloc_manager
}

@test "normalize_path: removes trailing slash" {
    run normalize_path "/usr/local/bin/"
    assert_success
    assert_output "/usr/local/bin"
}

@test "normalize_path: removes multiple trailing slashes" {
    run normalize_path "/usr/local/bin///"
    assert_success
    assert_output "/usr/local/bin"
}

@test "normalize_path: collapses double slashes in middle" {
    run normalize_path "/usr//local//bin"
    assert_success
    assert_output "/usr/local/bin"
}

@test "normalize_path: preserves root path" {
    run normalize_path "/"
    assert_success
    assert_output "/"
}

@test "normalize_path: handles simple path unchanged" {
    run normalize_path "/home/user/bin"
    assert_success
    assert_output "/home/user/bin"
}

@test "normalize_path: handles path with trailing slash and double slashes" {
    run normalize_path "/home//user//bin/"
    assert_success
    assert_output "/home/user/bin"
}
