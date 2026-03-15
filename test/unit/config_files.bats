#!/usr/bin/env bats
# Tests for get_shell_config_files and add_to_config_file functions

load "../helpers/common"

setup() {
    source_morloc_manager
    setup_isolated_home
}

teardown() {
    teardown_isolated_home
}

# --- get_shell_config_files ---

@test "get_shell_config_files: returns .bashrc for bash on Linux" {
    export BASH_VERSION="5.1.16(1)-release"
    unset ZSH_VERSION FISH_VERSION KSH_VERSION tcsh version
    touch "$HOME/.bashrc"
    run get_shell_config_files
    assert_success
    assert_output "$HOME/.bashrc"
}

@test "get_shell_config_files: returns .bashrc as default for bash when no rc exists" {
    export BASH_VERSION="5.1.16(1)-release"
    unset ZSH_VERSION FISH_VERSION KSH_VERSION tcsh version
    # Don't create any rc file - should default to .bashrc
    run get_shell_config_files
    assert_success
    assert_output "$HOME/.bashrc"
}

@test "get_shell_config_files: returns .zshrc for zsh" {
    export ZSH_VERSION="5.9"
    unset BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
    run get_shell_config_files
    assert_success
    assert_output "$HOME/.zshrc"
}

@test "get_shell_config_files: returns config.fish for fish" {
    export FISH_VERSION="3.6.0"
    unset ZSH_VERSION BASH_VERSION KSH_VERSION tcsh version
    run get_shell_config_files
    assert_success
    assert_output "$HOME/.config/fish/config.fish"
}

@test "get_shell_config_files: returns .kshrc for ksh when it exists" {
    export KSH_VERSION="93u+m/1.0.4"
    unset ZSH_VERSION BASH_VERSION FISH_VERSION tcsh version
    touch "$HOME/.kshrc"
    run get_shell_config_files
    assert_success
    assert_output "$HOME/.kshrc"
}

@test "get_shell_config_files: returns .profile for ksh when .kshrc missing" {
    export KSH_VERSION="93u+m/1.0.4"
    unset ZSH_VERSION BASH_VERSION FISH_VERSION tcsh version
    run get_shell_config_files
    assert_success
    assert_output "$HOME/.profile"
}

@test "get_shell_config_files: returns .profile for dash" {
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
    export SHELL="/bin/dash"
    run get_shell_config_files
    assert_success
    assert_output "$HOME/.profile"
}

@test "get_shell_config_files: returns .tcshrc for tcsh" {
    export tcsh="6.24.07"
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION version
    run get_shell_config_files
    assert_success
    assert_output "$HOME/.tcshrc"
}

# --- add_to_config_file ---

@test "add_to_config_file: adds POSIX PATH export to bashrc" {
    export BASH_VERSION="5.1.16(1)-release"
    unset ZSH_VERSION FISH_VERSION KSH_VERSION tcsh version
    local rc_file
    rc_file=$(create_shell_rc "bash")
    run add_to_config_file "$rc_file"
    assert_success
    assert_file_contains "$rc_file" '.local/bin'
    assert_file_contains "$rc_file" 'export PATH'
}

@test "add_to_config_file: adds fish-compatible syntax" {
    export FISH_VERSION="3.6.0"
    unset ZSH_VERSION BASH_VERSION KSH_VERSION tcsh version
    local rc_file
    rc_file=$(create_shell_rc "fish")
    run add_to_config_file "$rc_file"
    assert_success
    assert_file_contains "$rc_file" 'set -gx PATH'
}

@test "add_to_config_file: adds tcsh-compatible syntax" {
    export tcsh="6.24.07"
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION version
    local rc_file
    rc_file=$(create_shell_rc "tcsh")
    run add_to_config_file "$rc_file"
    assert_success
    assert_file_contains "$rc_file" 'set path'
}

@test "add_to_config_file: idempotent - second call is noop" {
    export BASH_VERSION="5.1.16(1)-release"
    unset ZSH_VERSION FISH_VERSION KSH_VERSION tcsh version
    local rc_file
    rc_file=$(create_shell_rc "bash")
    add_to_config_file "$rc_file"
    local first_content
    first_content=$(cat "$rc_file")
    run add_to_config_file "$rc_file"
    assert_success
    # Content should not change on second call
    local second_content
    second_content=$(cat "$rc_file")
    [ "$first_content" = "$second_content" ]
}

@test "add_to_config_file: creates parent directory if needed" {
    export BASH_VERSION="5.1.16(1)-release"
    unset ZSH_VERSION FISH_VERSION KSH_VERSION tcsh version
    local nested_file="$HOME/deep/nested/dir/.bashrc"
    run add_to_config_file "$nested_file"
    assert_success
    assert_file_exists "$nested_file"
}

@test "add_to_config_file: adds zsh PATH export to .zshrc" {
    export ZSH_VERSION="5.9"
    unset BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
    local rc_file
    rc_file=$(create_shell_rc "zsh")
    run add_to_config_file "$rc_file"
    assert_success
    assert_file_contains "$rc_file" 'export PATH'
}
