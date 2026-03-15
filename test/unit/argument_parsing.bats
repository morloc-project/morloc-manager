#!/usr/bin/env bats
# Tests for main argument parsing and subcommand dispatch

load "../helpers/common"

setup() {
    source_morloc_manager
}

# --- --help ---

@test "main: --help shows usage" {
    run main --help
    assert_success
    assert_output --partial "USAGE"
    assert_output --partial "COMMANDS"
}

@test "main: -h shows usage" {
    run main -h
    assert_success
    assert_output --partial "USAGE"
}

# --- --version ---

@test "main: --version shows version" {
    run main --version
    assert_success
    assert_output "$VERSION"
}

@test "main: -v shows version" {
    run main -v
    assert_success
    assert_output "$VERSION"
}

# --- unknown flags ---

@test "main: unknown flag shows error" {
    run main --bogus
    assert_failure
    assert_output --partial "Unknown option"
}

# --- subcommand help ---

@test "main: install --help shows install usage" {
    run main install --help
    assert_success
    assert_output --partial "install"
    assert_output --partial "USAGE"
}

@test "main: uninstall --help shows uninstall usage" {
    run main uninstall --help
    assert_success
    assert_output --partial "uninstall"
}

@test "main: update --help shows update usage" {
    run main update --help
    assert_success
    assert_output --partial "update"
}

@test "main: select --help shows select usage" {
    run main select --help
    assert_success
    assert_output --partial "select"
}

@test "main: env --help shows env usage" {
    run main env --help
    assert_success
    assert_output --partial "env"
}

@test "main: info --help shows info usage" {
    run main info --help
    assert_success
    assert_output --partial "info"
}

# --- unknown subcommand ---

@test "main: unknown subcommand shows error" {
    run main frobnicate
    assert_failure
    assert_output --partial "Unknown command"
}

# --- empty invocation ---

@test "main: no args shows help" {
    run main
    assert_success
    assert_output --partial "USAGE"
}

# --- install flag parsing ---

@test "install: unknown flag is rejected" {
    run cmd_install --unknown-flag
    assert_failure
    assert_output --partial "Unknown option"
}

# --- --container-engine ---

@test "main: --container-engine without argument fails" {
    run main --container-engine
    assert_failure
}
