#!/bin/bash
# Run the full Tier 2 test suite across all VM configurations
#
# Usage:
#   ./test/run-vm-tests.sh                  # Run all VMs
#   ./test/run-vm-tests.sh fedora           # Run specific VM(s)
#   ./test/run-vm-tests.sh --no-destroy     # Keep VMs running after tests
#
# Prerequisites:
#   - vagrant with libvirt provider
#   - KVM/libvirt installed and running

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_DIR"

ALL_VMS=(fedora ubuntu debian)
ENGINES=(docker podman)
NO_DESTROY=false

# Parse flags
VMS=()
for arg in "$@"; do
    case "$arg" in
        --no-destroy) NO_DESTROY=true ;;
        *)            VMS+=("$arg") ;;
    esac
done

# Default to all VMs if none specified
if [ ${#VMS[@]} -eq 0 ]; then
    VMS=("${ALL_VMS[@]}")
fi

# Counters
TOTAL=0
PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0

# Result tracking
declare -a RESULTS=()

log_result() {
    local vm="$1" engine="$2" mode="$3" suite="$4" result="$5"
    local line
    line=$(printf "[%-7s] [%-7s] [%-9s] [%-15s] %s" "$vm" "$engine" "$mode" "$suite" "$result")
    RESULTS+=("$line")
    TOTAL=$((TOTAL + 1))
    case "$result" in
        PASS) PASS_COUNT=$((PASS_COUNT + 1)) ;;
        FAIL) FAIL_COUNT=$((FAIL_COUNT + 1)) ;;
        SKIP) SKIP_COUNT=$((SKIP_COUNT + 1)) ;;
    esac
}

run_in_vm() {
    local vm="$1"
    shift
    vagrant ssh "$vm" -c "cd /vagrant && $*" 2>&1
}

echo "=== Morloc Manager VM Test Suite ==="
echo "VMs to test: ${VMS[*]}"
echo "Engines: ${ENGINES[*]}"
echo ""

# Start VMs in parallel
echo "--- Starting VMs ---"
vagrant up --parallel "${VMS[@]}" || {
    echo "WARNING: Some VMs may have failed to start"
}
echo ""

for vm in "${VMS[@]}"; do
    echo ""
    echo "=========================================="
    echo "  VM: $vm"
    echo "=========================================="

    # Check if VM is running
    if ! vagrant status "$vm" 2>/dev/null | grep -q "running"; then
        echo "SKIP: $vm is not running"
        log_result "$vm" "-" "-" "all" "SKIP"
        continue
    fi

    # Sync files
    vagrant rsync "$vm" 2>/dev/null || true

    # --- Step 1: Unit + Integration (sanity check) ---
    echo ""
    echo "--- [$vm] unit + integration ---"
    if run_in_vm "$vm" "bats test/unit/ test/integration/"; then
        log_result "$vm" "-" "-" "unit+integration" "PASS"
    else
        log_result "$vm" "-" "-" "unit+integration" "FAIL"
        echo "FAIL: $vm unit+integration failed, skipping remaining tests"
        continue
    fi

    # --- Step 2: VM-specific kernel tests ---
    echo ""
    echo "--- [$vm] vm-specific tests ---"
    if run_in_vm "$vm" "bats test/vm/"; then
        log_result "$vm" "-" "-" "vm-specific" "PASS"
    else
        log_result "$vm" "-" "-" "vm-specific" "FAIL"
    fi

    # --- Step 3: E2E per engine (rootless) ---
    for engine in "${ENGINES[@]}"; do
        echo ""
        echo "--- [$vm] [$engine] [rootless] e2e ---"
        if run_in_vm "$vm" "MORLOC_CONTAINER_ENGINE=$engine bats test/e2e/"; then
            log_result "$vm" "$engine" "rootless" "e2e" "PASS"
        else
            # Check if the engine is available at all
            if run_in_vm "$vm" "command -v $engine >/dev/null 2>&1 && $engine info >/dev/null 2>&1"; then
                log_result "$vm" "$engine" "rootless" "e2e" "FAIL"
            else
                log_result "$vm" "$engine" "rootless" "e2e" "SKIP"
            fi
        fi
    done

    # --- Step 4: Rootful (future) ---
    for engine in "${ENGINES[@]}"; do
        log_result "$vm" "$engine" "rootful" "e2e" "SKIP"
    done
done

# Summary
echo ""
echo "=========================================="
echo "  Summary"
echo "=========================================="
echo ""
printf "%-60s %s\n" "Test" "Result"
printf "%-60s %s\n" "----" "------"
for line in "${RESULTS[@]}"; do
    echo "$line"
done
echo ""
echo "Total: $TOTAL  Pass: $PASS_COUNT  Fail: $FAIL_COUNT  Skip: $SKIP_COUNT"

# Clean up
if [ "$NO_DESTROY" = true ]; then
    echo ""
    echo "VMs left running (--no-destroy). Use 'vagrant destroy -f' to clean up."
else
    echo ""
    read -rp "Destroy VMs? [y/N]: " response
    case "$response" in
        [yY]|[yY][eE][sS])
            vagrant destroy -f
            ;;
        *)
            echo "VMs left running. Use 'vagrant destroy -f' to clean up."
            ;;
    esac
fi

if [ "$FAIL_COUNT" -gt 0 ]; then
    exit 1
fi
