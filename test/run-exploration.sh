#!/bin/sh
# Overnight VM exploration orchestrator
#
# Runs persona-based agent sessions sequentially, one VM at a time.
# Morloc state is reset between personas; container images stay cached.
# Designed for unattended overnight runs (12+ hours).
#
# Prerequisites:
#   - vagrant + vagrant-libvirt plugin
#   - claude CLI (Claude Code)
#   - Vagrantfile in the repo root

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
                            (e.g., --focus "focus on the new 'shell' subcommand")
  --personas LIST           Comma-separated list of personas to run
                            (default: all)
  --vms LIST                Comma-separated list of VMs to run
                            (default: all)

Examples:
  $(basename "$0")
  $(basename "$0") --vms fedora,ubuntu --personas developer
  $(basename "$0") -f "test error handling in 'env' subcommand"
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

for vm in $VMS; do
    log "Starting VM: $vm"
    vagrant up "$vm"

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

        # Reset morloc state without touching cached container images
        log "Resetting VM state for $persona"
        vagrant ssh "$vm" -c "
            sudo rm -rf /etc/morloc /usr/local/share/morloc
            rm -rf ~/.config/morloc ~/.local/share/morloc ~/.local/bin/morloc-manager
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

        PROMPT="You are exploring morloc-manager on the '$vm' VM.

SSH into the VM with: $SSH_CMD \"<your command>\"
The morloc-manager script is at /vagrant/morloc-manager.sh inside the VM.
Run it as: $SSH_CMD \"cd /vagrant && bash morloc-manager.sh <subcommand>\"
For sudo commands: $SSH_CMD \"cd /vagrant && sudo bash morloc-manager.sh <subcommand>\"
For testuser commands: $SSH_CMD \"sudo -u testuser bash -c 'cd /vagrant && bash morloc-manager.sh <subcommand>'\"

Your persona: $persona
Write bug reports to: $FINDINGS_DIR/$vm/$persona/bug-NNN.md

$(cat "$PERSONA_FILE")

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
    done

    log "Destroying VM: $vm"
    vagrant destroy -f "$vm"
done

log "All VMs done. Running analyst agent"
claude -p "Fold across all bug reports in findings/. Initialize findings/action-plan.md, then process each bug report one at a time: compare it to existing root causes in the action plan, either merge it into an existing root cause or add a new one. The result should be a single consolidated action plan grouped by root cause, not a per-report analysis.

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
