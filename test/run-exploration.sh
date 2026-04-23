#!/bin/sh
# VM exploration orchestrator
#
# Runs persona-based agent sessions on a single VM. Each persona SSHs in as
# its own Linux user (provisioned by the Vagrantfile). No state is reset
# between personas -- agents start in a dirty session with whatever artifacts
# remain from past runs.
#
# Context is threaded between agents via a shared known-issues.md file.
# After all personas complete, an analyst agent consolidates findings.
#
# Prerequisites:
#   - vagrant + vagrant-libvirt plugin
#   - claude CLI (Claude Code)
#   - Vagrantfile in the repo root
#   - morloc-manager binary symlinked/copied to repo root

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

ALL_PERSONAS=$(cd "$SCRIPT_DIR/personas" && ls *.md 2>/dev/null | sed 's/\.md$//')
ALL_VMS="fedora ubuntu debian"

# Map persona names to their Linux usernames on the VM
persona_user() {
    case "$1" in
        new-user)       echo "newuser" ;;
        developer)      echo "developer" ;;
        sysadmin)       echo "sysadmin" ;;
        power-user)     echo "poweruser" ;;
        mathematician)  echo "mathematician" ;;
        *)              echo "$1" ;;
    esac
}

usage() {
    cat <<EOF
Usage: $(basename "$0") --vm VM PROMPT_FILE [OPTIONS]

Run autonomous agent-based exploratory testing of morloc-manager on a single VM.

Arguments:
  PROMPT_FILE               File containing the main prompt for explorer agents

Options:
  -h, --help                Show this help message
  --info                    List available personas and VMs
  --vm VM                   VM to test on (required: fedora, ubuntu, or debian)
  --personas LIST           Comma-separated list of personas to run (default: all)
  --fresh                   Remove existing known-issues.md before starting
  --sync                    Rsync files into the VM before starting

Examples:
  $(basename "$0") --vm fedora test/prompts/full-exploration.md
  $(basename "$0") --vm ubuntu test/prompts/freeze-pipeline.md --personas developer,new-user
  $(basename "$0") --vm fedora test/prompts/full-exploration.md --sync
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

PROMPT_FILE=""
PERSONAS=""
VM=""
FRESH=""
SYNC=""

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
        --vm)
            VM="$2"
            shift 2
            ;;
        --personas)
            PERSONAS=$(echo "$2" | tr ',' ' ')
            shift 2
            ;;
        --fresh)
            FRESH=1
            shift
            ;;
        --sync)
            SYNC=1
            shift
            ;;
        -*)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
        *)
            if [ -z "$PROMPT_FILE" ]; then
                PROMPT_FILE="$1"
            else
                echo "ERROR: unexpected argument: $1" >&2
                usage >&2
                exit 1
            fi
            shift
            ;;
    esac
done

if [ -z "$VM" ]; then
    echo "ERROR: --vm is required" >&2
    usage >&2
    exit 1
fi

if [ -z "$PROMPT_FILE" ]; then
    echo "ERROR: PROMPT_FILE is required" >&2
    usage >&2
    exit 1
fi

if [ ! -f "$PROMPT_FILE" ]; then
    echo "ERROR: prompt file not found: $PROMPT_FILE" >&2
    exit 1
fi

MAIN_PROMPT=$(cat "$PROMPT_FILE")

# Validate VM name
case "$VM" in
    fedora|ubuntu|debian) ;;
    *)
        echo "ERROR: unknown VM '$VM'. Must be one of: $ALL_VMS" >&2
        exit 1
        ;;
esac

PERSONAS="${PERSONAS:-$ALL_PERSONAS}"
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
check_short_circuit() {
    [ -f "$KI_FILE" ] || return 0
    _status=$(grep '<!-- STATUS:' "$KI_FILE" | head -1)
    case "$_status" in
        *short-circuit:*)
            _reason=$(echo "$_status" | sed 's/.*short-circuit: *//;s/ *-->.*//')
            log "SHORT-CIRCUIT: $_reason"
            return 1 ;;
    esac
    return 0
}

# Extract SSH connection details from vagrant
extract_ssh_config() {
    _ssh_dir="$FINDINGS_DIR/.ssh"
    mkdir -p "$_ssh_dir"

    _ssh_config=$(vagrant ssh-config "$VM")

    SSH_HOST=$(echo "$_ssh_config" | awk '/HostName/ {print $2}')
    SSH_PORT=$(echo "$_ssh_config" | awk '/Port/ {print $2}')
    _key_path=$(echo "$_ssh_config" | awk '/IdentityFile/ {print $2}')

    SSH_KEY="$_ssh_dir/${VM}_key"
    cp "$_key_path" "$SSH_KEY"
    chmod 600 "$SSH_KEY"
}

# Sync files into the VM if requested
if [ -n "$SYNC" ]; then
    log "Syncing files into $VM"
    vagrant rsync "$VM"
fi

# Verify VM is reachable
if ! vagrant ssh "$VM" -c "echo '$VM ready'" 2>/dev/null; then
    echo "ERROR: $VM not reachable. Start it with: vagrant up $VM" >&2
    exit 1
fi

# Extract SSH config
extract_ssh_config
log "SSH config: vagrant@$SSH_HOST:$SSH_PORT (key: $SSH_KEY)"

