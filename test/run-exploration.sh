#!/bin/sh
# Overnight VM exploration orchestrator
#
# Runs persona-based agent sessions sequentially, one VM at a time.
# Morloc state is reset between personas; container images stay cached.
# Context is threaded between agents via a shared known-issues.md file.
# Designed for unattended overnight runs (12+ hours).
#
# Prerequisites:
#   - vagrant + vagrant-libvirt plugin
#   - claude CLI (Claude Code)
#   - Vagrantfile in the repo root
#   - morloc-manager binary in the repo root

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

ALL_PERSONAS=$(cd "$SCRIPT_DIR/personas" && ls *.md 2>/dev/null | sed 's/\.md$//')
ALL_VMS="fedora ubuntu debian"

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Run autonomous agent-based exploratory testing of morloc-manager across VMs.

Options:
  -h, --help                Show this help message
  --info                    List available personas and VMs
  -f, --focus TEXT          Additional instructions to focus explorer agents
                            (e.g., --focus "focus on the build/freeze/serve lifecycle")
  --personas LIST           Comma-separated list of personas to run
                            (default: all)
  --vms LIST                Comma-separated list of VMs to run
                            (default: all)
  --fresh                   Remove existing known-issues.md before starting
                            (default: resume from existing file)
  --no-destroy              Keep VMs alive after exploration finishes
  --no-create               Skip vagrant up (assume VMs are already running)
  --persistent              Shorthand for --no-destroy --no-create

Examples:
  $(basename "$0")
  $(basename "$0") --vms fedora,ubuntu --personas developer
  $(basename "$0") -f "test the build/freeze/serve deployment pipeline"
EOF
}

show_info() {
    echo "Available personas:"
    for p in $ALL_PERSONAS; do
        _desc=""
        _file="$SCRIPT_DIR/personas/$p.md"
        if [ -f "$_file" ]; then
            _desc=$(head -5 "$_file" | sed -n 's/^#[[:space:]]*//p' | head -1)
        fi
        if [ -n "$_desc" ]; then
            printf "  %-14s %s\n" "$p" "$_desc"
        else
            printf "  %s\n" "$p"
        fi
    done
    echo ""
    echo "Available VMs:"
    for v in $ALL_VMS; do
        printf "  %s\n" "$v"
    done
}

FOCUS=""
PERSONAS=""
VMS=""
FRESH=""
NO_DESTROY=""
NO_CREATE=""

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        --info)
            show_info
            exit 0
            ;;
        -f|--focus)
            FOCUS="$2"
            shift 2
            ;;
        --personas)
            PERSONAS=$(echo "$2" | tr ',' ' ')
            shift 2
            ;;
        --vms)
            VMS=$(echo "$2" | tr ',' ' ')
            shift 2
            ;;
        --fresh)
            FRESH=1
            shift
            ;;
        --no-destroy)
            NO_DESTROY=1
            shift
            ;;
        --no-create)
            NO_CREATE=1
            shift
            ;;
        --persistent)
            NO_DESTROY=1
            NO_CREATE=1
            shift
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

PERSONAS="${PERSONAS:-$ALL_PERSONAS}"
VMS="${VMS:-$ALL_VMS}"
FINDINGS_DIR="findings"

cd "$REPO_DIR"

# Verify the binary exists
if [ ! -x "morloc-manager" ]; then
    echo "ERROR: morloc-manager binary not found or not executable in $REPO_DIR" >&2
    echo "Copy the compiled binary from the compiler repo first." >&2
    exit 1
fi

mkdir -p "$FINDINGS_DIR"

# Read customizable context files
EXPLORER_CONTEXT=""
if [ -f "$SCRIPT_DIR/explorer-context.md" ]; then
    EXPLORER_CONTEXT=$(cat "$SCRIPT_DIR/explorer-context.md")
fi
ANALYST_CONTEXT=""
if [ -f "$SCRIPT_DIR/analyst-context.md" ]; then
    ANALYST_CONTEXT=$(cat "$SCRIPT_DIR/analyst-context.md")
fi

log() {
    echo "=== $(date '+%Y-%m-%d %H:%M:%S') $* ==="
}

# Initialize or preserve the shared known-issues file.
# This file threads context between sequential agents -- each agent reads it
# at session start (injected via prompt) and updates it at session end.
KI_FILE="$FINDINGS_DIR/known-issues.md"
if [ -n "$FRESH" ] && [ -f "$KI_FILE" ]; then
    rm "$KI_FILE"
fi
if [ ! -f "$KI_FILE" ]; then
    cat > "$KI_FILE" <<'KIEOF'
# Known Issues

<!-- STATUS: ok -->
<!-- UPDATED: never -->
KIEOF
    log "Initialized $KI_FILE"
fi

