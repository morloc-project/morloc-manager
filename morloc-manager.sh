#!/usr/bin/env sh

# Morloc Installation Manager

# {{{ constants and system info

PROGRAM_NAME="morloc-manager"
VERSION="0.10.0-3"

CONTAINER_ENGINE_VERSION=""
CONTAINER_ENGINE=""
CONTAINER_ENGINE_CLI_OVERRIDE=""

SHARED_MEMORY_SIZE=512m

CONTAINER_BASE_FULL=ghcr.io/morloc-project/morloc/morloc-full
CONTAINER_BASE_TINY=ghcr.io/morloc-project/morloc/morloc-tiny

THIS_SCRIPT_URL="https://raw.githubusercontent.com/morloc-project/morloc-manager/refs/heads/main/morloc-manager.sh"

if [ -n "${MORLOC_CONTAINER_ENGINE:-}" ]; then
    CONTAINER_ENGINE="$MORLOC_CONTAINER_ENGINE"
    CONTAINER_ENGINE_VERSION=$($CONTAINER_ENGINE --version 2>/dev/null | sed 's/.*version \([0-9.]*\).*/\1/')
elif command -v podman >/dev/null 2>&1; then
    CONTAINER_ENGINE_VERSION=$(podman --version 2>/dev/null | sed 's/.* //')
    CONTAINER_ENGINE="podman"
elif command -v docker >/dev/null 2>&1; then
    CONTAINER_ENGINE_VERSION=$(docker --version 2>/dev/null | sed 's/.*version \([0-9.]*\).*/\1/')
    CONTAINER_ENGINE="docker"
fi

SELINUX_SUFFIX=""
SUDO_PREFIX=""

set_container_engine() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "[ERROR] Container engine '$1' not found" >&2
        exit 1
    fi
    CONTAINER_ENGINE="$1"
    CONTAINER_ENGINE_CLI_OVERRIDE=1
    CONTAINER_ENGINE_VERSION=$($CONTAINER_ENGINE --version 2>/dev/null | sed 's/.*version \([0-9.]*\).*/\1/')
}

# Check if the container engine is accessible (daemon running, permissions OK)
check_engine_accessible() {
    if [ -z "$CONTAINER_ENGINE" ]; then
        return 1
    fi
    if $SUDO_PREFIX$CONTAINER_ENGINE info >/dev/null 2>&1; then
        return 0
    fi
    if [ "$CONTAINER_ENGINE" = "docker" ]; then
        print_error "Cannot connect to Docker daemon."
        print_info "Either start the Docker service or add your user to the 'docker' group:"
        print_info "  sudo usermod -aG docker \$USER && newgrp docker"
    elif [ "$CONTAINER_ENGINE" = "podman" ]; then
        print_error "Cannot connect to Podman."
        print_info "Check that podman is properly installed: podman info"
    else
        print_error "Cannot connect to container engine '$CONTAINER_ENGINE'."
    fi
    return 1
}

detect_selinux() {
    if command -v getenforce >/dev/null 2>&1; then
        case "$(getenforce 2>/dev/null)" in
            Enforcing|Permissive)
                SELINUX_SUFFIX=":z"
                ;;
        esac
    fi
}

MORLOC_LIBRARY_RELDIR="src/modules"
MORLOC_DEFAULT_PLANE="default"
MORLOC_DEFAULT_PLANE_GITHUB_ORG="morloclib"
# Legacy sentinel: old dev container data was stored under versions/local/.
# Filter this directory from version listings.
LOCAL_VERSION="local"

# Initialize all paths based on MORLOC_SCOPE. Subcommands set MORLOC_SCOPE
# before calling set_paths when --system is passed.
# When run under sudo, $HOME is /root. Returns the invoking user's home
# so that local-scope paths always point to the real user's directories.
real_home() {
    if [ "$(id -u)" = "0" ] && [ -n "${SUDO_USER:-}" ]; then
        eval echo "~$SUDO_USER"
    else
        echo "$HOME"
    fi
}

set_paths() {
    _sp_home=$(real_home)
    if [ "${MORLOC_SCOPE:-}" = "system" ]; then
        MORLOC_DATA_HOME="/usr/local/share/morloc"
        MORLOC_CONFIG_HOME="/etc/morloc"
        MORLOC_STATE_HOME="/var/lib/morloc"
        MORLOC_CACHE_HOME="/var/cache/morloc"
        MORLOC_DEPENDENCY_DIR="/usr/local/share/morloc/deps"
        MORLOC_BIN="/usr/bin"
    else
        MORLOC_DATA_HOME="${XDG_DATA_HOME:-$_sp_home/.local/share}/morloc"
        MORLOC_CONFIG_HOME="${XDG_CONFIG_HOME:-$_sp_home/.config}/morloc"
        MORLOC_STATE_HOME="${XDG_STATE_HOME:-$_sp_home/.local/state}/morloc"
        MORLOC_CACHE_HOME="${XDG_CACHE_HOME:-$_sp_home/.cache}/morloc"
        MORLOC_DEPENDENCY_DIR="$_sp_home/.local/share/morloc/deps"
        MORLOC_BIN="$_sp_home/.local/bin"
    fi
    MORLOC_HOST_VERSION_DIR="$MORLOC_DATA_HOME/versions"
    PATH_EXPORT_LINE="export PATH=\"${MORLOC_BIN}:\$PATH\""
}

# Set default paths (subcommands re-call after parsing --system)
set_paths

# }}}
# {{{ printing functions

# Colors and text formatting for output (with robust fallback for maximum portability)
if [ -t 1 ]; then
    # Check if we have tput and it supports colors
    if command -v tput >/dev/null 2>&1 && tput colors >/dev/null 2>&1 && [ "$(tput colors 2>/dev/null || echo 0)" -gt 0 ]; then
        # Use tput for maximum compatibility with different terminals
        RED=$(tput setaf 1 2>/dev/null || echo "")
        GREEN=$(tput setaf 2 2>/dev/null || echo "")
        YELLOW=$(tput setaf 3 2>/dev/null || echo "")
        BLUE=$(tput setaf 4 2>/dev/null || echo "")
        MAGENTA=$(tput setaf 5 2>/dev/null || echo "")
        CYAN=$(tput setaf 6 2>/dev/null || echo "")

        # Text attributes
        BOLD=$(tput bold 2>/dev/null || echo "")
        DIM=$(tput dim 2>/dev/null || echo "")
        UNDERLINE=$(tput smul 2>/dev/null || echo "")
        REVERSE=$(tput rev 2>/dev/null || echo "")
        BLINK=$(tput blink 2>/dev/null || echo "")

        RESET=$(tput sgr0 2>/dev/null || echo "")
    # Fallback to ANSI escape codes if tput isn't available but terminal likely supports colors
    elif [ -n "$TERM" ] && [ "$TERM" != "dumb" ] && [ "$TERM" != "unknown" ]; then
        # Check for common color-capable terminal types
        case "$TERM" in
            *color*|*256*|xterm*|screen*|tmux*|rxvt*|gnome*|konsole*|alacritty*|kitty*)
                ESC=$(printf '\033')
                RED="${ESC}[0;31m"
                GREEN="${ESC}[0;32m"
                YELLOW="${ESC}[0;33m"
                BLUE="${ESC}[0;34m"
                MAGENTA="${ESC}[0;35m"
                CYAN="${ESC}[0;36m"

                # Text attributes
                BOLD="${ESC}[1m"
                DIM="${ESC}[2m"
                UNDERLINE="${ESC}[4m"
                REVERSE="${ESC}[7m"
                BLINK="${ESC}[5m"

                RESET="${ESC}[0m"
                ;;
            *)
                # Conservative: disable colors for unknown terminals
                RED=""
                GREEN=""
                YELLOW=""
                BLUE=""
                MAGENTA=""
                CYAN=""
                BOLD=""
                DIM=""
                UNDERLINE=""
                REVERSE=""
                BLINK=""
                RESET=""
                ;;
        esac
    else
        # No colors for non-color terminals or when TERM is unset/dumb
        RED=""
        GREEN=""
        YELLOW=""
        BLUE=""
        MAGENTA=""
        CYAN=""
        BOLD=""
        DIM=""
        UNDERLINE=""
        REVERSE=""
        BLINK=""
        RESET=""
    fi
else
    # No colors when not connected to a terminal (piped/redirected output)
    RED=""
    GREEN=""
    YELLOW=""
    BLUE=""
    MAGENTA=""
    CYAN=""
    BOLD=""
    DIM=""
    UNDERLINE=""
    REVERSE=""
    BLINK=""
    RESET=""
fi

# Print colored output
print_info() {
    printf "${BLUE}[INFO]${RESET} %s\n" "$1"
}

print_success() {
    printf "${GREEN}[SUCCESS]${RESET} %s\n" "$1"
}

print_warning() {
    printf "${YELLOW}[WARNING]${RESET} %s\n" "$1"
}

print_error() {
    printf "${RED}[ERROR]${RESET} %s\n" "$1"
}

print_point() {
    printf "  %s\n" "$1"
}

# }}}
# {{{ helper functions

# Function to create the target directory
create_directory() {
    DIR=$1

    if [ -d "$DIR" ]; then
        print_warning "Directory $DIR already exists"
        return 0
    fi

    print_info "Creating directory: $DIR"
    if ! $SUDO_PREFIX mkdir -p "$DIR" 2>/dev/null; then
        print_error "Failed to create directory: $DIR"
        return 1
    fi

    print_success "Created directory: $DIR"
    return 0
}



# Function to normalize a path (remove trailing slashes, resolve basic issues)
normalize_path() {
    _np_path="$1"
    # Remove trailing slashes (but keep root /)
    while [ "$_np_path" != "/" ] && [ "${_np_path%/}" != "$_np_path" ]; do
        _np_path="${_np_path%/}"
    done
    # Collapse multiple consecutive slashes
    echo "$_np_path" | sed 's|//*|/|g'
}

# Function to resolve a path to absolute (POSIX-portable)
resolve_path() {
    _rp_path="$1"
    if [ -f "$_rp_path" ]; then
        _rp_dir=$(cd "$(dirname "$_rp_path")" && pwd)
        echo "$_rp_dir/$(basename "$_rp_path")"
    elif [ -d "$_rp_path" ]; then
        (cd "$_rp_path" && pwd)
    else
        # File doesn't exist yet; resolve parent dir
        _rp_dir=$(cd "$(dirname "$_rp_path")" 2>/dev/null && pwd)
        if [ -n "$_rp_dir" ]; then
            echo "$_rp_dir/$(basename "$_rp_path")"
        else
            echo "$_rp_path"
        fi
    fi
}