for persona in $PERSONAS; do
    log "Persona: $persona on $VM"
    person_dir="$FINDINGS_DIR/$VM/$persona"
    mkdir -p $person_dir

    PERSONA_FILE="test/personas/$persona.md"
    if [ ! -f "$PERSONA_FILE" ]; then
        log "WARNING: persona file $PERSONA_FILE not found, skipping"
        continue
    fi

    # Determine the Linux user for this persona
    PERSONA_LOGIN=$(persona_user "$persona")

    # SSH command templates: vagrant user (for sudo) and persona user (direct)
    SSH_BASE="ssh -i $SSH_KEY -p $SSH_PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
    SSH_VAGRANT="$SSH_BASE vagrant@$SSH_HOST"

    # Read current known-issues to inject into the prompt
    KNOWN_ISSUES=$(cat "$KI_FILE")

    PROMPT="You are exploring morloc-manager on the '$VM' VM as the '$PERSONA_LOGIN' user.

## SSH Access

Run commands as your persona user ($PERSONA_LOGIN):
  $SSH_VAGRANT 'sudo -u $PERSONA_LOGIN <command>'

Run commands as root (for system-wide operations):
  $SSH_VAGRANT 'sudo <command>'

The morloc-manager binary is at /vagrant/morloc-manager inside the VM.

Examples:
  $SSH_VAGRANT 'sudo -u $PERSONA_LOGIN /vagrant/morloc-manager --help'
  $SSH_VAGRANT 'sudo -u $PERSONA_LOGIN bash -c \"cd /home/$PERSONA_LOGIN && /vagrant/morloc-manager new\"'
  $SSH_VAGRANT 'sudo /vagrant/morloc-manager new shared --system'

IMPORTANT — checking exit codes through SSH:
  Use SINGLE quotes around the remote command so that \$? is expanded on the VM, not locally.
  You MUST propagate the exit code with 'exit \$r' so that SSH itself exits non-zero on failure.
  Correct:   $SSH_VAGRANT 'sudo -u $PERSONA_LOGIN /vagrant/morloc-manager foobar; r=\$?; echo exit=\$r; exit \$r'
  WRONG:     $SSH_VAGRANT 'sudo -u $PERSONA_LOGIN /vagrant/morloc-manager foobar; echo exit=\$?'
    (echo always exits 0, so SSH reports success even when the command failed)
  WRONG:     $SSH_VAGRANT \"sudo -u $PERSONA_LOGIN /vagrant/morloc-manager foobar; echo exit=\$?\"
    (\$? inside double quotes is expanded locally to 0, not on the VM)

## Dirty Session

Your user account ($PERSONA_LOGIN) may have artifacts from previous test runs
(environments, config files, containers). This is intentional -- explore what
state exists before creating new things. If old state causes problems, that's
worth noting.

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

1. Explore following your persona's approach and focus areas
2. Write bug reports ONLY for issues NOT already in Known Issues above
   File path: $person_dir/bug-NNN.md
3. At the END of your session, update $KI_FILE:
   - Append new issues you found (use the KI-NNN format, increment from last number)
   - Add your persona/VM to confirmed-by for issues you reproduced
   - Add workarounds you discovered for existing issues
   - Update the UPDATED comment with current timestamp and your identity
4. Write your experience summary to: $person_dir/summary.md
5. Short-circuit: ONLY if the VM is completely unusable (can't SSH, binary crashes
   on every command, no workaround possible), change the STATUS line in $KI_FILE to:
   <!-- STATUS: short-circuit: REASON -->

$EXPLORER_CONTEXT

## Task

$MAIN_PROMPT"

    # Launch agent non-interactively
    claude -p "$PROMPT" \
        --agent vm-explorer \
        --allowedTools "Bash,Read,Write" \
        --no-session-persistence \
        --output-format text \
        < /dev/null 2>&1 | tee "$person_dir/session.log"

    log "Done: $persona on $VM"

    # Check for short-circuit after each agent
    if ! check_short_circuit; then
        log "Short-circuit triggered. Skipping remaining personas."
        break
    fi
done

log "Personas done. Running analyst agent"
claude -p "Fold across all findings to produce a consolidated action plan and report.

You have two key inputs:

1. **findings/known-issues.md** — a pre-deduplicated list of known issues accumulated
   across all agent sessions. Each entry has severity, scope, workaround, and cross-session
   confirmation data. Use this as your STARTING POINT for the action plan: convert each
   KI entry into an RC entry.

2. **Individual bug reports** (findings/*/*/bug-*.md) — these contain additional detail,
   reproduction steps, or edge cases not captured in known-issues.md. Fold each into the
   action plan, matching against existing root causes or adding new ones.

You also have read-only access to the morloc compiler source (morloc/) and documentation
(morloc-project.github.io/) via symlinks. Use these to validate bugs against the actual
source code, estimate fix difficulty, explore solutions, and report doc discrepancies.
NEVER modify files in either directory.

Initialize findings/action-plan.md from the known issues, then process each individual
bug report to enrich or extend it. Validate each root cause against the source code.
The result should be a single consolidated action plan grouped by root cause with
difficulty estimates and proposed solutions, not a per-report analysis.

After the action plan is complete, produce the report. Glob all usage summaries
(findings/*/*/summary.md) and the completed action plan, then write findings/report.md —
a consolidated report with an abstract, detailed findings, documentation discrepancies,
and prioritized action items with difficulty estimates.

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
log "Report: $FINDINGS_DIR/report.md"
log "Known issues: $FINDINGS_DIR/known-issues.md"
