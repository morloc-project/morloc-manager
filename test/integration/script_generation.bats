#!/usr/bin/env bats
# Integration tests for config directory structure (replaces old .env/menv generation tests)

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

# --- config helpers ---

@test "config_root: returns XDG config dir for local" {
    run config_root
    assert_success
    assert_output "$HOME/.config/morloc"
}

@test "config_root: returns /etc/morloc for system" {
    run config_root --system
    assert_success
    assert_output "/etc/morloc"
}

@test "data_root: returns XDG data dir for local" {
    run data_root
    assert_success
    assert_output "$HOME/.local/share/morloc"
}

@test "data_root: returns /usr/local/share/morloc for system" {
    run data_root --system
    assert_success
    assert_output "/usr/local/share/morloc"
}

@test "bin_root: returns local bin for local" {
    run bin_root
    assert_success
    assert_output "$HOME/.local/bin"
}

@test "bin_root: returns /usr/bin for system" {
    run bin_root --system
    assert_success
    assert_output "/usr/bin"
}

# --- read_config / write_config ---

@test "read_config: reads existing key" {
    local cfg="$HOME/.config/morloc/config"
    mkdir -p "$(dirname "$cfg")"
    printf 'active_version=0.55.0\nactive_scope=local\n' > "$cfg"
    run read_config "active_version" "$cfg"
    assert_success
    assert_output "0.55.0"
}

@test "read_config: returns empty for missing key" {
    local cfg="$HOME/.config/morloc/config"
    mkdir -p "$(dirname "$cfg")"
    printf 'active_scope=local\n' > "$cfg"
    run read_config "active_version" "$cfg"
    assert_success
    assert_output ""
}

@test "write_config: creates file and key" {
    local cfg="$HOME/test-config/config"
    write_config "mykey" "myval" "$cfg"
    assert_file_exists "$cfg"
    assert_file_contains "$cfg" "mykey=myval"
}

@test "write_config: updates existing key" {
    local cfg="$HOME/test-config/config"
    mkdir -p "$(dirname "$cfg")"
    printf 'mykey=old\n' > "$cfg"
    write_config "mykey" "new" "$cfg"
    assert_file_contains "$cfg" "mykey=new"
    assert_file_not_contains "$cfg" "mykey=old"
}

# --- active_version / active_scope ---

@test "active_version: reads from user config" {
    setup_version_config "0.55.0" "local"
    run active_version
    assert_success
    assert_output "0.55.0"
}

@test "active_scope: defaults to local" {
    mkdir -p "$(config_root)"
    printf '' > "$(config_root)/config"
    run active_scope
    assert_success
    assert_output "local"
}

@test "active_scope: reads system when set" {
    setup_version_config "0.55.0" "local"
    write_config "active_scope" "system" "$(config_root)/config"
    run active_scope
    assert_success
    assert_output "system"
}

# --- resolve_version ---

@test "resolve_version: finds local version" {
    mkdir -p "$(data_root)/versions/0.55.0"
    run resolve_version "0.55.0"
    assert_success
    assert_output "local"
}

@test "resolve_version: fails for missing version" {
    run resolve_version "0.99.0"
    assert_failure
}

# --- list_versions ---

@test "list_versions: lists local versions" {
    mkdir -p "$(data_root)/versions/0.55.0"
    mkdir -p "$(data_root)/versions/0.54.0"
    run list_versions --local
    assert_success
    assert_output --partial "0.55.0"
    assert_output --partial "0.54.0"
}

@test "list_versions: excludes local version keyword" {
    mkdir -p "$(data_root)/versions/local"
    mkdir -p "$(data_root)/versions/0.55.0"
    run list_versions --local
    assert_success
    assert_output --partial "0.55.0"
    # The output has "0.55.0\tlocal" where "local" is the scope, not the version.
    # Verify the "local" VERSION directory is not listed as a version name.
    # Each line is "version\tscope", so check no line starts with "local\t"
    refute_output --regexp "^local	"
}

# --- version_config_root / version_data_root ---

@test "version_config_root: returns correct path for local" {
    run version_config_root "0.55.0" "local"
    assert_success
    assert_output "$HOME/.config/morloc/versions/0.55.0"
}

@test "version_config_root: returns correct path for system" {
    run version_config_root "0.55.0" "system"
    assert_success
    assert_output "/etc/morloc/versions/0.55.0"
}

@test "version_data_root: returns correct path for local" {
    run version_data_root "0.55.0" "local"
    assert_success
    assert_output "$HOME/.local/share/morloc/versions/0.55.0"
}

# --- read_flags_file ---

@test "read_flags_file: strips comments and blank lines" {
    local flagsfile="$HOME/test.flags"
    printf '# comment\n--gpus all\n\n-v /data:/data\n# another comment\n' > "$flagsfile"
    run read_flags_file "$flagsfile"
    assert_success
    assert_line --index 0 "--gpus all"
    assert_line --index 1 "-v /data:/data"
}

@test "read_flags_file: returns nothing for missing file" {
    run read_flags_file "$HOME/nonexistent.flags"
    assert_success
    assert_output ""
}

# --- write_version_config full flow ---

@test "write_version_config: creates full config structure" {
    write_version_config "0.58.3" "local"

    # User config
    assert_file_exists "$(config_root)/config"
    assert_file_contains "$(config_root)/config" "active_version=0.58.3"
    assert_file_contains "$(config_root)/config" "active_scope=local"
    assert_file_contains "$(config_root)/config" "active_env=base"

    # Version config
    local vcfg="$(config_root)/versions/0.58.3/config"
    assert_file_exists "$vcfg"
    assert_file_contains "$vcfg" "image=ghcr.io/morloc-project/morloc/morloc-full:0.58.3"
    assert_file_contains "$vcfg" "dev_image=ghcr.io/morloc-project/morloc/morloc-test:latest"

    # Base environment
    local base="$(config_root)/versions/0.58.3/environments/base.conf"
    assert_file_exists "$base"
    assert_file_contains "$base" "image=ghcr.io/morloc-project/morloc/morloc-full:0.58.3"
}