# Function to check if directory is already in PATH
is_in_path() {
    local target_dir="$1"
    local normalized_target
    local path_entry
    local normalized_entry

    # Normalize the target directory
    normalized_target=$(normalize_path "$target_dir")

    # Handle empty PATH
    if [ -z "$PATH" ]; then
        return 1
    fi

    # Save IFS and set it to handle path separation
    local old_ifs="$IFS"
    IFS=':'

    # Check each PATH entry
    for path_entry in $PATH; do
        # Skip empty entries
        if [ -n "$path_entry" ]; then
            normalized_entry=$(normalize_path "$path_entry")
            if [ "$normalized_target" = "$normalized_entry" ]; then
                IFS="$old_ifs"
                return 0
            fi
        fi
    done

    # Restore IFS
    IFS="$old_ifs"
    return 1
}

# }}}
# {{{ config helpers

# Returns the config root directory for the given scope
# Usage: config_root [--system]
config_root() {
    if [ "${1:-}" = "--system" ]; then
        echo "/etc/morloc"
    elif [ "$(id -u)" = "0" ] && [ -n "${SUDO_USER:-}" ]; then
        # Under sudo, XDG vars belong to root; use real user's home
        echo "$(real_home)/.config/morloc"
    else
        echo "${XDG_CONFIG_HOME:-$(real_home)/.config}/morloc"
    fi
}

# Returns the data root directory for the given scope
# Usage: data_root [--system]
data_root() {
    if [ "${1:-}" = "--system" ]; then
        echo "/usr/local/share/morloc"
    elif [ "$(id -u)" = "0" ] && [ -n "${SUDO_USER:-}" ]; then
        echo "$(real_home)/.local/share/morloc"
    else
        echo "${XDG_DATA_HOME:-$(real_home)/.local/share}/morloc"
    fi
}

# Returns the bin directory for the given scope
# Usage: bin_root [--system]
bin_root() {
    if [ "${1:-}" = "--system" ]; then
        echo "/usr/bin"
    else
        echo "$(real_home)/.local/bin"
    fi
}

# Read a key=value from a config file
# Usage: read_config KEY [FILE]
# FILE defaults to the user config file
read_config() {
    _rc_key="$1"
    _rc_file="${2:-$(config_root)/config}"
    [ -f "$_rc_file" ] || return 0
    while IFS= read -r _rc_line || [ -n "$_rc_line" ]; do
        case "$_rc_line" in
            "${_rc_key}="*) printf '%s' "${_rc_line#*=}"; return 0 ;;
        esac
    done < "$_rc_file"
}

# Write or update a key=value in a config file
# Usage: write_config KEY VALUE [FILE] [--sudo]
# FILE defaults to the user config file. Creates parent dirs if needed.
# Pass --sudo as 4th arg to run with sudo (for system-scope files).
write_config() {
    _wc_key="$1"
    _wc_value="$2"
    _wc_file="${3:-$(config_root)/config}"
    _wc_sudo=""
    [ "${4:-}" = "--sudo" ] && _wc_sudo="sudo "
    if ! $_wc_sudo mkdir -p "$(dirname "$_wc_file")"; then
        print_error "Failed to create config directory: $(dirname "$_wc_file")"
        return 1
    fi
    if [ -f "$_wc_file" ] && grep -q "^${_wc_key}=" "$_wc_file" 2>/dev/null; then
        if ! $_wc_sudo sed -i "s|^${_wc_key}=.*|${_wc_key}=${_wc_value}|" "$_wc_file"; then
            print_error "Failed to update config key '$_wc_key' in $_wc_file"
            return 1
        fi
    else
        if ! printf '%s=%s\n' "$_wc_key" "$_wc_value" | $_wc_sudo tee -a "$_wc_file" > /dev/null; then
            print_error "Failed to write config key '$_wc_key' to $_wc_file"
            return 1
        fi
    fi
}

# Read active_version from user config
active_version() {
    read_config "active_version"
}

# Read active_scope from user config, default "local"
active_scope() {
    _as_val=$(read_config "active_scope")
    echo "${_as_val:-local}"
}

# Resolve which scope a version is in: checks local first, then system.
# Prints "local" or "system". Returns 1 if not found.
# Usage: resolve_version VERSION
resolve_version() {
    _rv_ver="$1"
    _rv_local_data="$(data_root)/versions/$_rv_ver"
    _rv_sys_data="$(data_root --system)/versions/$_rv_ver"
    if [ -d "$_rv_local_data" ]; then
        echo "local"
        return 0
    elif [ -d "$_rv_sys_data" ]; then
        echo "system"
        return 0
    fi
    return 1
}

