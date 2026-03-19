#!/usr/bin/env bats
# Tests for is_in_path, ensure_morloc_bin, and related PATH functions

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

# --- ensure_morloc_bin ---

@test "ensure_morloc_bin: succeeds silently when already in PATH" {
    export MORLOC_BIN="$HOME/.local/bin"
    export PATH="$MORLOC_BIN:$SAVED_PATH"
    run ensure_morloc_bin
    assert_success
    assert_output --partial "is in PATH"
}

@test "ensure_morloc_bin: detects profile reference and advises" {
    export MORLOC_BIN="$HOME/.local/bin"
    # Remove MORLOC_BIN from PATH
    export PATH="$SAVED_PATH"
    # Create a .profile that references .local/bin
    echo 'export PATH="$HOME/.local/bin:$PATH"' > "$HOME/.profile"
    run ensure_morloc_bin
    assert_success
    assert_output --partial "not yet in your PATH, but will be on next login"
}

@test "ensure_morloc_bin: advises user when not in PATH and no profile" {
    export MORLOC_BIN="$HOME/.local/bin"
    # Remove MORLOC_BIN from PATH
    export PATH="$SAVED_PATH"
    # No .profile exists
    run ensure_morloc_bin
    assert_success
    assert_output --partial "not in your PATH"
    assert_output --partial "~/.profile"
}

@test "ensure_morloc_bin: creates MORLOC_BIN directory if missing" {
    export MORLOC_BIN="$HOME/.local/newbin"
    export PATH="$SAVED_PATH"
    run ensure_morloc_bin
    assert_success
    assert_dir_exists "$HOME/.local/newbin"
}

@test "ensure_morloc_bin: never modifies rc files" {
    export MORLOC_BIN="$HOME/.local/bin"
    export PATH="$SAVED_PATH"
    # Create rc files to verify they are untouched
    touch "$HOME/.bashrc"
    touch "$HOME/.zshrc"
    local bashrc_before zshrc_before
    bashrc_before=$(cat "$HOME/.bashrc")
    zshrc_before=$(cat "$HOME/.zshrc")
    run ensure_morloc_bin
    assert_success
    [ "$(cat "$HOME/.bashrc")" = "$bashrc_before" ]
    [ "$(cat "$HOME/.zshrc")" = "$zshrc_before" ]
}
