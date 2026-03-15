#!/usr/bin/env bats
# Integration tests for the update subcommand

load "../helpers/common"

setup() {
    source_morloc_manager
}

@test "update: --help shows usage" {
    run cmd_update --help
    assert_success
    assert_output --partial "update"
    assert_output --partial "USAGE"
}

@test "update: unexpected argument is rejected" {
    run cmd_update --bogus
    assert_failure
    assert_output --partial "Unexpected argument"
}

@test "update: show_update_help includes examples" {
    run show_update_help
    assert_success
    assert_output --partial "EXAMPLES"
}