# List installed versions
# Usage: list_versions [--local|--system|--all]
list_versions() {
    _lv_filter="${1:---all}"
    _list_from_dir() {
        _lv_dir="$1/versions"
        _lv_scope="$2"
        if [ -d "$_lv_dir" ]; then
            for _lv_d in "$_lv_dir"/*/; do
                [ -d "$_lv_d" ] || continue
                _lv_v=$(basename "$_lv_d")
                case "$_lv_v" in "$LOCAL_VERSION") continue ;; esac
                printf '%s\t%s\n' "$_lv_v" "$_lv_scope"
            done
        fi
    }
    case "$_lv_filter" in
        --local)  _list_from_dir "$(data_root)" "local" ;;
        --system) _list_from_dir "$(data_root --system)" "system" ;;
        --all)
            _list_from_dir "$(data_root)" "local"
            _list_from_dir "$(data_root --system)" "system"
            ;;
    esac
}

# Returns the version-specific config directory
# Usage: version_config_root VER SCOPE
version_config_root() {
    _vcr_ver="$1"
    _vcr_scope="$2"
    if [ "$_vcr_scope" = "system" ]; then
        echo "$(config_root --system)/versions/$_vcr_ver"
    else
        echo "$(config_root)/versions/$_vcr_ver"
    fi
}

# Returns the version-specific data directory
# Usage: version_data_root VER SCOPE
version_data_root() {
    _vdr_ver="$1"
    _vdr_scope="$2"
    if [ "$_vdr_scope" = "system" ]; then
        echo "$(data_root --system)/versions/$_vdr_ver"
    else
        echo "$(data_root)/versions/$_vdr_ver"
    fi
}

# Write per-version config + base.conf environment
# Usage: write_version_config VER SCOPE
write_version_config() {
    _wvc_ver="$1"
    _wvc_scope="$2"
    _wvc_cfgdir=$(version_config_root "$_wvc_ver" "$_wvc_scope")
    _wvc_datadir=$(version_data_root "$_wvc_ver" "$_wvc_scope")

    _wvc_sudo=""
    [ "$_wvc_scope" = "system" ] && _wvc_sudo="--sudo"

    if [ -n "$_wvc_sudo" ]; then
        if ! sudo mkdir -p "$_wvc_cfgdir/environments"; then
            print_error "Failed to create config directory: $_wvc_cfgdir/environments"
            return 1
        fi
    else
        if ! mkdir -p "$_wvc_cfgdir/environments"; then
            print_error "Failed to create config directory: $_wvc_cfgdir/environments"
            return 1
        fi
    fi

    _wvc_cfg="$_wvc_cfgdir/config"
    write_config "image" "${CONTAINER_BASE_FULL}:${_wvc_ver}" "$_wvc_cfg" $_wvc_sudo || return 1
    write_config "host_dir" "$_wvc_datadir" "$_wvc_cfg" $_wvc_sudo || return 1
    write_config "container_engine" "$CONTAINER_ENGINE" "$_wvc_cfg" $_wvc_sudo || return 1
    write_config "shm_size" "$SHARED_MEMORY_SIZE" "$_wvc_cfg" $_wvc_sudo || return 1

    # Create base.conf environment if it doesn't exist
    _wvc_base="$_wvc_cfgdir/environments/base.conf"
    if [ ! -f "$_wvc_base" ]; then
        if [ -n "$_wvc_sudo" ]; then
            if ! printf 'image=%s\n' "${CONTAINER_BASE_FULL}:${_wvc_ver}" | sudo tee "$_wvc_base" > /dev/null; then
                print_error "Failed to write base environment config"
                return 1
            fi
        else
            if ! printf 'image=%s\n' "${CONTAINER_BASE_FULL}:${_wvc_ver}" > "$_wvc_base"; then
                print_error "Failed to write base environment config"
                return 1
            fi
        fi
    fi

    # Update active version in the appropriate scope config
    if [ "$_wvc_scope" = "system" ]; then
        # System install: write to system config only, never touch user config
        _wvc_sys_cfg="$(config_root --system)/config"
        write_config "active_version" "$_wvc_ver" "$_wvc_sys_cfg" --sudo || return 1
        write_config "active_scope" "system" "$_wvc_sys_cfg" --sudo || return 1
        write_config "active_env" "base" "$_wvc_sys_cfg" --sudo || return 1
    else
        _wvc_user_cfg="$(config_root)/config"
        write_config "active_version" "$_wvc_ver" "$_wvc_user_cfg" || return 1
        write_config "active_scope" "$_wvc_scope" "$_wvc_user_cfg" || return 1
        write_config "active_env" "base" "$_wvc_user_cfg" || return 1
    fi

    print_success "Wrote version config for $_wvc_ver ($_wvc_scope)"
}

# Read flags from a file, stripping comments and blank lines.
# Prints one flag per line.
# Usage: read_flags_file FILE
read_flags_file() {
    _rff_file="$1"
    [ -f "$_rff_file" ] || return 0
    while IFS= read -r _rff_line || [ -n "$_rff_line" ]; do
        # Strip comments
        _rff_line="${_rff_line%%#*}"
        # Trim leading whitespace (POSIX)
        _rff_line="${_rff_line#"${_rff_line%%[![:space:]]*}"}"
        # Trim trailing whitespace (POSIX)
        _rff_line="${_rff_line%"${_rff_line##*[![:space:]]}"}"
        [ -z "$_rff_line" ] && continue
        printf '%s\n' "$_rff_line"
    done < "$_rff_file"
}

# }}}
# {{{ setup Morloc bin folder

# }}}
# {{{ script generation helpers

# build an environment container if it does not yet exist
# Usage: build_environment NAME DOCKERFILE TAG BASE [EXTRA_BUILD_ARGS]
build_environment() {
    envname=$1
    dockerfile=$2
    envtag=$3
    container_base=$4
    _be_extra=${5:-}

    # Hash-based rebuild detection: store a SHA of the Dockerfile after each
    # successful build and skip the rebuild when the hash matches.
    _be_hash_file="${dockerfile%.Dockerfile}.sha256"
    _be_current_hash=""
    if [ -f "$dockerfile" ]; then
        _be_current_hash=$(sha256sum "$dockerfile" 2>/dev/null | cut -d' ' -f1)
        [ -z "$_be_current_hash" ] && _be_current_hash=$(shasum -a 256 "$dockerfile" 2>/dev/null | cut -d' ' -f1)
    fi

    # Check if image already exists and Dockerfile is unchanged
    if $SUDO_PREFIX$CONTAINER_ENGINE image inspect "$envtag" >/dev/null 2>&1; then
        if [ -f "$dockerfile" ]; then
            _be_stored_hash=""
            [ -f "$_be_hash_file" ] && _be_stored_hash=$(cat "$_be_hash_file")
            if [ -n "$_be_current_hash" ] && [ "$_be_current_hash" = "$_be_stored_hash" ]; then
                print_info "Image '$envtag' is up to date"
                return 0
            else
                print_info "Dockerfile has been modified, rebuilding image '$envtag'"
            fi
        else
            print_warning "Dockerfile '$dockerfile' not found, but image exists. Using existing image."
            return 0
        fi
    else
        print_info "Building new image '$envtag'"
    fi

    # Build the image (quotes needed in case of spaces in paths)
    # shellcheck disable=SC2086
    if ! $SUDO_PREFIX$CONTAINER_ENGINE build --build-arg CONTAINER_BASE="$container_base" --tag "$envtag" --file "$dockerfile" $_be_extra "$(dirname "$dockerfile")"; then
        print_error "Failed to build image '$envtag' from '$dockerfile'"
        return 1
    fi

    # Store Dockerfile hash for future rebuild detection
    if [ -n "$_be_current_hash" ]; then
        printf '%s\n' "$_be_current_hash" > "$_be_hash_file" 2>/dev/null || \
            $SUDO_PREFIX sh -c "printf '%s\n' '$_be_current_hash' > '$_be_hash_file'" 2>/dev/null || true
    fi

    print_success "Built image '$envtag'"
    return 0
}


# }}}
# {{{ run and shell subcommands

show_run_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") run [OPTIONS] [--] COMMAND [ARGS...]

Run a command inside the morloc container.

${BOLD}OPTIONS${RESET}:
  -h, --help       Show this help message
      --shell      Open an interactive bash shell in the container
      --system     Use system-scope version
      --local      Use local-scope version
  -x FLAG          Pass an extra flag to docker/podman run (repeatable)
      --            Stop processing options; remaining args are the command

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") run morloc --version
  $(basename "$0") run morloc make -o foo foo.loc
  $(basename "$0") run --shell
  $(basename "$0") run -x "--gpus all" python train.py

${BOLD}NOTE${RESET}:
  Use -- before the command when it has flags that could be confused
  with morloc-manager options (e.g., run -- morloc make -o out foo.loc).
  It is optional when the command has no ambiguous flags.
EOF
}

# Run a command inside the morloc container
# Usage: cmd_run [--shell] [-x FLAG]... [--] CMD...
cmd_run() {
    _cr_shell=""
    _cr_scope=""
    _cr_extra=""

    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)    show_run_help; exit 0 ;;
            --shell)      _cr_shell=1; shift ;;
            --system)     _cr_scope="system"; shift ;;
            --local)      _cr_scope="local"; shift ;;
            -x)           _cr_extra="$_cr_extra $2"; shift 2 ;;
            --)           shift; break ;;
            *)            break ;;
        esac
    done

    # Resolve scope first so we can check the right config for version
    if [ -z "$_cr_scope" ]; then
        _cr_scope=$(active_scope)
    fi

    # Resolve version from config
    _cr_version=$(active_version)
    if [ -z "$_cr_version" ] && [ "$_cr_scope" = "system" ]; then
        # Try reading from system config directly
        _cr_version=$(read_config "active_version" "$(config_root --system)/config")
    fi
    if [ -z "$_cr_version" ]; then
        # Check if a system version is available before erroring
        _cr_sys_versions=$(list_versions --system 2>/dev/null)
        if [ -n "$_cr_sys_versions" ]; then
            _cr_sys_first=$(echo "$_cr_sys_versions" | head -n1 | cut -f1)
            print_info "No active version set, but a system version is available."
            print_info "Run: $(basename "$0") select --system $_cr_sys_first"
        fi
        print_error "No active version set. Run 'install' or 'select' first."
        exit 1
    fi

    # Require either --shell or a command
    if [ -z "$_cr_shell" ] && [ $# -eq 0 ]; then
        print_error "No command specified. Use 'run --shell' for interactive mode or provide a command."
        show_run_help
        exit 1
    fi

    # Read version config
    _cr_scope_flag=""
    [ "$_cr_scope" = "system" ] && _cr_scope_flag="--system"

    _cr_vcfg=$(version_config_root "$_cr_version" "$_cr_scope")
    _cr_vdata=$(version_data_root "$_cr_version" "$_cr_scope")
    _cr_versions_dir="$(data_root $_cr_scope_flag)/versions"

    # Read image from version config
    _cr_image=$(read_config "image" "$_cr_vcfg/config")
    [ -z "$_cr_image" ] && _cr_image="${CONTAINER_BASE_FULL}:${_cr_version}"

    # Read active environment from the correct scope config
    if [ "$_cr_scope" = "system" ]; then
        _cr_scope_cfg="$(config_root --system)/config"
    else
        _cr_scope_cfg="$(config_root)/config"
    fi
    _cr_env=$(read_config "active_env" "$_cr_scope_cfg")
    if [ -n "$_cr_env" ] && [ "$_cr_env" != "base" ]; then
        _cr_env_conf="$_cr_vcfg/environments/${_cr_env}.conf"
        _cr_env_image=$(read_config "image" "$_cr_env_conf")
        [ -n "$_cr_env_image" ] && _cr_image="$_cr_env_image"
    fi

    # Read container engine: CLI --container-engine > version config > auto-detected
    if [ -n "$CONTAINER_ENGINE_CLI_OVERRIDE" ]; then
        _cr_engine="$CONTAINER_ENGINE"
    else
        _cr_engine=$(read_config "container_engine" "$_cr_vcfg/config")
        if [ -n "$_cr_engine" ] && ! command -v "$_cr_engine" >/dev/null 2>&1; then
            print_warning "Stored engine '$_cr_engine' not found; using '$CONTAINER_ENGINE'"
            _cr_engine="$CONTAINER_ENGINE"
        elif [ -n "$_cr_engine" ] && ! $SUDO_PREFIX$_cr_engine info >/dev/null 2>&1; then
            print_warning "Stored engine '$_cr_engine' not accessible; using '$CONTAINER_ENGINE'"
            _cr_engine="$CONTAINER_ENGINE"
        fi
        [ -z "$_cr_engine" ] && _cr_engine="$CONTAINER_ENGINE"
    fi

    # Check that the selected image exists in the current engine's store
    _cr_check_image="$_cr_image"
    if [ -n "$_cr_env" ] && [ "$_cr_env" != "base" ]; then
        if ! $SUDO_PREFIX$_cr_engine image inspect "$_cr_check_image" >/dev/null 2>&1; then
            print_error "Image '$_cr_check_image' not found in $_cr_engine's image store."
            print_info "Rebuild it with: $(basename "$0") env $_cr_env"
            exit 1
        fi
    fi

    # Read shared memory size from version config, fall back to default
    _cr_shm=$(read_config "shm_size" "$_cr_vcfg/config")
    _cr_shm="${_cr_shm:-$SHARED_MEMORY_SIZE}"

    # Determine container home — use invoking user's home, not root's
    _cr_real_home=$(real_home)
    _cr_container_home="$_cr_real_home"
    if [ "$_cr_scope" = "system" ]; then
        _cr_container_home="/root"
    fi

    # Detect SELinux — skip :z for system scope (pre-labeled at install time)
    detect_selinux
    if [ "$_cr_scope" = "system" ]; then
        _cr_z=""
    else
        _cr_z="$SELINUX_SUFFIX"
    fi

    export MORLOC_WORK_DIR="$PWD"

    # Build base flags
    _cr_use_image="$_cr_image"

    _cr_flags="-v ${_cr_vdata}:${_cr_container_home}/.local/share/morloc${_cr_z}"
    _cr_flags="$_cr_flags -v ${_cr_vdata}/bin:${_cr_container_home}/.local/bin${_cr_z}"
    _cr_flags="$_cr_flags -e HOME=${_cr_container_home}"
    _cr_flags="$_cr_flags -e PATH=${_cr_container_home}/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

    # Mount working directory (skip for "morloc init" which doesn't need it —
    # mounting $HOME triggers SELinux relabeling errors)
    _cr_need_workdir=true
    case "$1" in
        morloc) [ "${2:-}" = "init" ] && _cr_need_workdir=false ;;
    esac
    if $_cr_need_workdir; then
        # IMPORTANT: SELinux `:z` relabeling restrictions
        # -------------------------------------------
        # When SELinux is enforcing/permissive, we mount volumes with `:z`
        # so the container can access them. However, SELinux *forbids*
        # relabeling certain directories:
        #
        #   1. System directories: /tmp, /var/tmp, etc.
        #   2. The user's home directory root itself ($HOME) — because
        #      relabeling $HOME would change labels on *all* dotfiles,
        #      SSH keys, shell configs, etc., which breaks the system.
        #
        # Subdirectories of $HOME (e.g. ~/project/) are fine — SELinux
        # only blocks relabeling the home dir *root*.
        #
        # Do NOT weaken these checks (e.g. by dropping `:z`) — that
        # would cause silent "Permission denied" failures inside the
        # container on SELinux systems. The correct fix is always to
        # require users to work in a subdirectory.
        # -------------------------------------------
        if [ -n "$_cr_z" ]; then
            case "$MORLOC_WORK_DIR" in
                /tmp/*|/tmp|/var/tmp/*|/var/tmp)
                    print_error "Cannot bind-mount '$MORLOC_WORK_DIR': SELinux prevents relabeling system directories."
                    print_info "Move your project to a non-system directory (e.g. ~/project/) and retry."
                    exit 1
                    ;;
            esac
            # Block mounting $HOME itself — SELinux forbids relabeling it.
            # This is the #1 first-run failure on Fedora/RHEL; keep this
            # check even if the error message from the container engine
            # changes in future versions.
            if [ "$MORLOC_WORK_DIR" = "$_cr_real_home" ]; then
                print_error "Cannot bind-mount your home directory ('$MORLOC_WORK_DIR') with SELinux."
                print_info "SELinux does not allow relabeling your home directory root."
                print_info "Create a project subdirectory and run from there:"
                print_info "  mkdir -p ~/project && cd ~/project"
                exit 1
            fi
        fi
        _cr_flags="$_cr_flags -v ${MORLOC_WORK_DIR}:${MORLOC_WORK_DIR}${_cr_z}"
        _cr_workdir="${MORLOC_WORK_DIR}"
    else
        _cr_workdir="${_cr_container_home}"
    fi

    # Read global flags file
    _cr_global_flags_file="$(data_root $_cr_scope_flag)/morloc.flags"
    _cr_user_flags=""
    if [ -f "$_cr_global_flags_file" ]; then
        _cr_gf=$(read_flags_file "$_cr_global_flags_file")
        [ -n "$_cr_gf" ] && _cr_user_flags="$_cr_gf"
    fi

    # Read environment flags
    if [ -n "$_cr_env" ] && [ "$_cr_env" != "base" ]; then
        _cr_env_flags_file="$_cr_vcfg/environments/${_cr_env}.flags"
        if [ -f "$_cr_env_flags_file" ]; then
            _cr_ef=$(read_flags_file "$_cr_env_flags_file")
            [ -n "$_cr_ef" ] && _cr_user_flags="$_cr_user_flags $_cr_ef"
        fi
    fi

    # Also support legacy per-dependency flags
    _cr_env_flags_path=$(read_config "env_flags")
    if [ -n "$_cr_env_flags_path" ] && [ -f "$_cr_env_flags_path" ]; then
        _cr_lf=$(read_flags_file "$_cr_env_flags_path")
        [ -n "$_cr_lf" ] && _cr_user_flags="$_cr_user_flags $_cr_lf"
    fi

    # Build sudo prefix — only use sudo when explicitly running as root for system scope.
    # Regular users should have docker group membership or use rootless podman.
    _cr_sudo=""
    if [ "$_cr_scope" = "system" ] && [ "$(id -u)" = "0" ]; then
        _cr_sudo="sudo"
    fi

    # Execute
    if [ -n "$_cr_shell" ]; then
        if ! [ -t 0 ] || ! [ -t 1 ]; then
            print_error "--shell requires an interactive terminal (TTY)."
            print_info "If connecting over SSH, use: ssh -t <host> morloc-manager run --shell"
            exit 1
        fi
        if [ "${_MORLOC_NO_EXEC:-}" = "1" ]; then
            # shellcheck disable=SC2086
            $_cr_sudo "$_cr_engine" run --rm -it --shm-size ${_cr_shm} -w "$_cr_workdir" \
                $_cr_flags $_cr_user_flags $_cr_extra "$_cr_use_image" /bin/bash
            return $?
        fi
        # shellcheck disable=SC2086
        exec $_cr_sudo "$_cr_engine" run --rm -it --shm-size ${_cr_shm} -w "$_cr_workdir" \
            $_cr_flags $_cr_user_flags $_cr_extra "$_cr_use_image" /bin/bash
    else
        if [ "${_MORLOC_NO_EXEC:-}" = "1" ]; then
            # shellcheck disable=SC2086
            $_cr_sudo "$_cr_engine" run --rm --shm-size ${_cr_shm} -w "$_cr_workdir" \
                $_cr_flags $_cr_user_flags $_cr_extra "$_cr_use_image" "$@"
            return $?
        fi
        # shellcheck disable=SC2086
        exec $_cr_sudo "$_cr_engine" run --rm --shm-size ${_cr_shm} -w "$_cr_workdir" \
            $_cr_flags $_cr_user_flags $_cr_extra "$_cr_use_image" "$@"
    fi
}

# }}}
# {{{ main help and version

# Version function
show_version() {
    echo "${VERSION}"
}

show_help() {
    cat << EOF
${BOLD}$(basename "$0")${RESET} ${VERSION} - manage morloc containerized installation

${BOLD}USAGE${RESET}: $(basename "$0") [OPTIONS] COMMAND [ARGS...]

${BOLD}OPTIONS${RESET}:
  -h, --help                Show this help message
  -v, --version             Show this manager version
  --container-engine ENGINE  Use ENGINE instead of auto-detected (docker/podman)

${BOLD}COMMANDS${RESET}:
  ${BOLD}${GREEN}install${RESET}    Install morloc containers and home
  ${BOLD}${GREEN}uninstall${RESET}  Remove morloc containers and home
  ${BOLD}${GREEN}clean${RESET}      Remove build artifacts (keeps containers)
  ${BOLD}${GREEN}update${RESET}     Pull the latest version of this script
  ${BOLD}${GREEN}select${RESET}     Choose a new Morloc version
  ${BOLD}${GREEN}run${RESET}        Run a command inside the morloc container
  ${BOLD}${GREEN}env${RESET}        Select or explore available environments
  ${BOLD}${GREEN}info${RESET}       Print info about manager, installs and containers

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") install
  $(basename "$0") uninstall
  $(basename "$0") --container-engine docker install
  $(basename "$0") --help
EOF
}

# }}}
# {{{ install subcommand

# Help for install subcommand
show_install_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") install [OPTIONS] <version>

Setup morloc containers, scripts, and home for either the latest version
of Morloc or for the specified version.

After installation, use:
  $(basename "$0") run morloc make -o foo foo.loc   Run a command in the container
  $(basename "$0") run --shell                      Interactive shell

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message
      --system         Install to system scope (rootful)
      --no-init        Do not run 'morloc init'
      --shm-size SIZE  Container shared memory size (default: 512m)

${BOLD}ARGUMENTS${RESET}:
  version        Version to install

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") install
  $(basename "$0") install 0.54.2
  $(basename "$0") install --system 0.54.2
EOF
}

# Pull an image if not already present in the engine's store
_pull_if_missing() {
    _pim_image="$1"
    _pim_label="$2"
    if $SUDO_PREFIX$CONTAINER_ENGINE image inspect "$_pim_image" >/dev/null 2>&1; then
        print_info "Image '$_pim_image' already present, skipping pull"
        return 0
    fi
    _pim_tmpfile=$(mktemp 2>/dev/null || echo "/tmp/morloc-pull.$$")
    $SUDO_PREFIX$CONTAINER_ENGINE pull "$_pim_image" 2>&1 | tee "$_pim_tmpfile"
    # POSIX: can't get pipe exit status reliably; check if image is now present
    if ! $SUDO_PREFIX$CONTAINER_ENGINE image inspect "$_pim_image" >/dev/null 2>&1; then
        print_error "Failed to pull container '$_pim_label' ($_pim_image)"
        if grep -qi "no space left on device" "$_pim_tmpfile" 2>/dev/null; then
            echo "  Disk space is insufficient. Free space and retry."
            echo "  Run '$(basename "$0") clean' to remove cached data."
        else
            echo "  Are you sure this Morloc version is defined?"
            echo "  If you are behind a corporate firewall or proxy, configure your container engine:"
            echo "    docker: set HTTPS_PROXY environment variable"
            echo "    podman: set HTTPS_PROXY or configure in /etc/containers/registries.conf"
        fi
        rm -f "$_pim_tmpfile"
        return 1
    fi
    rm -f "$_pim_tmpfile"
}

# Install subcommand
cmd_install() {
    # calling these "undefined" instead of empty strings for better debugging
    version="undefined"
    tag="undefined"
    no_init="false"

    # Parse install subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_install_help
                exit 0
                ;;
            --system)
                MORLOC_SCOPE="system"
                SUDO_PREFIX="sudo "
                set_paths
                shift
                ;;
            --no-init)
                no_init="true"
                shift
                ;;
            --shm-size)
                SHARED_MEMORY_SIZE="$2"
                shift 2
                ;;
            -*)
                print_error "Unknown option for install: $1"
                show_install_help
                exit 1
                ;;
            *)
                if [ "$version" = "undefined" ]; then
                    version="$1"
                else
                    print_error "Multiple version installation not supported: $1"
                    exit 1
                fi
                shift
                ;;
        esac
    done

    if [ "$version" = "undefined" ]; then
        print_info "Detecting latest available version..."
        tag="edge"
    else
        print_info "Installing Morloc v$version"
        tag=$version
    fi

    print_info "Looking for a container engine"

    # check if an appropriate container engine is installed
    if [ -z "$CONTAINER_ENGINE" ]; then
        print_error "No container engine found, please install podman or docker"
        exit 1
    else
        print_info "Using $CONTAINER_ENGINE $CONTAINER_ENGINE_VERSION as container engine"
    fi

    check_engine_accessible || exit 1

    if [ "$version" = "undefined" ]
    then
        print_info "Attempting to pull containers for Morloc tag '$tag'"
    else
        print_info "Attempting to pull containers for Morloc version $version"
    fi

    _pull_if_missing "$CONTAINER_BASE_TINY:${tag}" "tiny" || exit 1
    _pull_if_missing "$CONTAINER_BASE_FULL:${tag}" "full" || exit 1

    # get Morloc version from container
    # filter out the carriage return that podman helpfully provided
    if [ "$version" = "undefined" ]
    then
        detected_version=$($SUDO_PREFIX$CONTAINER_ENGINE run --rm "$CONTAINER_BASE_FULL:edge" morloc --version 2>/dev/null)
        if [ $? -ne 0 ]
        then
            print_error "Failed to detect version from morloc container"
            exit 1
        fi
        detected_version=$(printf '%s' "$detected_version" | tr -d '\r\n')

        if [ -z "$detected_version" ]
        then
            print_error "No Morloc version found - something went wrong"
            exit 1
        fi
        print_info "Detected Morloc v$detected_version in retrieved container"
        version=$detected_version
    fi

    morloc_data_home="$MORLOC_HOST_VERSION_DIR/$version"

    print_info "Setting Morloc home to '${morloc_data_home}'"

    # create .morloc/version/$version folder
    create_directory "$morloc_data_home"
    if [ $? -ne 0 ]
    then
        print_error "Failed to create morloc home directory at '$morloc_data_home'"
        exit 1
    fi
    for _inst_subdir in bin include lib opt src/morloc/plane tmp; do
        create_directory "$morloc_data_home/$_inst_subdir" || {
            print_error "Failed to create directory: $morloc_data_home/$_inst_subdir"
            exit 1
        }
    done

    print_info "Created $morloc_data_home"

    # Warn about legacy docker-compose.override.yml
    if [ -f "$MORLOC_DATA_HOME/docker-compose.override.yml" ]; then
        print_warning "Found legacy docker-compose.override.yml in $MORLOC_DATA_HOME"
        print_info "Compose is no longer used. Migrate custom flags to morloc.flags or env-specific .flags files."
        print_info "See 'morloc-manager env --help' for details."
    fi

    # Determine scope
    _inst_scope="local"
    [ "${MORLOC_SCOPE:-}" = "system" ] && _inst_scope="system"

    # Write version config
    if ! write_version_config "$version" "$_inst_scope"; then
        print_error "Failed to write version config"
        exit 1
    fi

    # Pre-label system data dirs for SELinux so non-root users don't need :z relabeling
    if [ "$_inst_scope" = "system" ]; then
        detect_selinux
        if [ -n "$SELINUX_SUFFIX" ]; then
            print_info "Pre-labeling system data directory for SELinux"
            chcon -R -t container_file_t "$morloc_data_home" 2>/dev/null || true
        fi
    fi

    if [ "$no_init" = "false" ]; then
      print_info "Initializing morloc libraries"
      # Use --userns=keep-id for rootless podman (matches cmd_run logic);
      # use --user UID:GID for docker/rootful podman to avoid root-owned files
      if [ "$_inst_scope" = "local" ]; then
          if [ "$CONTAINER_ENGINE" = "podman" ] && [ "$(id -u)" != "0" ]; then
              _MORLOC_NO_EXEC=1 cmd_run -x "--userns=keep-id" -- morloc init -f
          else
              _MORLOC_NO_EXEC=1 cmd_run -x "--user $(id -u):$(id -g)" -- morloc init -f
          fi
      else
          _MORLOC_NO_EXEC=1 cmd_run --system morloc init -f
      fi
      if [ $? -ne 0 ]
      then
          # Recover file ownership so user can retry or uninstall cleanly
          if [ "$_inst_scope" = "local" ] && [ -d "$morloc_data_home" ]; then
              print_info "Attempting to recover file ownership in $morloc_data_home"
              sudo chown -R "$(id -u):$(id -g)" "$morloc_data_home" 2>/dev/null || true
          fi
          print_error "Failed to build morloc libraries"
          exit 1
      fi
    else
      print_info "Skipping morloc init step"
    fi

    print_success "Morloc v$version installed successfully"
}

# }}}
# {{{ uninstall subcommand

# Function to remove all containers for a given image
# Usage: remove_containers_for "image_name"
remove_containers_for_version() {
    version="$1"

    if [ -z "$version" ]; then
        print_error "Image version required missing"
        return 1
    fi

    if [ -z "$CONTAINER_ENGINE" ]; then
        print_error "CONTAINER_ENGINE variable not set"
        return 1
    fi

    print_info "Removing containers for $version using $CONTAINER_ENGINE ..."

    _rcfv_removed=0

    # Remove containers using this version
    ids=$($SUDO_PREFIX$CONTAINER_ENGINE ps -a --filter "ancestor=$CONTAINER_BASE_FULL:$version" --format '{{.ID}}')
    if [ -n "$ids" ]; then
        echo "$ids" | xargs $SUDO_PREFIX$CONTAINER_ENGINE rm -f
        _rcfv_removed=$((_rcfv_removed + 1))
    fi
    ids=$($SUDO_PREFIX$CONTAINER_ENGINE ps -a --filter "ancestor=$CONTAINER_BASE_TINY:$version" --format '{{.ID}}')
    if [ -n "$ids" ]; then
        echo "$ids" | xargs $SUDO_PREFIX$CONTAINER_ENGINE rm -f
        _rcfv_removed=$((_rcfv_removed + 1))
    fi

    # Remove environment images for this version
    ids=$($SUDO_PREFIX$CONTAINER_ENGINE images --filter "reference=morloc-env:$version-*" --format '{{.ID}}')
    if [ -n "$ids" ]; then
        echo "$ids" | xargs $SUDO_PREFIX$CONTAINER_ENGINE rmi -f
        _rcfv_removed=$((_rcfv_removed + 1))
    fi

    # Remove base images
    if $SUDO_PREFIX$CONTAINER_ENGINE rmi -f "$CONTAINER_BASE_FULL:$version" >/dev/null 2>&1; then
        _rcfv_removed=$((_rcfv_removed + 1))
    fi
    if $SUDO_PREFIX$CONTAINER_ENGINE rmi -f "$CONTAINER_BASE_TINY:$version" >/dev/null 2>&1; then
        _rcfv_removed=$((_rcfv_removed + 1))
    fi

    if [ "$_rcfv_removed" -gt 0 ]; then
        print_success "Removed containers and images for $version"
    else
        print_info "No containers or images found for $version (already clean)"
    fi

}


remove_all_containers_and_images() {
    base_image="$1"

    if [ -z "$base_image" ]; then
        print_error "Base image name required"
        return 1
    fi

    print_info "Removing all containers and images for $base_image using $CONTAINER_ENGINE..."

    # Step 1: Remove all containers based on any tag of this base image
    print_info "Step 1: Removing containers..."
    # Get all image IDs for this base image (all tags)
    all_image_ids=$($SUDO_PREFIX$CONTAINER_ENGINE images --filter "reference=${base_image}:*" --format '{{.ID}}' 2>/dev/null)
    # For each image ID, find containers
    container_ids=""
    for img_id in $all_image_ids; do
        ids=$($SUDO_PREFIX$CONTAINER_ENGINE ps -a --filter "ancestor=$img_id" --format '{{.ID}}' 2>/dev/null)
        [ -n "$ids" ] && container_ids="$container_ids $ids"
    done

    if [ -n "$container_ids" ]; then
        print_info "Found containers: $container_ids"
        if $SUDO_PREFIX$CONTAINER_ENGINE rm -f $container_ids; then
            print_success "Containers removed successfully"
        else
            print_warning "Error removing containers"
            return 1
        fi
    else
        print_info "No containers found for $base_image"
    fi

    # Step 2: Find and remove all images with this base name (all tags)
    print_info "Step 2: Removing images (this may take a moment) ..."
    image_ids=$($SUDO_PREFIX$CONTAINER_ENGINE images --filter "reference=$base_image" --format '{{.ID}}' 2>/dev/null | sort -u)

    if [ -n "$image_ids" ]; then
        print_info "Found images: $image_ids"
        if $SUDO_PREFIX$CONTAINER_ENGINE rmi -f $image_ids; then
            print_success "Images removed successfully"
        else
            print_warning "Error removing images"
            return 1
        fi
    else
        print_info "No images found for $base_image"
    fi

    print_success "Cleanup complete for $base_image"
}


# Help for remove subcommand
show_uninstall_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") uninstall [OPTIONS] [VERSION]...

Remove Morloc home (or specific versions) and all associated containers.

${BOLD}OPTIONS${RESET}:
  -h, --help     Show this help message
  -a, --all      Remove all Morloc versions
      --system   Target system scope
      --local    Target local scope

${BOLD}ARGUMENTS${RESET}:
  VERSION        Version to remove, may specify multiple versions

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") uninstall --all
  $(basename "$0") uninstall 0.55.7
  sudo $(basename "$0") uninstall --system --all
EOF
}

cmd_uninstall() {
    version=""
    _uninst_scope="local"
    _uninst_err=0

    # Parse remove subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_uninstall_help
                exit 0
                ;;
            --system)
                _uninst_scope="system"
                MORLOC_SCOPE="system"
                SUDO_PREFIX="sudo "
                set_paths
                shift
                ;;
            --local)
                _uninst_scope="local"
                shift
                ;;
            -a|--all)
                _uninst_all_err=0

                # Read engines from all version configs BEFORE deleting anything
                _uninst_cfg_root_pre=$(config_root)
                [ "$_uninst_scope" = "system" ] && _uninst_cfg_root_pre=$(config_root --system)
                if [ -d "$_uninst_cfg_root_pre/versions" ]; then
                    for _uninst_vdir in "$_uninst_cfg_root_pre"/versions/*/; do
                        [ -d "$_uninst_vdir" ] || continue
                        _uninst_veng=$(read_config "container_engine" "$_uninst_vdir/config" 2>/dev/null)
                        if [ -n "$_uninst_veng" ] && command -v "$_uninst_veng" >/dev/null 2>&1; then
                            CONTAINER_ENGINE="$_uninst_veng"
                        fi
                    done
                fi

                morloc_home="$MORLOC_HOST_VERSION_DIR"
                if [ -d "$morloc_home" ]
                then
                    $SUDO_PREFIX rm -rf "$morloc_home"
                    if [ $? -ne 0 ]
                    then
                        # Container may have created root-owned files; retry with sudo
                        if [ -z "$SUDO_PREFIX" ]; then
                            print_info "Retrying removal with sudo (root-owned files from container)"
                            sudo rm -rf "$morloc_home"
                        fi
                        if [ -d "$morloc_home" ]; then
                            print_error "Failed to remove morloc home directory '$morloc_home'"
                            _uninst_all_err=1
                        else
                            print_success "Removed morloc home directory '$morloc_home'"
                        fi
                    else
                        print_success "Removed morloc home directory '$morloc_home'"
                    fi
                else
                    print_warning "Cannot remove morloc home directory '$morloc_home', it does not exist"
                fi

                # remove all containers/images for all Morloc tags
                remove_all_containers_and_images "$CONTAINER_BASE_FULL" || _uninst_all_err=1
                remove_all_containers_and_images "$CONTAINER_BASE_TINY" || _uninst_all_err=1

                # Remove custom environment images (morloc-env:*)
                if [ -n "$CONTAINER_ENGINE" ]; then
                    _uninst_env_imgs=$($SUDO_PREFIX$CONTAINER_ENGINE images --format '{{.Repository}}:{{.Tag}}' 2>/dev/null | grep '^morloc-env:' || true)
                    if [ -n "$_uninst_env_imgs" ]; then
                        print_info "Removing custom environment images..."
                        echo "$_uninst_env_imgs" | while IFS= read -r _uninst_img; do
                            $SUDO_PREFIX$CONTAINER_ENGINE rmi -f "$_uninst_img" 2>/dev/null || true
                        done
                        print_success "Removed custom environment images"
                    fi
                fi

                # Clean up config directories
                _uninst_cfg_root=$(config_root)
                [ "$_uninst_scope" = "system" ] && _uninst_cfg_root=$(config_root --system)
                if [ -d "$_uninst_cfg_root/versions" ]; then
                    $SUDO_PREFIX rm -rf "$_uninst_cfg_root/versions"
                fi

                # Clean up legacy files
                $SUDO_PREFIX rm -f "$MORLOC_DATA_HOME/.env"
                $SUDO_PREFIX rm -f "$MORLOC_DATA_HOME/morloc.flags"
                $SUDO_PREFIX rm -f "$MORLOC_DATA_HOME/docker-compose.yml"

                # Clean up environment flags
                if [ -d "$MORLOC_DEPENDENCY_DIR" ]; then
                    $SUDO_PREFIX rm -f "$MORLOC_DEPENDENCY_DIR"/*.flags
                fi

                # Clear active version in user config
                _uninst_user_cfg="$(config_root)/config"
                if [ -f "$_uninst_user_cfg" ]; then
                    write_config "active_version" "" "$_uninst_user_cfg" || _uninst_all_err=1
                    write_config "active_env" "base" "$_uninst_user_cfg" || _uninst_all_err=1
                fi

                # Clean up directories (use rm -rf for config root since it may contain stale config files)
                $SUDO_PREFIX rmdir "$MORLOC_DATA_HOME" 2>/dev/null || true
                $SUDO_PREFIX rm -rf "$_uninst_cfg_root" 2>/dev/null || true

                # Warn about system versions that were not removed
                if [ "$_uninst_scope" = "local" ]; then
                    _uninst_sys_versions=$(list_versions --system 2>/dev/null)
                    if [ -n "$_uninst_sys_versions" ]; then
                        print_warning "System-scope versions were NOT removed (run with --system --all and sudo to remove them):"
                        echo "$_uninst_sys_versions" | while IFS='	' read -r _sv_v _sv_s; do
                            print_point "$_sv_v"
                        done
                    fi
                fi

                exit $_uninst_all_err
                ;;
            -*)
                print_error "Unknown option for uninstall: $1"
                show_uninstall_help
                exit 1
                ;;
            *)
                version=$1
                # Remove data directory
                morloc_home="$MORLOC_HOST_VERSION_DIR/$version"
                if [ -d "$morloc_home" ]
                then
                    print_info "Morloc home '$morloc_home' found, deleting"
                    $SUDO_PREFIX rm -rf "$morloc_home"
                    if [ $? -ne 0 ]
                    then
                        # Container may have created root-owned files; retry with sudo
                        if [ -z "$SUDO_PREFIX" ]; then
                            print_info "Retrying removal with sudo (root-owned files from container)"
                            sudo rm -rf "$morloc_home"
                        fi
                        if [ -d "$morloc_home" ]; then
                            print_error "Failed to remove morloc home directory '$morloc_home'"
                            _uninst_err=1
                        else
                            print_success "Removed morloc directory '$morloc_home'"
                        fi
                    else
                        print_success "Removed morloc directory '$morloc_home'"
                    fi
                else
                    print_warning "Cannot remove morloc directory '$morloc_home', it does not exist"
                    _uninst_err=1
                fi

                # Read engine from version config before removing it
                _uninst_vcfg=$(version_config_root "$version" "$_uninst_scope")
                _uninst_engine=$(read_config "container_engine" "$_uninst_vcfg/config")
                [ -n "$_uninst_engine" ] && CONTAINER_ENGINE="$_uninst_engine"

                # Remove version config directory
                if [ -d "$_uninst_vcfg" ]; then
                    $SUDO_PREFIX rm -rf "$_uninst_vcfg"
                    print_success "Removed config directory '$_uninst_vcfg'"
                fi

                # Clear active version if it was the uninstalled one
                _uninst_active=$(active_version)
                if [ "$_uninst_active" = "$version" ]; then
                    if [ "$_uninst_scope" = "system" ]; then
                        write_config "active_version" "" "$(config_root --system)/config" --sudo || _uninst_err=1
                        write_config "active_env" "base" "$(config_root --system)/config" --sudo || _uninst_err=1
                    else
                        write_config "active_version" "" "$(config_root)/config" || _uninst_err=1
                        write_config "active_env" "base" "$(config_root)/config" || _uninst_err=1
                    fi
                fi

                remove_containers_for_version "$version"
                shift
                ;;
        esac
    done

    if [ -z "$version" ]; then
        print_error "No version given, to uninstall everything call with --all option"
        show_uninstall_help
        exit 1
    fi

    if [ "$_uninst_err" -ne 0 ]; then
        print_warning "Uninstall completed with errors"
        exit 1
    fi

    print_success "Removed containers and Morloc home, scripts remain"
}

# }}}
# {{{ clean subcommand

show_clean_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") clean [OPTIONS] [VERSION]

Remove build artifacts without removing container images.

${BOLD}OPTIONS${RESET}:
  -h, --help     Show this help message
      --all      Clean all version data
      --system   Target system scope
      --local    Target local scope
      --dry-run  List what would be deleted without deleting

${BOLD}ARGUMENTS${RESET}:
  VERSION        Clean runtime artifacts for a specific version

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") clean 0.55.0            Remove runtime data for 0.55.0
  $(basename "$0") clean --all             Remove all version data
  $(basename "$0") clean --all --dry-run   Show what would be removed
EOF
}

cmd_clean() {
    _cl_scope="local"
    _cl_dry_run=""
    _cl_all=""
    _cl_version=""

    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_clean_help
                exit 0
                ;;
            --system)
                _cl_scope="system"
                SUDO_PREFIX="sudo "
                shift
                ;;
            --local)
                _cl_scope="local"
                shift
                ;;
            --all)
                _cl_all=1
                shift
                ;;
            --dry-run)
                _cl_dry_run=1
                shift
                ;;
            -*)
                print_error "Unknown option for clean: $1"
                show_clean_help
                exit 1
                ;;
            *)
                if [ -n "$_cl_version" ]; then
                    print_error "Multiple versions not supported; use --all to clean everything"
                    exit 1
                fi
                _cl_version="$1"
                shift
                ;;
        esac
    done

    if [ -z "$_cl_all" ] && [ -z "$_cl_version" ]; then
        if [ -n "$_cl_dry_run" ]; then
            _cl_all=1
        else
            print_error "Specify --all or a VERSION to clean"
            show_clean_help
            exit 1
        fi
    fi

    _cl_scope_flag=""
    [ "$_cl_scope" = "system" ] && _cl_scope_flag="--system"
    _cl_versions_dir="$(data_root $_cl_scope_flag)/versions"

    # Helper: remove a path (or print it in dry-run mode)
    _clean_remove() {
        if [ ! -e "$1" ]; then
            return
        fi
        if [ -n "$_cl_dry_run" ]; then
            _sz=$(du -sh "$1" 2>/dev/null | cut -f1)
            print_info "[dry-run] would remove: $1 ($_sz)"
        else
            $SUDO_PREFIX rm -rf "$1"
            print_success "Removed $1"
        fi
    }

    # --all: clean every version data directory
    if [ -n "$_cl_all" ]; then
        if [ -d "$_cl_versions_dir" ]; then
            for _cl_d in "$_cl_versions_dir"/*/; do
                [ -d "$_cl_d" ] && _clean_remove "$_cl_d"
            done
        else
            print_info "No versions directory found at $_cl_versions_dir"
        fi
        if [ -z "$_cl_dry_run" ]; then
            print_success "All version data cleaned"
        fi
    fi

    # VERSION: clean a specific version's data
    if [ -n "$_cl_version" ]; then
        _cl_vdata=$(version_data_root "$_cl_version" "$_cl_scope")
        if [ -d "$_cl_vdata" ]; then
            _clean_remove "$_cl_vdata"
        else
            print_warning "No data found for version $_cl_version at $_cl_vdata"
        fi
    fi
}

# }}}
# {{{ update subcommand

# Help for update subcommand
show_update_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") update [OPTIONS]

Update this install script

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message
      --force          Allow downgrade to an older version

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") update
  $(basename "$0") update --force
EOF
}

