#!/usr/bin/env bash
# Mock container engine helpers for unit tests
# These create fake docker/podman commands so tests don't need a real engine

# Set up a mock container engine in a temp bin directory and prepend to PATH.
# Sets MOCK_ENGINE_DIR. Must NOT be called in a subshell (no $() capture).
# Usage: setup_mock_engine "docker" [version]
setup_mock_engine() {
    local engine_name="$1"
    local engine_version="${2:-24.0.7}"

    MOCK_ENGINE_DIR="$(mktemp -d "${BATS_TMPDIR:-/tmp}/mock-engine.XXXXXX")"

    cat > "$MOCK_ENGINE_DIR/$engine_name" << MOCK_EOF
#!/bin/sh
case "\$1" in
    --version)
        echo "$engine_name version $engine_version"
        ;;
    run)
        # Simulate running a container
        shift
        # Find the command after all flags
        while [ \$# -gt 0 ]; do
            case "\$1" in
                --*) shift; [ \$# -gt 0 ] && shift ;;
                -*)  shift; [ \$# -gt 0 ] && shift ;;
                *)   break ;;
            esac
        done
        echo "mock-run: \$*"
        ;;
    pull)
        echo "mock-pull: \$2"
        ;;
    build)
        echo "mock-build: \$*"
        ;;
    images)
        echo "mock-images"
        ;;
    image)
        case "\$2" in
            inspect)
                echo '{"Created": "2024-01-01T00:00:00Z"}'
                ;;
        esac
        ;;
    ps)
        # Return empty (no containers)
        ;;
    rm|rmi)
        echo "mock-\$1: \$*"
        ;;
    *)
        echo "mock-unknown: \$*"
        ;;
esac
exit 0
MOCK_EOF

    chmod +x "$MOCK_ENGINE_DIR/$engine_name"
    export PATH="$MOCK_ENGINE_DIR:$PATH"
}

# Remove a mock engine from PATH
teardown_mock_engine() {
    if [ -n "${MOCK_ENGINE_DIR:-}" ]; then
        rm -rf "$MOCK_ENGINE_DIR"
        export PATH="${PATH#$MOCK_ENGINE_DIR:}"
        unset MOCK_ENGINE_DIR
    fi
}

# Create a mock engine that fails specific commands
# Sets MOCK_ENGINE_DIR and prepends to PATH.
# Usage: setup_failing_mock_engine "docker" "pull"
setup_failing_mock_engine() {
    local engine_name="$1"
    local fail_command="$2"

    MOCK_ENGINE_DIR="$(mktemp -d "${BATS_TMPDIR:-/tmp}/mock-engine.XXXXXX")"

    cat > "$MOCK_ENGINE_DIR/$engine_name" << MOCK_EOF
#!/bin/sh
case "\$1" in
    --version)
        echo "$engine_name version 24.0.7"
        ;;
    $fail_command)
        echo "Error: $fail_command failed" >&2
        exit 1
        ;;
    *)
        exit 0
        ;;
esac
MOCK_EOF

    chmod +x "$MOCK_ENGINE_DIR/$engine_name"
    export PATH="$MOCK_ENGINE_DIR:$PATH"
}
