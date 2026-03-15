#!/usr/bin/env bats
# Tests for is_in_path, path_exists_in_file, and related PATH functions

load "../helpers/common"

setup() {
    source_morloc_manager
    setup_isolated_home
    SAVED_PATH="$PATH"
}

teardown() {
    export PATH="$SAVED_PATH"
    teardown_isolated_home
}

# --- is_in_path ---

# Note: is_in_path calls normalize_path which uses sed.  Every test PATH must
# include $SAVED_PATH (appended) so that system tools like sed remain reachable.
# The appended real PATH never contains the synthetic directories we test for,
# so assertions are unaffected.

@test "is_in_path: returns 0 when directory is in PATH" {
    export PATH="/usr/local/bin:$HOME/.local/bin:$SAVED_PATH"
    run is_in_path "/usr/local/bin"
    assert_success
}

@test "is_in_path: returns 1 when directory is not in PATH" {
    export PATH="/usr/local/bin:$SAVED_PATH"
    run is_in_path "/nonexistent/path"
    assert_failure
}

@test "is_in_path: handles empty PATH" {
    export PATH=""
    run is_in_path "/usr/bin"
    assert_failure
}

@test "is_in_path: normalizes trailing slashes" {
    export PATH="/usr/local/bin/:$SAVED_PATH"
    run is_in_path "/usr/local/bin"
    assert_success
}

@test "is_in_path: handles target with trailing slash" {
    export PATH="/usr/local/bin:$SAVED_PATH"
    run is_in_path "/usr/local/bin/"
    assert_success
}

@test "is_in_path: handles single-entry PATH" {
    export PATH="/opt/test-bin:$SAVED_PATH"
    run is_in_path "/opt/test-bin"
    assert_success
}

@test "is_in_path: distinguishes partial matches" {
    export PATH="/usr/local/bin:$SAVED_PATH"
    run is_in_path "/usr/local"
    assert_failure
}

@test "is_in_path: handles HOME/.local/bin" {
    export PATH="$HOME/.local/bin:$SAVED_PATH"
    run is_in_path "$HOME/.local/bin"
    assert_success
}

# --- path_exists_in_file ---

@test "path_exists_in_file: returns 0 when pattern found" {
    local rc_file="$HOME/.bashrc"
    echo 'export PATH="$HOME/.local/bin:$PATH"' > "$rc_file"
    run path_exists_in_file "$rc_file"
    assert_success
}

@test "path_exists_in_file: returns 1 when pattern absent" {
    local rc_file="$HOME/.bashrc"
    echo 'export FOO=bar' > "$rc_file"
    run path_exists_in_file "$rc_file"
    assert_failure
}

@test "path_exists_in_file: returns 1 for missing file" {
    run path_exists_in_file "$HOME/.nonexistent_rc"
    assert_failure
}

@test "path_exists_in_file: returns 1 for empty file" {
    local rc_file="$HOME/.bashrc"
    touch "$rc_file"
    run path_exists_in_file "$rc_file"
    assert_failure
}

# --- resolve_path ---

@test "resolve_path: resolves existing directory" {
    local test_dir="$HOME/testdir"
    mkdir -p "$test_dir"
    run resolve_path "$test_dir"
    assert_success
    assert_output "$test_dir"
}

@test "resolve_path: resolves existing file" {
    local test_file="$HOME/testfile.txt"
    touch "$test_file"
    run resolve_path "$test_file"
    assert_success
    assert_output "$test_file"
}

@test "resolve_path: resolves non-existing file in existing directory" {
    run resolve_path "$HOME/nonexistent.txt"
    assert_success
    assert_output "$HOME/nonexistent.txt"
}