# Check the STATUS comment in known-issues.md for short-circuit signals.
# Returns: 0 = continue, 2 = skip this VM, 3 = abort entire run
check_short_circuit() {
    [ -f "$KI_FILE" ] || return 0
    _status=$(grep '<!-- STATUS:' "$KI_FILE" | head -1)
    case "$_status" in
        *short-circuit-vm:*)
            _reason=$(echo "$_status" | sed 's/.*short-circuit-vm: *//;s/ *-->.*//')
            log "VM SHORT-CIRCUIT: $_reason"
            return 2 ;;
        *short-circuit:*)
            _reason=$(echo "$_status" | sed 's/.*short-circuit: *//;s/ *-->.*//')
            log "GLOBAL SHORT-CIRCUIT: $_reason"
            return 3 ;;
    esac
    return 0
}

# Reset a VM-level short-circuit back to ok so the next VM can proceed
reset_vm_short_circuit() {
    if [ -f "$KI_FILE" ]; then
        sed -i 's/<!-- STATUS: short-circuit-vm:.*-->/<!-- STATUS: ok -->/' "$KI_FILE"
    fi
}

# Extract SSH connection details from vagrant and write them to files
# in the workspace (which is mounted into the claude container).
# Returns: sets SSH_HOST, SSH_PORT, SSH_USER, SSH_KEY variables
extract_ssh_config() {
    _vm="$1"
    _ssh_dir="$FINDINGS_DIR/.ssh"
    mkdir -p "$_ssh_dir"

    # Get SSH config from vagrant
    _ssh_config=$(vagrant ssh-config "$_vm")

    SSH_HOST=$(echo "$_ssh_config" | awk '/HostName/ {print $2}')
    SSH_PORT=$(echo "$_ssh_config" | awk '/Port/ {print $2}')
    SSH_USER=$(echo "$_ssh_config" | awk '/User / {print $2}')
    _key_path=$(echo "$_ssh_config" | awk '/IdentityFile/ {print $2}')

    # Copy the SSH key into the workspace so the container can access it
    SSH_KEY="$_ssh_dir/${_vm}_key"
    cp "$_key_path" "$SSH_KEY"
    chmod 600 "$SSH_KEY"
}

GLOBAL_ABORT=""

for vm in $VMS; do
    if [ -n "$GLOBAL_ABORT" ]; then
        break
    fi

    if [ -z "$NO_CREATE" ]; then
        log "Starting VM: $vm"
        vagrant up "$vm"
    else
        log "Using existing VM: $vm"
        vagrant rsync "$vm"
    fi

    # Wait for VM to be ready
    if ! vagrant ssh "$vm" -c "echo '$vm ready'" 2>/dev/null; then
        log "FAIL: $vm not reachable, skipping"
        continue
    fi

    # Extract SSH config for this VM (so containerized claude can reach it)
    extract_ssh_config "$vm"
    log "SSH config: $SSH_USER@$SSH_HOST:$SSH_PORT (key: $SSH_KEY)"

    for persona in $PERSONAS; do
        log "Persona: $persona on $vm"
        mkdir -p "$FINDINGS_DIR/$vm/$persona"

        # Ensure latest binary is synced into the VM
        vagrant rsync "$vm"

        # Reset morloc state without touching cached container images
        log "Resetting VM state for $persona"
        vagrant ssh "$vm" -c "
            sudo rm -rf /etc/morloc /usr/local/share/morloc
            sudo rm -rf ~/.config/morloc ~/.local/share/morloc ~/.local/bin/morloc-manager
            if id testuser &>/dev/null; then
                sudo userdel -r testuser
            fi
            sudo useradd -m -s /bin/bash testuser
            grep -q testuser /etc/subuid || echo 'testuser:100000:65536' | sudo tee -a /etc/subuid
            grep -q testuser /etc/subgid || echo 'testuser:100000:65536' | sudo tee -a /etc/subgid
            sudo loginctl enable-linger testuser || true
        " 2>/dev/null

        # Build the prompt with persona context
        PERSONA_FILE="test/personas/$persona.md"
        if [ ! -f "$PERSONA_FILE" ]; then
            log "WARNING: persona file $PERSONA_FILE not found, skipping"
            continue
        fi

        SSH_CMD="ssh -i $SSH_KEY -p $SSH_PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR $SSH_USER@$SSH_HOST"

        # Read current known-issues to inject into the prompt
        KNOWN_ISSUES=$(cat "$KI_FILE")

        PROMPT="You are exploring morloc-manager on the '$vm' VM.

## SSH Access

