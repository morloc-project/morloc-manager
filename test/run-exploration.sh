#!/bin/sh
# VM exploration orchestrator.
#
# Pure orchestration. All agent context lives in:
#   test/explorer-context.md   -- briefing for the persona testers
#   test/analyst-context.md    -- briefing for the analyst
# This script only assembles paths and parameters; it does not contain prose
# instructions for the agents.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
FINDINGS_DIR="findings"

# Defaults applied when no override is passed.
DEFAULT_MODEL="sonnet"
DEFAULT_EXPLORER_MAX_TURNS=50
DEFAULT_ANALYST_MAX_TURNS=80

usage() {
    cat <<EOF
Usage: $(basename "$0") --vm VM PROMPT_FILE [OPTIONS]

Run autonomous agent-based exploratory testing on a single VM.

Arguments:
  PROMPT_FILE        File containing the task prompt for the testers.

Options:
  -h, --help                 Show this help message.
  --info                     List available personas and VMs.
  --vm VM                    VM to test on (required).
  --personas LIST            Comma-separated personas (default: all).
  --fresh                    Remove findings/log.md before starting.
  --sync                     Rsync files into the VM before starting.
  --model MODEL              Model for both agents (default: $DEFAULT_MODEL).
  --explorer-model MODEL     Override model for explorer only.
  --analyst-model MODEL      Override model for analyst only.
  --max-turns N              Max turns for both agents.
  --explorer-max-turns N     Override max turns for explorer
                             (default: $DEFAULT_EXPLORER_MAX_TURNS).
  --analyst-max-turns N      Override max turns for analyst
                             (default: $DEFAULT_ANALYST_MAX_TURNS).

Per-agent flags take precedence over --model / --max-turns.
EOF
}

list_personas() {
    (cd "$SCRIPT_DIR/personas" && ls *.md 2>/dev/null | sed 's/\.md$//')
}

list_vms() {
    awk '/config.vm.define/ { gsub(/[",]/, "", $2); print $2 }' "$REPO_DIR/Vagrantfile"
}

show_info() {
    echo "Available personas:"
    for p in $(list_personas); do
        printf "  %s\n" "$p"
    done
    echo ""
    echo "Available VMs:"
    for v in $(list_vms); do
        printf "  %s\n" "$v"
    done
}

PROMPT_FILE=""
PERSONAS=""
VM=""
FRESH=""
SYNC=""
MODEL=""
EXPLORER_MODEL=""
ANALYST_MODEL=""
MAX_TURNS=""
EXPLORER_MAX_TURNS=""
ANALYST_MAX_TURNS=""

while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage; exit 0 ;;
        --info) show_info; exit 0 ;;
        --vm) VM="$2"; shift 2 ;;
        --personas) PERSONAS=$(echo "$2" | tr ',' ' '); shift 2 ;;
        --fresh) FRESH=1; shift ;;
        --sync) SYNC=1; shift ;;
        --model) MODEL="$2"; shift 2 ;;
        --explorer-model) EXPLORER_MODEL="$2"; shift 2 ;;
        --analyst-model) ANALYST_MODEL="$2"; shift 2 ;;
        --max-turns) MAX_TURNS="$2"; shift 2 ;;
        --explorer-max-turns) EXPLORER_MAX_TURNS="$2"; shift 2 ;;
        --analyst-max-turns) ANALYST_MAX_TURNS="$2"; shift 2 ;;
        -*) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
        *)
            if [ -z "$PROMPT_FILE" ]; then PROMPT_FILE="$1"
            else echo "ERROR: unexpected argument: $1" >&2; usage >&2; exit 1
            fi
            shift ;;
    esac
done

# Resolve per-agent settings. Precedence: per-agent flag > shared flag > default.
EXPLORER_MODEL="${EXPLORER_MODEL:-${MODEL:-$DEFAULT_MODEL}}"
ANALYST_MODEL="${ANALYST_MODEL:-${MODEL:-$DEFAULT_MODEL}}"
EXPLORER_MAX_TURNS="${EXPLORER_MAX_TURNS:-${MAX_TURNS:-$DEFAULT_EXPLORER_MAX_TURNS}}"
ANALYST_MAX_TURNS="${ANALYST_MAX_TURNS:-${MAX_TURNS:-$DEFAULT_ANALYST_MAX_TURNS}}"

[ -n "$VM" ] || { echo "ERROR: --vm is required" >&2; usage >&2; exit 1; }
[ -n "$PROMPT_FILE" ] || { echo "ERROR: PROMPT_FILE is required" >&2; usage >&2; exit 1; }
[ -f "$PROMPT_FILE" ] || { echo "ERROR: prompt file not found: $PROMPT_FILE" >&2; exit 1; }

PERSONAS="${PERSONAS:-$(list_personas)}"

cd "$REPO_DIR"

[ -x "morloc-manager" ] || {
    echo "ERROR: morloc-manager binary not found or not executable in $REPO_DIR" >&2
    exit 1
}

mkdir -p "$FINDINGS_DIR"
rm -f "$FINDINGS_DIR/HALT"
if [ -n "$FRESH" ]; then rm -f "$FINDINGS_DIR/log.md"; fi
[ -f "$FINDINGS_DIR/log.md" ] || : > "$FINDINGS_DIR/log.md"

log() { echo "=== $(date '+%Y-%m-%d %H:%M:%S') $* ==="; }

[ -n "$SYNC" ] && { log "Syncing files into $VM"; vagrant rsync "$VM"; }