# Compare two version strings (major.minor.patch format).
# Returns 0 (true) if $1 < $2, 1 otherwise.
_version_lt() {
    # Strip leading 'v' if present
    _vlt_a=$(printf '%s' "$1" | sed 's/^v//')
    _vlt_b=$(printf '%s' "$2" | sed 's/^v//')
    # Use sort -V if available, otherwise fall back to string comparison
    if printf '%s\n%s\n' "$_vlt_a" "$_vlt_b" | sort -V >/dev/null 2>&1; then
        _vlt_smallest=$(printf '%s\n%s\n' "$_vlt_a" "$_vlt_b" | sort -V | head -n1)
        [ "$_vlt_smallest" = "$_vlt_a" ] && [ "$_vlt_a" != "$_vlt_b" ]
    else
        # Fallback: simple string comparison
        [ "$_vlt_a" != "$_vlt_b" ] && [ "$(printf '%s\n%s\n' "$_vlt_a" "$_vlt_b" | sort | head -n1)" = "$_vlt_a" ]
    fi
}

cmd_update() {
    _upd_force=""

    # Parse update subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_update_help
                exit 0
                ;;
            --force)
                _upd_force=1
                shift
                ;;
            *)
                print_error "Unexpected argument: $1"
                show_update_help
                exit 1
                ;;
        esac
    done

    # Resolve absolute path for self-invocation (avoid "command not found" when
    # the script is invoked via a relative path not in $PATH)
    _upd_self="$0"
    case "$_upd_self" in
        /*) ;; # already absolute
        *)  _upd_self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")" ;;
    esac

    old_version=$(sh "$_upd_self" --version 2>/dev/null)
    if [ $? -ne 0 ]; then
      print_info "No current version detected"
      old_version=""
    else
      print_info "Current version: $old_version"
    fi

    if command -v mktemp >/dev/null 2>&1; then
        tmp_script=$(mktemp "/tmp/${PROGRAM_NAME}.XXXXXX")
    else
        tmp_script="/tmp/${PROGRAM_NAME}.$$"
    fi

    WGET_PATH=$(command -v wget 2>/dev/null || true)
    CURL_PATH=$(command -v curl 2>/dev/null || true)

    if [ -n "$WGET_PATH" ] && [ -x "$WGET_PATH" ]; then
      print_info "Checking for latest $PROGRAM_NAME script (using wget)"
      "$WGET_PATH" -q -O "$tmp_script" "$THIS_SCRIPT_URL"
      download_rc=$?
    elif [ -n "$CURL_PATH" ] && [ -x "$CURL_PATH" ]; then
      print_info "Checking for latest $PROGRAM_NAME script (using curl)"
      "$CURL_PATH" -fsSL -o "$tmp_script" "$THIS_SCRIPT_URL"
      download_rc=$?
    else
      print_error "Please install either wget or curl"
      rm -f "$tmp_script"
      exit 1
    fi

    if [ "$download_rc" -ne 0 ]
    then
        print_error "Failed to retrieve script from '$THIS_SCRIPT_URL'"
        rm -f "$tmp_script"
        exit 1
    fi

    nlinesdiff=$(diff "$tmp_script" "$_upd_self" | wc -l)
    if [ "$nlinesdiff" -ne 0 ]
    then
        print_info "Successfully pulled '$THIS_SCRIPT_URL'"
    else
        print_info "You are already using the latest version"
        rm -f "$tmp_script"
        exit 0
    fi

    print_info "Making script executable"
    chmod 755 "$tmp_script"
    if [ $? -ne 0 ]
    then
        print_error "Failed to make new script executable, exiting"
        rm -f "$tmp_script"
        exit 1
    fi

    new_version=$(sh "$tmp_script" --version 2>/dev/null)

    # Refuse to downgrade unless --force is given
    if [ -n "$old_version" ] && [ -n "$new_version" ] && [ -z "$_upd_force" ]; then
        if _version_lt "$new_version" "$old_version"; then
            print_warning "Remote version ($new_version) is older than current ($old_version)"
            print_info "Use 'update --force' to downgrade"
            rm -f "$tmp_script"
            exit 1
        fi
    fi

    print_info "Replacing current script at '$_upd_self'"
    mv "$tmp_script" "$_upd_self"
    if [ $? -ne 0 ]
    then
        print_error "Failed to replace current script, exiting"
        rm -f "$tmp_script"
        exit 1
    fi

    if [ -z "$old_version" ]; then
      print_success "Updated to $new_version"
    else
      print_success "Updated from $old_version to $new_version"
    fi
}
# }}}
# {{{ select subcommand

# Help for select subcommand
show_select_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") select [OPTIONS] <version>

Set active Morloc version.

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message
      --system         Force system scope
      --local          Force local scope

${BOLD}ARGUMENTS${RESET}:
  version        Version to activate

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") select 0.54.2
  $(basename "$0") select --system 0.54.2
EOF
}

cmd_select() {
    # Check for --help before sudo guard (help doesn't write anything)
    for _sel_arg in "$@"; do
        case "$_sel_arg" in
            -h|--help) show_select_help; exit 0 ;;
        esac
    done

    # select only writes to user config — sudo would write to root's config
    if [ "$(id -u)" = "0" ] && [ -n "${SUDO_USER:-}" ]; then
        print_error "Do not run 'select' with sudo — it writes to the real user's config, not root's"
        print_info "Run without sudo: $(basename "$0") select $*"
        exit 1
    fi

    version="undefined"
    _sel_scope=""
    _sel_explicit_scope=""

    # Parse select subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_select_help
                exit 0
                ;;
            --system)
                _sel_scope="system"
                _sel_explicit_scope="system"
                shift
                ;;
            --local)
                _sel_scope="local"
                _sel_explicit_scope="local"
                shift
                ;;
            *)
                if [ "$version" = "undefined" ]; then
                    version="$1"
                else
                    print_error "Multiple version installation not supported: $1"
                    exit 1
                fi
                shift
                ;;
        esac
    done

    if [ "$version" = "$LOCAL_VERSION" ]
    then
        print_error "Cannot select reserved version '${LOCAL_VERSION}'"
        exit 1
    fi

    if [ "$version" = "undefined" ]
    then
        print_error "Please select a version"
        # List available versions with scope annotations
        print_info "Available versions:"
        list_versions --all | while IFS='	' read -r _sv_v _sv_s; do
            print_point "$_sv_v ($_sv_s)"
        done
        show_select_help
        exit 1
    fi

    # Resolve where the version actually lives
    _sel_resolved=$(resolve_version "$version") || true

    if [ -z "$_sel_resolved" ]; then
        print_error "Morloc version '$version' is not installed"
        print_info "Run: $(basename "$0") install $version"
        exit 1
    fi

    # If scope was forced, verify the version exists in that scope
    if [ -n "$_sel_scope" ] && [ "$_sel_scope" != "$_sel_resolved" ]; then
        _sel_vdata=$(version_data_root "$version" "$_sel_scope")
        if [ ! -d "$_sel_vdata" ]; then
            print_error "Morloc version '$version' is installed in $_sel_resolved scope, not $_sel_scope"
            print_info "Run: $(basename "$0") select --$_sel_resolved $version"
            exit 1
        fi
    fi

    # Use resolved scope if not forced
    [ -z "$_sel_scope" ] && _sel_scope="$_sel_resolved"

    # Write active version to user config
    _sel_user_cfg="$(config_root)/config"
    if ! write_config "active_scope" "$_sel_scope" "$_sel_user_cfg"; then
        print_error "Failed to write active scope config"
        exit 1
    fi
    if [ "$_sel_scope" = "system" ] && [ -z "$_sel_explicit_scope" ]; then
        # Don't overwrite local active_version with a system-only version
        :
    else
        if ! write_config "active_version" "$version" "$_sel_user_cfg"; then
            print_error "Failed to write active version config"
            exit 1
        fi
    fi

    # Reset active env when switching versions (env images are version-tagged)
    _sel_prev_version=$(read_config "active_version" "$_sel_user_cfg")
    _sel_prev_env=$(read_config "active_env" "$_sel_user_cfg")
    if [ -n "$_sel_prev_env" ] && [ "$_sel_prev_env" != "base" ]; then
        if ! write_config "active_env" "base" "$_sel_user_cfg"; then
            print_error "Failed to reset active environment config"
            exit 1
        fi
        print_warning "Reset active environment to 'base' (rebuild '$_sel_prev_env' for version $version with: $(basename "$0") env $_sel_prev_env)"
    fi

    print_success "Switched to Morloc version '$version' ($_sel_scope)"
    exit 0
}

# }}}
# {{{ info subcommand

# Help for info subcommand
show_info_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") info [OPTIONS]

Print info on Morloc versions and check containers.

${BOLD}OPTIONS${RESET}:
  -h, --help   Show this help message
      --system Show system-scope info
      --local  Show local-scope info

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") info
  $(basename "$0") info --system
EOF
}

cmd_info() {

    _info_scope_override=""

    # Parse info subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_info_help
                exit 0
                ;;
            --system)
                _info_scope_override="system"
                MORLOC_SCOPE="system"
                SUDO_PREFIX="sudo "
                set_paths
                shift
                ;;
            --local)
                _info_scope_override="local"
                MORLOC_SCOPE="local"
                SUDO_PREFIX=""
                set_paths
                shift
                ;;
            *)
                print_error "Unexpected argument: $1"
                show_info_help
                exit 1
                ;;
        esac
    done

    # Read from structured config; honor explicit --system/--local override
    if [ -n "$_info_scope_override" ]; then
        _info_scope="$_info_scope_override"
        # Read version from the overridden scope's config
        if [ "$_info_scope" = "system" ]; then
            _info_version=$(read_config "active_version" "$(config_root --system)/config")
            _info_env=$(read_config "active_env" "$(config_root --system)/config")
        else
            _info_version=$(read_config "active_version" "$(config_root)/config")
            _info_env=$(read_config "active_env" "$(config_root)/config")
        fi
    else
        _info_version=$(active_version)
        _info_scope=$(active_scope)
        _info_env=$(read_config "active_env")
    fi
    [ -z "$_info_version" ] && _info_version="none"
    [ -z "$_info_env" ] && _info_env="base"

    # Read engine from version config, fall back to auto-detected or "none"
    if [ "$_info_version" != "none" ]; then
        _info_vcfg=$(version_config_root "$_info_version" "$_info_scope")
        _info_engine=$(read_config "container_engine" "$_info_vcfg/config")
    else
        _info_engine=""
    fi
    [ -z "$_info_engine" ] && _info_engine="${CONTAINER_ENGINE:-none}"

    # Display scope and SELinux info
    detect_selinux
    if [ "$_info_scope" = "system" ]; then
        printf "Scope:          system\n"
    else
        printf "Scope:          local\n"
    fi
    if [ -n "$SELINUX_SUFFIX" ]; then
        printf "SELinux:        enforcing (bind mounts use :z)\n"
    else
        printf "SELinux:        not detected\n"
    fi
    _info_scope_flag=""
    [ "$_info_scope" = "system" ] && _info_scope_flag="--system"

    printf "Active version: %s\n" "$_info_version"
    printf "Active scope:   %s\n" "$_info_scope"
    printf "Active env:     %s\n" "$_info_env"
    printf "Engine:         %s\n" "$_info_engine"
    printf "Config root:    %s\n" "$(config_root $_info_scope_flag)"
    printf "Data root:      %s\n" "$(data_root $_info_scope_flag)"
    printf "Bin dir:        %s\n" "$(bin_root $_info_scope_flag)"

    # Show local versions
    printf "\nLocal versions:\n"
    _info_local=$(list_versions --local)
    if [ -n "$_info_local" ]; then
        echo "$_info_local" | while IFS='	' read -r _iv_v _iv_s; do
            _iv_mark=""
            [ "$_iv_v" = "$_info_version" ] && [ "$_info_scope" = "local" ] && _iv_mark=" (active)"
            printf "  %s%s\n" "$_iv_v" "$_iv_mark"
        done
    else
        printf "  (none)\n"
    fi

    # Show system versions
    printf "\nSystem versions:\n"
    _info_sys=$(list_versions --system)
    if [ -n "$_info_sys" ]; then
        echo "$_info_sys" | while IFS='	' read -r _iv_v _iv_s; do
            _iv_mark=""
            [ "$_iv_v" = "$_info_version" ] && [ "$_info_scope" = "system" ] && _iv_mark=" (active)"
            printf "  %s%s\n" "$_iv_v" "$_iv_mark"
        done
    else
        printf "  (none)\n"
    fi

    exit 0
}
# }}}
# {{{ env subcommand

update_environment() {
  envname=$1; shift
  extra_args=${1:-}
  envfile="$MORLOC_DEPENDENCY_DIR/$envname.Dockerfile"

  print_info "Attempting to switch environment to ${envname} with ${envfile}"

  if [ -e "$envfile" ]; then
    print_info "$envfile found, attempting to build"
  else
    print_error "$envfile not found, please create and retry"
    return 1
  fi

  # Read current version and scope from config
  version=$(active_version)
  _ue_scope=$(active_scope)
  if [ -z "$version" ]; then
      print_error "No active version — run 'install' first"
      return 1
  fi
  print_info "Currently using morloc v$version"

  _ue_vcfg=$(version_config_root "$version" "$_ue_scope")
  _ue_sudo=""
  [ "$_ue_scope" = "system" ] && _ue_sudo="--sudo"

  # Use the engine from version config to avoid mismatch on dual-engine systems,
  # but CLI --container-engine always takes precedence
  if [ -z "$CONTAINER_ENGINE_CLI_OVERRIDE" ]; then
      _ue_engine=$(read_config "container_engine" "$_ue_vcfg/config")
      [ -n "$_ue_engine" ] && CONTAINER_ENGINE="$_ue_engine"
  fi

  base_container="${CONTAINER_BASE_FULL}:${version}"
  user_container="morloc-env:${version}-${envname}"
  build_environment "$envname" "$envfile" "$user_container" "$base_container" "$extra_args" || return $?
  write_config "image" "$user_container" "$_ue_vcfg/config" $_ue_sudo || return 1
  print_success "Switched environment to $version-$envname"

  # Write environment config file to version-specific environments dir
  if [ "$_ue_scope" = "system" ]; then
      if ! sudo mkdir -p "$_ue_vcfg/environments"; then
          print_error "Failed to create environments directory"
          return 1
      fi
  else
      if ! mkdir -p "$_ue_vcfg/environments"; then
          print_error "Failed to create environments directory"
          return 1
      fi
  fi
  _ue_env_conf="$_ue_vcfg/environments/${envname}.conf"
  write_config "image" "$user_container" "$_ue_env_conf" $_ue_sudo || return 1

  # Copy flags file to version environments dir if it exists
  flags_file="$MORLOC_DEPENDENCY_DIR/${envname}.flags"
  if [ -f "$flags_file" ]; then
      if [ "$_ue_scope" = "system" ]; then
          if ! sudo cp "$flags_file" "$_ue_vcfg/environments/${envname}.flags"; then
              print_error "Failed to copy environment flags file"
              return 1
          fi
      else
          if ! cp "$flags_file" "$_ue_vcfg/environments/${envname}.flags"; then
              print_error "Failed to copy environment flags file"
              return 1
          fi
      fi
      print_info "Activated environment flags: $flags_file"
  fi

  # Set active environment in the correct scope config
  if [ "$_ue_scope" = "system" ]; then
      write_config "active_env" "$envname" "$(config_root --system)/config" --sudo || return 1
  else
      write_config "active_env" "$envname" "$(config_root)/config" || return 1
  fi

  return 0
}

reset_environment() {
  version=$(active_version)
  _re_scope=$(active_scope)
  if [ -z "$version" ]; then
      print_error "No active version — nothing to reset"
      return 1
  fi
  print_info "Currently using morloc v$version"

  _re_vcfg=$(version_config_root "$version" "$_re_scope")
  _re_sudo=""
  [ "$_re_scope" = "system" ] && _re_sudo="--sudo"

  write_config "image" "${CONTAINER_BASE_FULL}:${version}" "$_re_vcfg/config" $_re_sudo || return 1
  print_success "Successfully reset environment to default"

  # Reset active env to base in the correct scope config
  if [ "$_re_scope" = "system" ]; then
      write_config "active_env" "base" "$(config_root --system)/config" --sudo || return 1
  else
      write_config "active_env" "base" "$(config_root)/config" || return 1
  fi
  print_info "Reset active environment to base"

  return 0
}

list_local_environment() {

    current_env=$(read_config "active_env")

    printf "Available environments:\n"

    # Always show base environment first
    if [ "$current_env" = "base" ] || [ -z "$current_env" ]; then
        printf "  * %-20s (active)\n" "base"
    else
        printf "    %-20s\n" "base"
    fi

    # Check if custom environments directory exists
    if [ ! -d "$MORLOC_DEPENDENCY_DIR" ]; then
        return 0
    fi

    # List all .Dockerfile files
    for file in "$MORLOC_DEPENDENCY_DIR"/*.Dockerfile; do
        if [ -e "$file" ]; then
            _le_base="${file##*/}"
            _le_base="${_le_base%.Dockerfile}"
            if [ "$_le_base" = "$current_env" ]; then
                printf "  * %-20s (active)\n" "$_le_base"
            else
                printf "    %-20s\n" "$_le_base"
            fi
        fi
    done
}