SSH into the VM with: $SSH_CMD '<your command>'
The morloc-manager binary is at /vagrant/morloc-manager inside the VM.
Run it as: $SSH_CMD '/vagrant/morloc-manager <subcommand>'
For sudo commands: $SSH_CMD 'sudo /vagrant/morloc-manager <subcommand>'
For testuser commands: $SSH_CMD 'sudo -u testuser /vagrant/morloc-manager <subcommand>'
For multi-step workflows inside one container session: $SSH_CMD 'cd ~/myproject && /vagrant/morloc-manager run -- bash -c \"morloc init && morloc make foo.loc\"'

IMPORTANT — checking exit codes through SSH:
  Use SINGLE quotes around the remote command so that \$? is expanded on the VM, not locally.
  Correct:   $SSH_CMD '/vagrant/morloc-manager foobar; echo exit=\$?'
  WRONG:     $SSH_CMD \"/vagrant/morloc-manager foobar; echo exit=\$?\"
  The wrong form expands \$? to 0 locally before SSH sends the command.

## Your Persona: $persona

$(cat "$PERSONA_FILE")

## Known Issues from Previous Sessions

The following issues have already been discovered by previous agents.
DO NOT re-report these as bug files. Instead:
- Use the listed workarounds to get past blockers
- Confirm whether each issue reproduces on this VM/engine (note in your summary)
- Explore BEYOND these known issues — your value is finding NEW things

$KNOWN_ISSUES

## Your Responsibilities

1. Explore following your persona goals, using workarounds for known blockers
2. Write bug reports ONLY for issues NOT already in Known Issues above
   File path: $FINDINGS_DIR/$vm/$persona/bug-NNN.md
3. At the END of your session, update $KI_FILE:
   - Append new issues you found (use the KI-NNN format, increment from last number)
   - Add your persona/VM to confirmed-by for issues you reproduced
   - Add workarounds you discovered for existing issues
   - Update the UPDATED comment with current timestamp and your identity
4. Write your experience summary to: $FINDINGS_DIR/$vm/$persona/summary.md
5. Short-circuit: ONLY if the VM is completely unusable (can't SSH, binary crashes
   on every command, no workaround possible), change the STATUS line in $KI_FILE to:
   <!-- STATUS: short-circuit-vm: REASON -->

$EXPLORER_CONTEXT${FOCUS:+

FOCUS: $FOCUS}"

        # Launch agent non-interactively
        claude -p "$PROMPT" \
            --agent vm-explorer \
            --allowedTools "Bash,Read,Write" \
            --no-session-persistence \
            --output-format text \
            < /dev/null 2>&1 | tee "$FINDINGS_DIR/$vm/$persona/session.log"

        log "Done: $persona on $vm"

        # Check for short-circuit after each agent
        _sc=0
        check_short_circuit || _sc=$?
        if [ $_sc -eq 3 ]; then
            log "Global short-circuit triggered. Skipping remaining sessions."
            GLOBAL_ABORT=1
            break
        elif [ $_sc -eq 2 ]; then
            log "VM short-circuit for $vm. Moving to next VM."
            reset_vm_short_circuit
            break
        fi
    done

    if [ -z "$NO_DESTROY" ]; then
        log "Destroying VM: $vm"
        vagrant destroy -f "$vm"
    else
        log "Keeping VM: $vm (--no-destroy)"
    fi
done

log "All VMs done. Running analyst agent"
claude -p "Fold across all findings to produce a consolidated action plan and UX report.

You have two key inputs:

1. **findings/known-issues.md** — a pre-deduplicated list of known issues accumulated
   across all agent sessions. Each entry has severity, scope, workaround, and cross-session
   confirmation data. Use this as your STARTING POINT for the action plan: convert each
   KI entry into an RC entry.

2. **Individual bug reports** (findings/*/*/bug-*.md) — these contain additional detail,
   reproduction steps, or edge cases not captured in known-issues.md. Fold each into the
   action plan, matching against existing root causes or adding new ones.

Initialize findings/action-plan.md from the known issues, then process each individual
bug report to enrich or extend it. The result should be a single consolidated action plan
grouped by root cause, not a per-report analysis.

After the action plan is complete, produce a UX report. Glob all usage summaries
(findings/*/*/summary.md), fold them into findings/ux-report.md — a consolidated
narrative of user experience across personas and VMs.

$ANALYST_CONTEXT" \
    --agent vm-analyst \
    --allowedTools "Read,Write,Glob,Grep" \
    --no-session-persistence \
    --output-format text \
    < /dev/null 2>&1 | tee "$FINDINGS_DIR/analyst-session.log"

# Clean up SSH keys
rm -rf "$FINDINGS_DIR/.ssh"

log "Exploration complete. Results in $FINDINGS_DIR/"
log "Action plan: $FINDINGS_DIR/action-plan.md"
log "UX report: $FINDINGS_DIR/ux-report.md"
log "Known issues: $FINDINGS_DIR/known-issues.md"
