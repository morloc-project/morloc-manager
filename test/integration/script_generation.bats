#!/usr/bin/env bats
# Integration tests for generated wrapper scripts

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

# --- menv script content ---

@test "script_generation: menv has correct shebang" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    local first_line
    first_line=$(head -n1 "$MORLOC_BIN/menv")
    [ "$first_line" = "#!/usr/bin/env sh" ]
}

@test "script_generation: menv has --rm flag" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" "--rm"
}

@test "script_generation: menv mounts PWD to work directory" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" 'PWD'
}

@test "script_generation: menv sets working directory" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" 'work'
}

@test "script_generation: menv passes through arguments" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" '"$@"'
}

@test "script_generation: menv uses correct container image" {
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" "ghcr.io/morloc-project/morloc/morloc-full"
}

# --- morloc-shell script content ---

@test "script_generation: morloc-shell has -it for interactive" {
    script_morloc_shell "$MORLOC_BIN/morloc-shell" "0.55.0"
    assert_file_contains "$MORLOC_BIN/morloc-shell" "rm -it"
}

@test "script_generation: morloc-shell ends with /bin/bash" {
    script_morloc_shell "$MORLOC_BIN/morloc-shell" "0.55.0"
    assert_file_contains "$MORLOC_BIN/morloc-shell" "/bin/bash"
}

@test "script_generation: morloc-shell does NOT pass through args" {
    script_morloc_shell "$MORLOC_BIN/morloc-shell" "0.55.0"
    assert_file_not_contains "$MORLOC_BIN/morloc-shell" '"$@"'
}

@test "script_generation: morloc-shell sets PATH with ghcup" {
    script_morloc_shell "$MORLOC_BIN/morloc-shell" "0.55.0"
    assert_file_contains "$MORLOC_BIN/morloc-shell" "ghcup"
}

# --- menv-dev script content ---

@test "script_generation: menv-dev uses test container" {
    script_menv_dev "$MORLOC_BIN/menv-dev"
    assert_file_contains "$MORLOC_BIN/menv-dev" "morloc-test"
}

@test "script_generation: menv-dev mounts .stack directory" {
    script_menv_dev "$MORLOC_BIN/menv-dev"
    assert_file_contains "$MORLOC_BIN/menv-dev" ".stack"
}

@test "script_generation: menv-dev creates needed directories" {
    script_menv_dev "$MORLOC_BIN/menv-dev"
    assert_file_contains "$MORLOC_BIN/menv-dev" "mkdir -p"
}

# --- morloc-shell-dev script content ---

@test "script_generation: morloc-shell-dev has -it flag" {
    script_morloc_dev_shell "$MORLOC_BIN/morloc-shell-dev"
    assert_file_contains "$MORLOC_BIN/morloc-shell-dev" "-it"
}

@test "script_generation: morloc-shell-dev ends with /bin/bash" {
    script_morloc_dev_shell "$MORLOC_BIN/morloc-shell-dev"
    assert_file_contains "$MORLOC_BIN/morloc-shell-dev" "/bin/bash"
}

# --- environment variant scripts ---

@test "script_generation: menv with env uses custom container tag" {
    mkdir -p "$HOME/.local/share/morloc/deps"
    cat > "$HOME/.local/share/morloc/deps/ml.Dockerfile" << 'EOF'
ARG CONTAINER_BASE
FROM ${CONTAINER_BASE}
RUN pip install numpy
EOF
    script_menv "$MORLOC_BIN/menv" "0.55.0" "ml" "$HOME/.local/share/morloc/deps/ml.Dockerfile"
    assert_file_contains "$MORLOC_BIN/menv" "morloc-env:0.55.0-ml"
}

@test "script_generation: podman scripts use podman run" {
    teardown_mock_engine
    setup_mock_engine "podman" "4.7.2"
    CONTAINER_ENGINE="podman"
    script_menv "$MORLOC_BIN/menv" "0.55.0"
    assert_file_contains "$MORLOC_BIN/menv" "podman run"
    assert_file_not_contains "$MORLOC_BIN/menv" "docker run"
}