init_environment() {
    envname="$1"
    envfile="$MORLOC_DEPENDENCY_DIR/$1.Dockerfile"

    # if MORLOC_DEPENDENCY_DIR does not exist, create the directory
    $SUDO_PREFIX mkdir -p "$MORLOC_DEPENDENCY_DIR"

    if [ -e "$envfile" ]; then
        print_error "Cannot create $envfile, file already exists"
        exit 1
    fi

    $SUDO_PREFIX tee "$envfile" > /dev/null << EOF
# Automatically generated section, DO NOT MODIFY
# ----------------------------------------------
ARG CONTAINER_BASE=scratch
FROM \${CONTAINER_BASE}
LABEL morloc.environment="$envname"
ENV MORLOC_ENV_NAME="$envname"
# End of automatically generated section
# ----------------------------------------------

# Add custom setup below this line
EOF

    print_success "Created stub Dockerfile at $envfile, edit as needed"

    # Create flags file stub
    flagsfile="$MORLOC_DEPENDENCY_DIR/$envname.flags"
    if [ ! -e "$flagsfile" ]; then
        $SUDO_PREFIX tee "$flagsfile" > /dev/null << EOF
# Extra docker/podman flags for the '$envname' environment
# Lines starting with # are comments. Each non-empty line is passed as
# an argument to 'docker run' / 'podman run'.
#
# Examples:
#   --gpus all
#   -v /data/datasets:/data:ro
#   -p 8888:8888
#   -e MY_VAR=value
EOF
        print_success "Created stub flags file at $flagsfile, edit as needed"
    fi

    exit 0
}