vagrant ssh "$VM" -c "echo '$VM ready'" >/dev/null 2>&1 || {
    echo "ERROR: $VM not reachable. Start it with: vagrant up $VM" >&2
    exit 1
}

# Extract SSH config so the agent can run ssh directly.
SSH_DIR="$FINDINGS_DIR/.ssh"
mkdir -p "$SSH_DIR"
SSH_CONFIG=$(vagrant ssh-config "$VM")
SSH_HOST=$(echo "$SSH_CONFIG" | awk '/HostName/ {print $2}')
SSH_PORT=$(echo "$SSH_CONFIG" | awk '/Port/ {print $2}')
SSH_KEY="$SSH_DIR/${VM}_key"
cp "$(echo "$SSH_CONFIG" | awk '/IdentityFile/ {print $2}')" "$SSH_KEY"
chmod 600 "$SSH_KEY"
SSH_BASE="ssh -i $SSH_KEY -p $SSH_PORT -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR"
SSH_VAGRANT="$SSH_BASE vagrant@$SSH_HOST"
log "SSH: $SSH_VAGRANT"

# Idempotent runtime provisioning of a persona user on the VM. Creates the
# user, sets up subuid/subgid ranges for rootless containers, enables linger,
# adds them to the docker group, and copies vagrant's authorized_keys so the
# explorer can SSH directly as the persona.
ensure_persona_user() {
    _user="$1"
    vagrant ssh "$VM" -c "sudo bash -s -- '$_user'" <<'EOF' >/dev/null 2>&1
user="$1"
if ! id "$user" >/dev/null 2>&1; then
    useradd -m -s /bin/bash "$user"
    # Allocate next free subuid/subgid range above existing entries.
    start=$(awk -F: 'BEGIN{m=100000} {if($2+$3>m) m=$2+$3} END{print m}' /etc/subuid)
    echo "$user:$start:65536" >> /etc/subuid
    echo "$user:$start:65536" >> /etc/subgid
    loginctl enable-linger "$user" 2>/dev/null || true
    usermod -aG docker "$user" 2>/dev/null || true
fi
# Always (re)install authorized_keys so SSH-as-persona works even if the
# user existed but had no key set up yet.
home="/home/$user"
install -d -o "$user" -g "$user" -m 700 "$home/.ssh"
install -o "$user" -g "$user" -m 600 \
    /home/vagrant/.ssh/authorized_keys "$home/.ssh/authorized_keys"
EOF
}

EXPLORER_CONTEXT=$(cat "$SCRIPT_DIR/explorer-context.md")
ANALYST_CONTEXT=$(cat "$SCRIPT_DIR/analyst-context.md")
TASK=$(cat "$PROMPT_FILE")

for persona in $PERSONAS; do
    PERSONA_FILE="$SCRIPT_DIR/personas/$persona.md"
    [ -f "$PERSONA_FILE" ] || { log "WARN: $PERSONA_FILE missing, skipping"; continue; }

    log "Persona: $persona on $VM"
    PERSONA_DIR="$FINDINGS_DIR/$persona"
    mkdir -p "$PERSONA_DIR"

    log "Ensuring VM user '$persona' exists"
    ensure_persona_user "$persona"

    PROMPT="$EXPLORER_CONTEXT

# Persona ($persona)

$(cat "$PERSONA_FILE")

# Connection

- VM: $VM
- Persona user: $persona
- Login (primary): $SSH_BASE $persona@$SSH_HOST
- Root escape hatch (use ONLY if your persona instructs you):
    $SSH_VAGRANT 'sudo <command>'
- Binary path on VM: /vagrant/morloc-manager

# Paths

- Report (write here):  $PERSONA_DIR/report.md
- Shared log (read+append): $FINDINGS_DIR/log.md
- HALT sentinel (write only on setup-blocker): $FINDINGS_DIR/HALT

# Task

$TASK"

    claude -p "$PROMPT" \
        --agent vm-explorer \
        --model "$EXPLORER_MODEL" \
        --max-turns "$EXPLORER_MAX_TURNS" \
        --allowedTools "Bash,Read,Write" \
        --no-session-persistence \
        --output-format text \
        < /dev/null 2>&1 | tee "$PERSONA_DIR/session.log"

    log "Done: $persona"

    if [ -f "$FINDINGS_DIR/HALT" ]; then
        log "HALT sentinel present. Skipping remaining personas."
        break
    fi
done

log "Personas done. Running analyst."

ANALYST_PROMPT="$ANALYST_CONTEXT

# Paths

- Per-persona reports: $FINDINGS_DIR/<persona>/report.md
- Shared log: $FINDINGS_DIR/log.md
- HALT sentinel (if present): $FINDINGS_DIR/HALT
- Source (read-only): morloc/
- Docs (read-only): morloc-project.github.io/
- Final report (write here): $FINDINGS_DIR/report.md

# Task

$TASK"

claude -p "$ANALYST_PROMPT" \
    --agent vm-analyst \
    --model "$ANALYST_MODEL" \
    --max-turns "$ANALYST_MAX_TURNS" \
    --allowedTools "Read,Write,Glob,Grep" \
    --no-session-persistence \
    --output-format text \
    < /dev/null 2>&1 | tee "$FINDINGS_DIR/analyst-session.log"

rm -rf "$SSH_DIR"

log "Exploration complete. Results in $FINDINGS_DIR/"
log "Final report: $FINDINGS_DIR/report.md"
log "Shared log: $FINDINGS_DIR/log.md"
