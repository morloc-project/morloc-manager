#!/usr/bin/env bats
# Integration tests for the install subcommand
# These use mock container engines - no real Docker/Podman needed

load "../helpers/common"
load "../helpers/mock_engine"

setup() {
    setup_isolated_home
    setup_mock_engine "docker" "24.0.7"
    source_morloc_manager
    # Force the mock engine — auto-detection may find real podman instead
    CONTAINER_ENGINE="docker"
    export MORLOC_BIN="$HOME/.local/bin"
    mkdir -p "$MORLOC_BIN"
}

teardown() {
    teardown_mock_engine
    teardown_isolated_home
}

@test "install: write_version_config creates config directory" {
    write_version_config "0.55.0" "local"
    assert_dir_exists "$(config_root)/versions/0.55.0"
    assert_dir_exists "$(config_root)/versions/0.55.0/environments"
}

@test "install: write_version_config creates version config file" {
    write_version_config "0.55.0" "local"
    local vcfg="$(config_root)/versions/0.55.0/config"
    assert_file_exists "$vcfg"
    assert_file_contains "$vcfg" "image=ghcr.io/morloc-project/morloc/morloc-full:0.55.0"
}

@test "install: write_version_config sets active version in user config" {
    write_version_config "0.55.0" "local"
    local ucfg="$(config_root)/config"
    assert_file_exists "$ucfg"
    assert_file_contains "$ucfg" "active_version=0.55.0"
    assert_file_contains "$ucfg" "active_scope=local"
}

@test "install: write_version_config creates base.conf" {
    write_version_config "0.55.0" "local"
    local base="$(config_root)/versions/0.55.0/environments/base.conf"
    assert_file_exists "$base"
    assert_file_contains "$base" "image=ghcr.io/morloc-project/morloc/morloc-full:0.55.0"
}

@test "install: write_version_config records container engine" {
    write_version_config "0.55.0" "local"
    local vcfg="$(version_config_root "0.55.0" "local")/config"
    assert_file_contains "$vcfg" "container_engine=docker"
}

@test "install: --no-init flag is parsed correctly" {
    run show_install_help
    assert_success
    assert_output --partial "--no-init"
}

@test "install: --system flag is parsed correctly" {
    run show_install_help
    assert_success
    assert_output --partial "--system"
}

@test "install: create_directory makes new directory" {
    local test_dir="$HOME/test-new-dir"
    run create_directory "$test_dir"
    assert_success
    assert_dir_exists "$test_dir"
}

@test "install: create_directory handles existing directory" {
    local test_dir="$HOME/test-existing-dir"
    mkdir -p "$test_dir"
    run create_directory "$test_dir"
    assert_success
    assert_output --partial "already exists"
}

@test "install: version data directory structure is correct" {
    local version="0.55.0"
    local morloc_data_home="$MORLOC_HOST_VERSION_DIR/$version"
    create_directory "$morloc_data_home"
    create_directory "$morloc_data_home/include"
    create_directory "$morloc_data_home/lib"
    create_directory "$morloc_data_home/opt"
    create_directory "$morloc_data_home/src/morloc/plane"
    create_directory "$morloc_data_home/tmp"

    assert_dir_exists "$morloc_data_home"
    assert_dir_exists "$morloc_data_home/include"
    assert_dir_exists "$morloc_data_home/lib"
    assert_dir_exists "$morloc_data_home/opt"
    assert_dir_exists "$morloc_data_home/src/morloc/plane"
    assert_dir_exists "$morloc_data_home/tmp"
}

@test "install: write_version_config with podman records podman" {
    teardown_mock_engine
    setup_mock_engine "podman" "4.7.2"
    CONTAINER_ENGINE="podman"
    write_version_config "0.55.0" "local"
    local vcfg="$(version_config_root "0.55.0" "local")/config"
    assert_file_contains "$vcfg" "container_engine=podman"
}

@test "install: write_version_config sets active_env to base" {
    write_version_config "0.55.0" "local"
    local ucfg="$(config_root)/config"
    assert_file_contains "$ucfg" "active_env=base"
}