# Help for env subcommand
show_env_help() {
    _eh_dep_display=$(printf '%s' "$MORLOC_DEPENDENCY_DIR" | sed "s|^$HOME|~|")
    _eh_data_display=$(printf '%s' "$MORLOC_DATA_HOME" | sed "s|^$HOME|~|")
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") env [OPTIONS] [ENV]

Select an environment. An environment is a Dockerfile that builds on a
version-specific morloc image, plus a .flags file with extra docker/podman
flags (volumes, ports, GPUs, etc.).

Files live in ${_eh_dep_display}:
  <name>.Dockerfile   What is inside the container (packages)
  <name>.flags        How the container connects to the host (volumes, ports, etc.)

A global ${_eh_data_display}/morloc.flags applies to ALL runs. At runtime,
global flags are applied first, then environment flags appended.

${BOLD}OPTIONS${RESET}:
  -h, --help      Show this help message
      --list      List all locally defined environments
      --init ENV  Create stub Dockerfile and flags file
      --reset     Reset to the default environment (clears env flags)
  -x, --extra ARG Extra arguments for the container

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") env --list
  $(basename "$0") env --init ml
  $(basename "$0") env ml
  $(basename "$0") env --reset
EOF
}

cmd_env() {
    # Parse env subcommand arguments
    env=""
    reset="false"
    extra_args=""

    # Resolve scope from active install — env always operates on the active version
    _ce_scope=$(active_scope)
    if [ "$_ce_scope" = "system" ]; then
        MORLOC_SCOPE="system"
        SUDO_PREFIX="sudo "
        set_paths
    fi

    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_env_help
                exit 0
                ;;
            --list)
                list_local_environment
                exit 0
                ;;
            --init)
                shift
                if [ -z "${1:-}" ]; then
                    print_error "Missing environment name for --init"
                    show_env_help
                    exit 1
                fi
                init_environment "$1"
                exit 0
                ;;
            --reset)
                shift
                reset="true"
                ;;
            -x|--extra)
                shift
                extra_args="${extra_args}${1} "
                shift
                ;;
            -*)
                print_error "Unexpected argument"
                show_env_help
                exit 1
                ;;
            *)
                if [ -z "$env" ]; then
                    env="$1"
                    shift
                else
                    print_error "Nested environments are not supported"
                    exit 1
                fi
                ;;
        esac
    done

    if [ "$reset" = "true" ]; then
        if [ -n "$env" ]; then
            print_warning "Ignoring environment name '$env' with --reset"
        fi
        reset_environment
        exit $?
    else
        if [ -z "$env" ]; then
          print_error "No environment specified"
          show_env_help
          exit 1
        else
          update_environment "$env" "$extra_args"
          exit $?
        fi
    fi
}
# }}}
# {{{ main

