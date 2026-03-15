#!/usr/bin/env bats
# Tests for detect_shell function

load "../helpers/common"

setup() {
    source_morloc_manager
    # Clear all shell detection variables
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
}

@test "detect_shell: detects bash via BASH_VERSION" {
    export BASH_VERSION="5.1.16(1)-release"
    unset ZSH_VERSION FISH_VERSION KSH_VERSION tcsh version
    run detect_shell
    assert_success
    assert_output "bash"
}

@test "detect_shell: detects zsh via ZSH_VERSION" {
    export ZSH_VERSION="5.9"
    unset BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
    run detect_shell
    assert_success
    assert_output "zsh"
}

@test "detect_shell: ZSH_VERSION takes precedence over BASH_VERSION" {
    export ZSH_VERSION="5.9"
    export BASH_VERSION="5.1.16(1)-release"
    run detect_shell
    assert_success
    assert_output "zsh"
}

@test "detect_shell: detects fish via FISH_VERSION" {
    export FISH_VERSION="3.6.0"
    unset ZSH_VERSION BASH_VERSION KSH_VERSION tcsh version
    run detect_shell
    assert_success
    assert_output "fish"
}

@test "detect_shell: detects ksh via KSH_VERSION" {
    export KSH_VERSION="93u+m/1.0.4"
    unset ZSH_VERSION BASH_VERSION FISH_VERSION tcsh version
    run detect_shell
    assert_success
    assert_output "ksh"
}

@test "detect_shell: detects tcsh via tcsh variable" {
    export tcsh="6.24.07"
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION version
    run detect_shell
    assert_success
    assert_output "tcsh"
}

@test "detect_shell: falls back to SHELL env var for bash" {
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
    export SHELL="/bin/bash"
    run detect_shell
    assert_success
    assert_output "bash"
}

@test "detect_shell: falls back to SHELL env var for zsh" {
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
    export SHELL="/usr/bin/zsh"
    run detect_shell
    assert_success
    assert_output "zsh"
}

@test "detect_shell: falls back to SHELL env var for dash" {
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
    export SHELL="/bin/dash"
    run detect_shell
    assert_success
    assert_output "dash"
}

@test "detect_shell: falls back to SHELL env var for ash" {
    unset ZSH_VERSION BASH_VERSION FISH_VERSION KSH_VERSION tcsh version
    export SHELL="/bin/ash"
    run detect_shell
    assert_success
    assert_output "ash"
}