# Main argument parsing

main() {
    # Parse global options
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_help
                exit 0
                ;;
            -v|--version)
                show_version
                exit 0
                ;;
            --container-engine)
                shift
                set_container_engine "${1:?'--container-engine requires an argument'}"
                shift
                ;;
            -*)
                print_error "Unknown option: $1"
                show_help
                exit 1
                ;;
            *)
                break
                ;;
        esac
    done

    set_paths
    detect_selinux

    # Dispatch subcommand
    case "${1:-}" in
        install)   shift; cmd_install "$@" ;;
        uninstall) shift; cmd_uninstall "$@" ;;
        clean)     shift; cmd_clean "$@" ;;
        update)    shift; cmd_update "$@" ;;
        select)    shift; cmd_select "$@" ;;
        run)       shift; cmd_run "$@" ;;
        env)       shift; cmd_env "$@" ;;
        info)      shift; cmd_info "$@" ;;
        shell)     print_info "'shell' has been replaced — use: $(basename "$0") run --shell"; exit 0 ;;
        "")        show_help; exit 0 ;;
        *)         print_error "Unknown command: $1"; show_help; exit 1 ;;
    esac
}

# }}}

# Run main unless sourced for testing
if [ "${MORLOC_MANAGER_TESTING:-}" != "1" ]; then
    main "$@"
fi
