#!/usr/bin/env sh

# Morloc Installation Manager

# {{{ constants and system info

PROGRAM_NAME="morloc-manager"
VERSION="0.7.1"

CONTAINER_ENGINE_VERSION=""
CONTAINER_ENGINE=""

SHARED_MEMORY_SIZE=4g

CONTAINER_BASE_FULL=ghcr.io/morloc-project/morloc/morloc-full
CONTAINER_BASE_TINY=ghcr.io/morloc-project/morloc/morloc-tiny
CONTAINER_BASE_TEST=ghcr.io/morloc-project/morloc/morloc-test

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

set_container_engine() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "[ERROR] Container engine '$1' not found" >&2
        exit 1
    fi
    CONTAINER_ENGINE="$1"
    CONTAINER_ENGINE_VERSION=$($CONTAINER_ENGINE --version 2>/dev/null | sed 's/.*version \([0-9.]*\).*/\1/')
}

# location of modules and other data will be stored for all morloc versions
MORLOC_DATA_HOME=${XDG_DATA_HOME:-~/.local/share}/morloc

# location of global morloc config and version specific configs will be stored
MORLOC_CONFIG_HOME=${XDG_CONFIG_HOME:-~/.config}/morloc

# location of all program state may be stored (may always be safely deleted
# when programs are not running)
MORLOC_STATE_HOME=${XDG_STATE_HOME:-~/.local/state}/morloc

# location of all cached data for morloc programs
MORLOC_CACHE_HOME=${XDG_CACHE_HOME:-~/.cache}/morloc

MORLOC_DEPENDENCY_DIR="$HOME/.local/share/morloc/deps"

# Derive a relative path from HOME for use in mount paths
case "$MORLOC_DATA_HOME" in
    "$HOME"/*)
        MORLOC_DATA_RELDIR="${MORLOC_DATA_HOME#$HOME/}"
        ;;
    *)
        # Non-standard XDG_DATA_HOME; fall back to default relative path
        printf "[WARNING] XDG_DATA_HOME is outside \$HOME; using default ~/.local/share/morloc for container mounts\n" >&2
        MORLOC_DATA_RELDIR=".local/share/morloc"
        ;;
esac
MORLOC_INSTALL_DIR="${MORLOC_DATA_RELDIR}/versions"
MORLOC_LIBRARY_RELDIR="src/modules"
MORLOC_DEFAULT_PLANE="default"
MORLOC_DEFAULT_PLANE_GITHUB_ORG="morloclib"

# Configuration for setting up executable folder
MORLOC_BIN_BASENAME=".local/bin"
MORLOC_BIN="$HOME/$MORLOC_BIN_BASENAME"
PATH_EXPORT_LINE="export PATH=\"${MORLOC_BIN}:\$PATH\""
COMMENT_LINE="# For Morloc support"

LOCAL_VERSION="local"

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
                RED='\033[0;31m'
                GREEN='\033[0;32m'
                YELLOW='\033[0;33m'
                BLUE='\033[0;34m'
                MAGENTA='\033[0;35m'
                CYAN='\033[0;36m'

                # Text attributes
                BOLD='\033[1m'
                DIM='\033[2m'
                UNDERLINE='\033[4m'
                REVERSE='\033[7m'
                BLINK='\033[5m'

                RESET='\033[0m'
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
    if ! mkdir -p "$DIR" 2>/dev/null; then
        print_error "Failed to create directory: $DIR"
        return 1
    fi

    print_success "Created directory: $DIR"
    return 0
}


# Function to detect the current shell
detect_shell() {
    # First, check if we're running under a specific shell using version variables
    if [ -n "$ZSH_VERSION" ]; then
        echo "zsh"
    elif [ -n "$BASH_VERSION" ]; then
        echo "bash"
    elif [ -n "$FISH_VERSION" ]; then
        echo "fish"
    elif [ -n "$KSH_VERSION" ]; then
        echo "ksh"
    # Check for tcsh/csh specific variables
    elif [ -n "$tcsh" ] || [ -n "$version" ]; then
        if [ -n "$tcsh" ]; then
            echo "tcsh"
        else
            echo "csh"
        fi
    # Check SHELL environment variable
    elif [ -n "$SHELL" ]; then
        case "$(basename "$SHELL")" in
            *zsh*) echo "zsh" ;;
            *bash*) echo "bash" ;;
            *fish*) echo "fish" ;;
            *ksh*) echo "ksh" ;;
            *tcsh*) echo "tcsh" ;;
            *csh*) echo "csh" ;;
            *dash*) echo "dash" ;;
            *ash*) echo "ash" ;;
            *) basename "$SHELL" ;;
        esac
    # Last resort: check process name
    else
        # Try to get process name from ps (with fallback)
        if command -v ps >/dev/null 2>&1; then
            shell_name=$(ps -p $$ -o comm= 2>/dev/null | sed 's/^-//' || echo "sh")
            case "$shell_name" in
                *zsh*) echo "zsh" ;;
                *bash*) echo "bash" ;;
                *fish*) echo "fish" ;;
                *ksh*) echo "ksh" ;;
                *tcsh*) echo "tcsh" ;;
                *csh*) echo "csh" ;;
                *dash*) echo "dash" ;;
                *ash*) echo "ash" ;;
                *) echo "$shell_name" ;;
            esac
        else
            echo "sh"
        fi
    fi
}

# Function to get appropriate shell configuration files
get_shell_config_files() {
    local shell_name
    shell_name=$(detect_shell)

    case "$shell_name" in
        bash)
            # macOS typically uses .bash_profile, Linux uses .bashrc
            # Check in order of preference for login shells
            if [ "$(uname -s)" = "Darwin" ]; then
                # macOS prefers .bash_profile for login shells
                if [ -f "$HOME/.bash_profile" ]; then
                    echo "$HOME/.bash_profile"
                elif [ -f "$HOME/.bashrc" ]; then
                    echo "$HOME/.bashrc"
                else
                    echo "$HOME/.bash_profile"
                fi
            else
                # Linux and others: prefer .bashrc
                if [ -f "$HOME/.bashrc" ]; then
                    echo "$HOME/.bashrc"
                elif [ -f "$HOME/.bash_profile" ]; then
                    echo "$HOME/.bash_profile"
                else
                    echo "$HOME/.bashrc"
                fi
            fi
            ;;
        zsh)
            echo "$HOME/.zshrc"
            ;;
        fish)
            # Ensure fish config directory exists
            if [ ! -d "$HOME/.config/fish" ]; then
                mkdir -p "$HOME/.config/fish" 2>/dev/null || true
            fi
            echo "$HOME/.config/fish/config.fish"
            ;;
        ksh)
            # Korn shell typically uses .kshrc or .profile
            if [ -f "$HOME/.kshrc" ]; then
                echo "$HOME/.kshrc"
            else
                echo "$HOME/.profile"
            fi
            ;;
        dash|ash)
            # dash and ash are usually non-interactive, but if used as login shell
            # they typically source .profile
            echo "$HOME/.profile"
            ;;
        tcsh)
            echo "$HOME/.tcshrc"
            ;;
        csh)
            echo "$HOME/.cshrc"
            ;;
        *)
            # For other shells, use .profile (most portable)
            echo "$HOME/.profile"
            ;;
    esac
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

# Function to check if PATH export already exists in a file
path_exists_in_file() {
    local file="$1"
    if [ -f "$file" ] && [ -r "$file" ]; then
        # Use more specific pattern to avoid false positives
        if grep -q "$MORLOC_BIN_BASENAME" "$file" 2>/dev/null; then
            return 0
        fi
    fi
    return 1
}

# Function to safely add PATH export to configuration file (handles multiple shells)
add_to_config_file() {
    local config_file="$1"
    local config_dir
    local shell_name
    shell_name=$(detect_shell)
    config_dir=$(dirname "$config_file")

    # Create config directory if it doesn't exist
    if [ ! -d "$config_dir" ]; then
        print_info "Creating configuration directory: $config_dir"
        if ! mkdir -p "$config_dir" 2>/dev/null; then
            print_error "Failed to create directory: $config_dir"
            return 1
        fi
    fi

    # Check if PATH export already exists
    if path_exists_in_file "$config_file"; then
        print_warning "PATH export for ~/$MORLOC_BIN_BASENAME already exists in $config_file"
        return 0
    fi

    # Add the appropriate PATH export based on shell
    case "$shell_name" in
        fish)
            # Fish shell uses different syntax
            {
                echo ""
                echo "# Added by Morloc setup script"
                echo "set -gx PATH \$HOME/$MORLOC_BIN_BASENAME \$PATH"
            } >> "$config_file" 2>/dev/null || {
                print_error "Failed to write to $config_file"
                return 1
            }
            print_success "Added Fish-compatible PATH export to $config_file"
            ;;
        tcsh|csh)
            # C shell family uses different syntax
            {
                echo ""
                echo "# Added by Morloc setup script"
                echo "set path = (\$HOME/$MORLOC_BIN_BASENAME \$path)"
            } >> "$config_file" 2>/dev/null || {
                print_error "Failed to write to $config_file"
                return 1
            }
            print_success "Added C shell-compatible PATH export to $config_file"
            ;;
        *)
            # POSIX-compatible shells (bash, zsh, sh, dash, ash, ksh, etc.)
            {
                echo ""
                echo "$COMMENT_LINE"
                echo "$PATH_EXPORT_LINE"
            } >> "$config_file" 2>/dev/null || {
                print_error "Failed to write to $config_file"
                return 1
            }
            print_success "Added POSIX-compatible PATH export to $config_file"
            ;;
    esac

    return 0
}

# Function to source the configuration file (shell-aware)
source_config_file() {
    local config_file="$1"
    local shell_name
    shell_name=$(detect_shell)

    print_info "Sourcing configuration file to update current PATH..."

    # Handle shells that don't support sourcing or have different syntax
    case "$shell_name" in
        fish)
            print_info "Fish shell detected - PATH will be available in new fish sessions"
            print_info "To update current session: exec fish"
            return 0
            ;;
        tcsh|csh)
            print_info "C shell detected - PATH will be available in new shell sessions"
            print_info "To update current session: source \"$config_file\""
            # Try to source with csh syntax, but don't fail if it doesn't work
            # shellcheck disable=SC1090
            if [ -f "$config_file" ] && command -v source >/dev/null 2>&1; then
                source "$config_file" 2>/dev/null || true
            fi
            return 0
            ;;
        *)
            # POSIX-compatible shells (bash, zsh, sh, dash, ash, ksh, etc.)
            # Add to PATH directly instead of sourcing the full config file,
            # which can have side effects (override variables, produce output, etc.)
            export PATH="$MORLOC_BIN:$PATH"

            if is_in_path "$MORLOC_BIN"; then
                print_success "$MORLOC_BIN is now in your current PATH"
            else
                print_warning "PATH update may not have taken effect immediately"
                print_warning "Try opening a new terminal if the directory isn't accessible"
            fi
            ;;
    esac
}

# }}}
# {{{ setup Morloc bin folder

# Function to test PATH functionality
test_path_functionality() {
    # Use a more unique test filename to avoid conflicts
    local timestamp
    local test_script
    local test_command

    # Get timestamp in a portable way
    if command -v date >/dev/null 2>&1; then
        timestamp=$(date +%s 2>/dev/null || echo "$$")
    else
        timestamp="$$"
    fi

    test_script="$MORLOC_BIN/path-test-$timestamp"
    test_command="path-test-$timestamp"

    print_info "Testing PATH functionality..."

    # Create a simple test script with error handling
    if ! cat > "$test_script" << 'EOF' 2>/dev/null
#!/usr/bin/env sh
echo "PATH test successful!"
exit 0
EOF
    then
        print_error "Failed to create test script"
        return 1
    fi

    # Make it executable with error handling
    if ! chmod +x "$test_script" 2>/dev/null; then
        print_error "Failed to make test script executable"
        rm -f "$test_script" 2>/dev/null || true
        return 1
    fi

    # Test if we can run it by name (proving it's in PATH)
    if command -v "$test_command" >/dev/null 2>&1 && "$test_command" >/dev/null 2>&1; then
        print_success "PATH test passed - executable files in ~/$MORLOC_BIN_BASENAME are accessible"
        rm -f "$test_script" 2>/dev/null || true
        return 0
    else
        print_warning "PATH test failed - executable may not be immediately accessible"
        print_info "This sometimes happens due to shell caching - try opening a new terminal"
        rm -f "$test_script" 2>/dev/null || true
        return 1
    fi
}

# Main function
add_morloc_bin_to_path() {

    ### Configuration ####

    # Show current status
    print_info "Setting up Morloc bin:"

    morloc_bin_exists=$( if [ -d "$MORLOC_BIN" ]; then echo 0; else echo 1; fi )
    morloc_bin_is_in_path=$( if is_in_path "$MORLOC_BIN"; then echo 0; else echo 1; fi )

    printf "  Target Morloc bin folder: %s " "$MORLOC_BIN"

    if [ $morloc_bin_exists = 0 ]; then
        printf "%s[EXISTS]%s\n" "$GREEN" "$RESET"
    else
        printf "%s[MISSING]%s\n" "$RED" "$RESET"
    fi

    printf "  In current PATH? "
    if [ $morloc_bin_is_in_path = 0 ]; then
        printf "%s[YES]%s\n" "$GREEN" "$RESET"
    else
        printf "%s[NO]%s\n" "$RED" "$RESET"
    fi

    if [ $morloc_bin_exists = 0 ]; then
        if [ $morloc_bin_is_in_path = 0 ]; then
            printf "  %s[OK] All systems go!%s\n" "$GREEN" "$RESET"
            return 0
        fi
    fi

    local shell_name
    shell_name=$(detect_shell)

    local config_file
    config_file=$(get_shell_config_files)

    local operating_system
    operating_system=$(uname -s)

    printf "  Detected shell: %s\n" "${shell_name}"
    printf "  Configuration file: %s\n" "${config_file}"
    printf "  Operating system: %s\n" "${operating_system}"
    echo ""

    printf "%sThis script will:%s\n" "$YELLOW" "$RESET"
    echo "  1. Create directory: $MORLOC_BIN"
    echo "  2. Add PATH export to config file: $config_file"
    echo "  3. Source the config file to update current PATH"
    echo "  4. Test PATH functionality with a sample executable"
    echo "  5. Make ~/${MORLOC_BIN_BASENAME} available immediately and in future sessions"
    echo ""

    ### Confirmation ####

    printf "Do you want to proceed? [y/N]: "

    # More portable read that works across shells
    if command -v read >/dev/null 2>&1; then
        read -r response 2>/dev/null || {
            # Fallback for systems where read might not work as expected
            response=$(head -n1 2>/dev/null || echo "n")
        }
    else
        # Ultimate fallback
        response="n"
    fi

    case "$response" in
        [yY]|[yY][eE][sS])
            ;;
        *)
            print_info "Operation cancelled by user"
            return 1
            ;;
    esac

    ### Doing the thing ####

    echo ""
    print_info "Starting setup process..."

    # Create target directory
    if ! create_directory "$MORLOC_BIN"; then
        return 1
    fi

    print_info "Using configuration file: $config_file"

    # Add to configuration file
    if ! add_to_config_file "$config_file"; then
        print_error "Failed to update configuration file"
        return 1
    fi

    # Source the configuration file to make PATH available immediately
    source_config_file "$config_file"

    # Test PATH functionality
    test_passed="false"
    if test_path_functionality; then
        test_passed="true"
    fi

    ### Show completion message ####

    echo ""
    print_success "Setup completed successfully!"
    echo ""

    if [ "$test_passed" = "true" ]; then
        printf "%s[OK] All systems go!%s\n" "$GREEN" "$RESET"
        echo "  - Directory created: $MORLOC_BIN"
        echo "  - PATH updated and active"
        echo "  - Executable test passed"
        echo ""
        printf "%sReady to use:%s\n" "$YELLOW" "$RESET"
        echo "  - Place executable files in: $MORLOC_BIN"
        echo "  - They will be accessible by name from anywhere"
    else
        printf "%sSetup complete with minor issues:%s\n" "$YELLOW" "$RESET"
        echo "  - Directory created: $MORLOC_BIN"
        echo "  - PATH updated in configuration file"
        echo "  - Executable test failed (shell caching or permissions)"
        echo ""
        printf "%sTroubleshooting:%s\n" "$YELLOW" "$RESET"
        echo "  - Try opening a new terminal"

        if [ "$shell_name" = "fish" ]; then
            echo "  - For fish shell, run: exec fish"
            echo "  - Verify with: echo \$PATH | grep $MORLOC_BIN_BASENAME"
        else
            echo "  - Verify with: echo \$PATH | grep '$MORLOC_BIN_BASENAME'"
            echo "  - Source manually: . \"${config_file}\""
        fi

        echo "  - Test manually: ls -la \"$MORLOC_BIN\""
    fi

    # Platform-specific notes
    case "${operating_system}" in
        "Darwin")
            echo ""
            printf "%smacOS Note:%s Terminal.app may need to be restarted for PATH changes\n" "$BLUE" "$RESET"
            ;;
        "Linux")
            # Check for WSL
            if [ -n "$WSL_DISTRO_NAME" ] || [ -n "$WSLENV" ] || grep -qi microsoft /proc/version 2>/dev/null; then
                echo ""
                printf "%sWSL Note:%s Windows Terminal may need to be restarted for PATH changes\n" "$BLUE" "$RESET"
            fi
            ;;
    esac
}

# }}}
# {{{ define scripts and their environments

# build an environment container if it does not yet exist
build_environment() {
    envname=$1
    dockerfile=$2
    envtag=$3
    container_base=$4

    # Check if image already exists
    if $CONTAINER_ENGINE image inspect "$envtag" >/dev/null 2>&1; then
        # Get the modification time of the Dockerfile
        if [ -f "$dockerfile" ]; then
            dockerfile_mtime=$(stat -c %Y "$dockerfile" 2>/dev/null || stat -f %m "$dockerfile" 2>/dev/null)
            # Get image creation time (Unix timestamp)
            # Docker and Podman both support this format
            image_created=$($CONTAINER_ENGINE image inspect "$envtag" --format '{{.Created}}' 2>/dev/null)

            # Convert image created time to Unix timestamp
            # This is portable across docker and podman
            if command -v date >/dev/null 2>&1; then
                image_timestamp=$(date -d "$image_created" +%s 2>/dev/null || date -j -f "%Y-%m-%dT%H:%M:%S" "$image_created" +%s 2>/dev/null)
            fi

            # Compare timestamps - rebuild if Dockerfile is newer
            # Default to rebuilding when comparison fails (empty values or arithmetic errors)
            if [ -n "$dockerfile_mtime" ] && [ -n "$image_timestamp" ] && \
               [ "$dockerfile_mtime" -le "$image_timestamp" ] 2>/dev/null; then
                print_info "Image '$envtag' is up to date"
                return 0
            else
                print_info "Dockerfile has been modified (or timestamp comparison failed), rebuilding image '$envtag'"
            fi
        else
            print_warning "Dockerfile '$dockerfile' not found, but image exists. Using existing image."
            return 0
        fi
    else
        print_info "Building new image '$envtag'"
    fi

    # Build the image (quotes needed in case of spaces in paths)
    if ! $CONTAINER_ENGINE build --build-arg CONTAINER_BASE="$container_base" --tag "$envtag" --file "$dockerfile" "$(dirname "$dockerfile")"; then
        print_error "Failed to build image '$envtag' from '$dockerfile'"
        return 1
    fi

    print_success "Built image '$envtag'"
    return 0
}

script_menv() {
    script_path="${1:-}"; [ $# -gt 0 ] && shift
    tag="${1:-}";         [ $# -gt 0 ] && shift
    envname="${1:-}";     [ $# -gt 0 ] && shift
    envfile="${1:-}";     [ $# -gt 0 ] && shift
    extra_args="${1:-}"

    base_container=$CONTAINER_BASE_FULL:$tag

    if [ -n "$envname" ] && [ -n "$envfile" ]; then
        user_container="morloc-env:$tag-$envname"
        build_environment "$envname" "$envfile" "$user_container" "$base_container" || return $?
    elif [ -n "$envname" ] || [ -n "$envfile" ]; then
        print_error "Both env name and file must be provided together"
        return 1
    else
        user_container="$base_container"
    fi

    print_info "Creating menv at '$script_path' with Morloc v${tag}"

    cat << EOF > "$script_path"
#!/usr/bin/env sh
# automatically generated script, do not modify
$CONTAINER_ENGINE run --rm \\
           --shm-size=$SHARED_MEMORY_SIZE \\
           -e HOME=\$HOME \\
           -v \$HOME/${MORLOC_INSTALL_DIR}/$tag:\$HOME/${MORLOC_DATA_RELDIR} \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           ${extra_args}${user_container} "\$@"

EOF

    if [ $? -ne 0 ]; then
        print_error "Failed to create script at '$script_path'"
        return 1
    fi

    chmod 755 "$script_path"

    observed_version=$("$script_path" morloc --version 2>/dev/null)
    if [ $? -ne 0 ]; then
        print_warning "Could not verify morloc version from '$script_path'"
    elif [ "$observed_version" != "$tag" ]; then
        print_warning "Observed version ($observed_version) is different from expected version ($tag)"
    fi

    print_info "$script_path made executable"
}

script_morloc_shell() {
    script_path="${1:-}"; [ $# -gt 0 ] && shift
    tag="${1:-}";         [ $# -gt 0 ] && shift
    envname="${1:-}";     [ $# -gt 0 ] && shift
    envfile="${1:-}";     [ $# -gt 0 ] && shift
    extra_args="${1:-}"

    base_container=$CONTAINER_BASE_FULL:$tag

    if [ -n "$envname" ] && [ -n "$envfile" ]; then
        user_container="morloc-env:$tag-$envname"
        build_environment "$envname" "$envfile" "$user_container" "$base_container" || return $?
    elif [ -n "$envname" ] || [ -n "$envfile" ]; then
        print_error "Both env name and file must be provided together"
        return 1
    else
        user_container="$base_container"
    fi

    print_info "Creating morloc-shell at '$script_path' with Morloc v${tag}"

    cat << EOF > "$script_path"
#!/usr/bin/env sh
# automatically generated script, do not modify
$CONTAINER_ENGINE run --shm-size=$SHARED_MEMORY_SIZE \\
           --rm -it \\
           -e HOME=\$HOME \\
           -e PATH="/root/.ghcup/bin:\$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \\
           -v \$HOME/${MORLOC_INSTALL_DIR}/$tag:\$HOME/${MORLOC_DATA_RELDIR} \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           ${extra_args}${user_container} /bin/bash

EOF

    if [ $? -ne 0 ]; then
        print_error "Failed to create script at '$script_path'"
        return 1
    fi

    chmod 755 "$script_path"

    # Check version via menv (morloc-shell is interactive, can't run non-interactively)
    if [ -f "$MORLOC_BIN/menv" ]; then
        observed_version=$("$MORLOC_BIN/menv" morloc --version 2>/dev/null)
        if [ $? -ne 0 ]; then
            print_warning "Could not verify morloc version from '$MORLOC_BIN/menv'"
        elif [ "$observed_version" != "$tag" ]; then
            print_warning "Observed version ($observed_version) is different from expected version ($tag)"
        fi
    fi
}

script_menv_dev() {
    script_path="${1:-}"; [ $# -gt 0 ] && shift
    envname="${1:-}";     [ $# -gt 0 ] && shift
    envfile="${1:-}";     [ $# -gt 0 ] && shift
    extra_args="${1:-}"

    tag=${LOCAL_VERSION}
    base_container=$CONTAINER_BASE_TEST

    if [ -n "$envname" ] && [ -n "$envfile" ]; then
        user_container="morloc-env:$tag-$envname"
        build_environment "$envname" "$envfile" "$user_container" "$base_container" || return $?
    elif [ -n "$envname" ] || [ -n "$envfile" ]; then
        print_error "Both env name and file must be provided together"
        return 1
    else
        user_container="$CONTAINER_BASE_TEST"
    fi

    print_info "Creating menv-dev at '$script_path'"

    mock_home="${MORLOC_INSTALL_DIR}/$tag/home"
    mkdir -p "$HOME/$mock_home/.stack"
    mkdir -p "$HOME/$mock_home/.local/bin"
    cat << EOF > "$script_path"
#!/usr/bin/env sh
# automatically generated script, do not modify
$CONTAINER_ENGINE run --shm-size=$SHARED_MEMORY_SIZE \\
           --rm \\
           -e HOME=\$HOME \\
           -e PATH="/root/.ghcup/bin:\$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \\
           -v \$HOME/${MORLOC_INSTALL_DIR}/$tag:\$HOME/${MORLOC_DATA_RELDIR} \\
           -v \$HOME/$mock_home/.local/bin:\$HOME/${MORLOC_BIN_BASENAME} \\
           -v \$HOME/$mock_home/.stack:\$HOME/.stack \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           ${extra_args}${user_container} "\$@"

EOF
    chmod 755 "$script_path"
}

script_morloc_dev_shell() {
    script_path="${1:-}"; [ $# -gt 0 ] && shift
    envname="${1:-}";     [ $# -gt 0 ] && shift
    envfile="${1:-}";     [ $# -gt 0 ] && shift
    extra_args="${1:-}"

    tag=${LOCAL_VERSION}
    base_container=$CONTAINER_BASE_TEST
    mock_home="${MORLOC_INSTALL_DIR}/$tag/home"

    if [ -n "$envname" ] && [ -n "$envfile" ]; then
        user_container="morloc-env:$tag-$envname"
        build_environment "$envname" "$envfile" "$user_container" "$base_container" || return $?
    elif [ -n "$envname" ] || [ -n "$envfile" ]; then
        print_error "Both env name and file must be provided together"
        return 1
    else
        user_container="$CONTAINER_BASE_TEST"
    fi

    print_info "Creating dev shell at '$script_path'"

    mkdir -p "$HOME/$mock_home/.stack"
    mkdir -p "$HOME/$mock_home/.local/bin"
    cat << EOF > "$script_path"
#!/usr/bin/env sh
# automatically generated script, do not modify
$CONTAINER_ENGINE run --shm-size=$SHARED_MEMORY_SIZE \\
           --rm \\
           -it \\
           -e HOME=\$HOME \\
           -e PATH="/root/.ghcup/bin:\$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \\
           -v \$HOME/${MORLOC_INSTALL_DIR}/$tag:\$HOME/${MORLOC_DATA_RELDIR} \\
           -v \$HOME/$mock_home/.local/bin:\$HOME/${MORLOC_BIN_BASENAME} \\
           -v \$HOME/$mock_home/.stack:\$HOME/.stack \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           ${extra_args}${user_container} /bin/bash
EOF
    chmod 755 "$script_path"
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
  ${BOLD}${GREEN}install${RESET}    Install morloc containers, scripts, and home
  ${BOLD}${GREEN}uninstall${RESET}  Remove morloc containers, scripts, and home
  ${BOLD}${GREEN}update${RESET}     Pull the latest version of this script
  ${BOLD}${GREEN}select${RESET}     Choose a new Morloc version
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

Creates four executable scripts:

 1. ${BOLD}${GREEN}menv${RESET}: runs commands in a Morloc container. Examples:
    $ menv morloc make -o foo foo.loc
    $ menv ./foo double 21

 2. ${BOLD}${GREEN}morloc-shell${RESET}: enter the "full" container in a shell
    - contains Python, R, and C++ compiler
    - contains vim and other conveniences

 3. ${BOLD}${GREEN}menv-dev${RESET}: runs commands in a dev container
    - contains Haskell tools for building from source
    - can access all system executables

 4. ${BOLD}${GREEN}morloc-shell-dev${RESET}: enter the dev shell
    - interactively build and test morloc from source

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message
      --no-init        Do not run 'morloc init'

${BOLD}ARGUMENTS${RESET}:
  version        Version to install

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") install
  $(basename "$0") install 0.54.2
EOF
}

# Install subcommand
cmd_install() {
    verbose=false

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
            --no-init)
                no_init="true"
                shift
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
        print_info "Installing latest Morloc version"
        tag="edge"
    else
        print_info "Installing Morloc v$version"
        tag=$version
    fi

    add_morloc_bin_to_path || exit 1

    print_info "Copying this install script to $MORLOC_BIN"
    if [ "$(resolve_path "$MORLOC_BIN/$PROGRAM_NAME")" = "$(resolve_path "$0")" ]
    then
        print_point "$(basename "$0") is already on there!"
    else
        cp "$0" "$MORLOC_BIN/$PROGRAM_NAME"
    fi

    print_info "Looking for a container engine"

    # check if an appropriate container engine is installed
    if [ -z "$CONTAINER_ENGINE" ]; then
        print_error "No container engine found, please install podman or docker"
        exit 1
    else
        print_info "Using $CONTAINER_ENGINE $CONTAINER_ENGINE_VERSION as container engine"
    fi

    if [ "$version" = "undefined" ]
    then
        print_info "Attempting to pull containers for Morloc tag '$tag'"
    else
        print_info "Attempting to pull containers for Morloc version $version"
    fi

    $CONTAINER_ENGINE pull "$CONTAINER_BASE_TINY:${tag}"
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull container 'tiny'"
        echo "  Are you sure this Morloc version is defined?"
        echo "  If you are behind a corporate firewall or proxy, configure your container engine:"
        echo "    docker: set HTTPS_PROXY environment variable"
        echo "    podman: set HTTPS_PROXY or configure in /etc/containers/registries.conf"
        exit 1
    fi

    # pull container
    $CONTAINER_ENGINE pull "$CONTAINER_BASE_FULL:${tag}"
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull container 'full'"
        echo "  Are you sure this Morloc version is defined?"
        echo "  If you are behind a corporate firewall or proxy, configure your container engine:"
        echo "    docker: set HTTPS_PROXY environment variable"
        echo "    podman: set HTTPS_PROXY or configure in /etc/containers/registries.conf"
        exit 1
    fi

    $CONTAINER_ENGINE pull "$CONTAINER_BASE_TEST:latest"
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull container 'dev'"
        echo "  Are you sure this Morloc version is defined?"
        echo "  If you are behind a corporate firewall or proxy, configure your container engine:"
        echo "    docker: set HTTPS_PROXY environment variable"
        echo "    podman: set HTTPS_PROXY or configure in /etc/containers/registries.conf"
        exit 1
    fi

    # get Morloc version from container
    # filter out the carriage return that podman helpfully provided
    if [ "$version" = "undefined" ]
    then
        detected_version=$($CONTAINER_ENGINE run --rm "$CONTAINER_BASE_FULL:edge" morloc --version 2>/dev/null)
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

    morloc_data_home="$HOME/${MORLOC_INSTALL_DIR}/$version"

    print_info "Setting Morloc home to '${morloc_data_home}'"

    # create .morloc/version/$version folder
    create_directory "$morloc_data_home"
    if [ $? -ne 0 ]
    then
        print_error "Failed to create morloc home directory at '$morloc_data_home'"
        exit 1
    fi
    create_directory "$morloc_data_home/include"
    create_directory "$morloc_data_home/lib"
    create_directory "$morloc_data_home/opt"
    create_directory "$morloc_data_home/src/morloc/plane"
    create_directory "$morloc_data_home/tmp"

    print_info "Created $morloc_data_home"

    # create morloc scripts
    script_menv             "$MORLOC_BIN/menv" "$version"
    script_morloc_shell     "$MORLOC_BIN/morloc-shell" "$version"
    script_menv_dev         "$MORLOC_BIN/menv-dev"
    script_morloc_dev_shell "$MORLOC_BIN/morloc-shell-dev"

    if [ "$no_init" = "false" ]; then
      print_info "Initializing morloc libraries"
      "$MORLOC_BIN/menv" morloc init -f
      if [ $? -ne 0 ]
      then
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

    # Remove containers using this version
    ids=$($CONTAINER_ENGINE ps -a --filter "ancestor=$CONTAINER_BASE_FULL:$version" --format '{{.ID}}')
    [ -n "$ids" ] && echo "$ids" | xargs $CONTAINER_ENGINE rm -f
    ids=$($CONTAINER_ENGINE ps -a --filter "ancestor=$CONTAINER_BASE_TINY:$version" --format '{{.ID}}')
    [ -n "$ids" ] && echo "$ids" | xargs $CONTAINER_ENGINE rm -f

    # Remove environment images for this version
    ids=$($CONTAINER_ENGINE images --filter "reference=morloc-env:$version-*" --format '{{.ID}}')
    [ -n "$ids" ] && echo "$ids" | xargs $CONTAINER_ENGINE rmi -f

    # Remove base image
    $CONTAINER_ENGINE rmi -f "$CONTAINER_BASE_FULL:$version"
    $CONTAINER_ENGINE rmi -f "$CONTAINER_BASE_TINY:$version"

    print_success "All containers and images removed for $version"

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
    all_image_ids=$($CONTAINER_ENGINE images --filter "reference=${base_image}:*" --format '{{.ID}}' 2>/dev/null)
    # For each image ID, find containers
    container_ids=""
    for img_id in $all_image_ids; do
        ids=$($CONTAINER_ENGINE ps -a --filter "ancestor=$img_id" --format '{{.ID}}' 2>/dev/null)
        [ -n "$ids" ] && container_ids="$container_ids $ids"
    done

    if [ -n "$container_ids" ]; then
        print_info "Found containers: $container_ids"
        if $CONTAINER_ENGINE rm -f $container_ids; then
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
    image_ids=$($CONTAINER_ENGINE images --filter "reference=$base_image" --format '{{.ID}}' 2>/dev/null)

    if [ -n "$image_ids" ]; then
        print_info "Found images: $image_ids"
        if $CONTAINER_ENGINE rmi -f $image_ids; then
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

Remove Morloc home (or specific versions) and all associated containers

${BOLD}OPTIONS${RESET}:
  -h, --help     Show this help message
  -a, --all      Remove all Morloc versions

${BOLD}ARGUMENTS${RESET}:
  VERSION        Version to remove, may specify multiple versions

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") uninstall --all
  $(basename "$0") uninstall 0.55.7
  $(basename "$0") uninstall 0.53.6 0.53.7
EOF
}

cmd_uninstall() {
    version=""

    # Parse remove subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_uninstall_help
                exit 0
                ;;
            -a|--all)
                morloc_home="$HOME/${MORLOC_INSTALL_DIR}"
                if [ -d "$morloc_home" ]
                then
                    rm -rf "$morloc_home"
                    if [ $? -ne 0 ]
                    then
                        print_error "Failed to remove morloc home directory '$morloc_home'"
                    else
                        print_success "Removed morloc home directory '$morloc_home'"
                    fi
                else
                    print_warning "Cannot remove morloc home directory '$morloc_home', it does not exist"
                fi

                # remove all containers/images for all Morloc tags
                remove_all_containers_and_images "$CONTAINER_BASE_FULL"
                remove_all_containers_and_images "$CONTAINER_BASE_TINY"
                remove_all_containers_and_images "$CONTAINER_BASE_TEST"
                print_warning "Scripts in $MORLOC_BIN (menv, morloc-shell, etc.) were not removed"
                print_info "To remove them: rm $MORLOC_BIN/menv $MORLOC_BIN/morloc-shell $MORLOC_BIN/menv-dev $MORLOC_BIN/morloc-shell-dev"
                exit 0
                ;;
            -*)
                print_error "Unknown option for uninstall: $1"
                show_uninstall_help
                exit 1
                ;;
            *)
                version=$1
                morloc_home="$HOME/${MORLOC_INSTALL_DIR}/$version"
                if [ -d "$morloc_home" ]
                then
                    print_info "Morloc home '$morloc_home' found, deleting"
                    rm -rf "$morloc_home"
                    if [ $? -ne 0 ]
                    then
                        print_error "Failed to remove morloc home directory '$morloc_home'"
                    else
                        print_success "Removed morloc directory '$morloc_home'"
                    fi
                else
                    print_warning "Cannot remove morloc directory '$morloc_home', it does not exist"
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

    print_success "Removed containers and Morloc home, scripts remain"
}

# }}}
# {{{ update subcommand

# Help for install subcommand
show_update_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") update

Update this install script

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") update
EOF
}


cmd_update() {
    # Parse install subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_update_help
                exit 0
                ;;
            *)
                print_error "Unexpected argument"
                show_update_help
                exit 1
                ;;
        esac
    done

    old_version=$("$0" --version)
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

    nlinesdiff=$(diff "$tmp_script" "$0" | wc -l)
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

    new_version=$("$tmp_script" --version)

    print_info "Replacing current script at '$0'"
    mv "$tmp_script" "$0"
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

# Help for install subcommand
show_select_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") select <version>

Set Morloc version.

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message

${BOLD}ARGUMENTS${RESET}:
  version        Version to install

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") select 0.54.2
EOF
}

cmd_select() {

    version="undefined"

    # Parse install subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_select_help
                exit 0
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
        print_error "Cannot set to '${LOCAL_VERSION}' version, please use dev containers"
        exit 1
    fi

    if [ "$version" = "undefined" ]
    then
        print_error "Please select a version"
        # List available versions
        install_dir="$HOME/${MORLOC_INSTALL_DIR}"
        if [ -d "$install_dir" ]; then
            print_info "Available versions:"
            for d in "$install_dir"/*/; do
                [ -d "$d" ] || continue
                v=$(basename "$d")
                case "$v" in "$LOCAL_VERSION") continue ;; esac
                print_point "$v"
            done
        fi
        show_select_help
        exit 1
    fi

    if [ -d "$HOME/${MORLOC_INSTALL_DIR}/$version" ]
    then
        add_morloc_bin_to_path
        script_menv "$MORLOC_BIN/menv" "$version"
        script_morloc_shell "$MORLOC_BIN/morloc-shell" "$version"
    else
        print_error "Morloc version '$version' does not exist, install first"
        exit 1
    fi

    print_success "Switched to Morloc version '$version'"
    exit 0
}

# }}}
# {{{ info subcommand

# Help for install subcommand
show_info_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") info

Print info on Morloc versions and check containers

${BOLD}OPTIONS${RESET}:
  -h, --help   Show this help message

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") info
EOF
}

cmd_info() {

    # Parse install subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_info_help
                exit 0
                ;;
            *)
                print_error "Unexpected argument"
                show_info_help
                exit 1
                ;;
        esac
    done

    install_dir="$HOME/${MORLOC_INSTALL_DIR}"
    versions=""
    if [ -d "$install_dir" ]; then
        for d in "$install_dir"/*/; do
            [ -d "$d" ] || continue
            v=$(basename "$d")
            case "$v" in
                "$LOCAL_VERSION") continue ;;
            esac
            versions="$versions $v"
        done
    fi

    current_version=$("$MORLOC_BIN/menv" morloc --version 2>/dev/null)
    if [ $? -ne 0 ]
    then
        print_error "No current Morloc version set"
        current_version="none"
    fi

    dev_container=${CONTAINER_BASE_TEST}
    if $CONTAINER_ENGINE images --format '{{.Repository}}' | grep -q "^${dev_container}$"
    then
        printf "dev             %scontainer exists%s\n" "$GREEN" "$RESET"
    else
        printf "dev             %scontainer missing%s\n" "$RED" "$RESET"
    fi

    for version in $versions
    do
        selection="         "
        if [ "$version" = "$current_version" ]
        then
            selection=" selected"
        fi

        version_container="${CONTAINER_BASE_FULL}:${version}"

        if $CONTAINER_ENGINE images --format '{{.Repository}}:{{.Tag}}' | grep -q "^${version_container}$"
        then
            printf "%s%s %scontainer exists%s\n" "$version" "$selection" "$GREEN" "$RESET"
        else
            printf "%s%s %scontainer missing%s\n" "$version" "$selection" "$RED" "$RESET"
        fi

    done

    exit 0
}
# }}}
# {{{ env subcommand

update_environment() {
  envname=$1; shift
  update_dev=$1; shift
  update_usr=$1; shift
  extra_args=$1
  envfile="$MORLOC_DEPENDENCY_DIR/$envname.Dockerfile"

  print_info "Attempting to switch environment to ${envname} with ${envfile}"

  if [ -e "$envfile" ]; then
    print_info "$envfile found, attempting to build"
  else
    print_error "$envfile not found, please create and retry"
    return 1
  fi

  version=$("$MORLOC_BIN/menv" morloc --version 2>/dev/null)
  if [ $? -ne 0 ]
  then
      print_error "morloc does not appear to be installed, first install and then set the environment"
      return 1
  else
      print_info "Currently using morloc v$version"
  fi

  if [ "$update_usr" = "true" ]; then
      script_menv         "$MORLOC_BIN/menv"         "$version" "$envname" "$envfile" "$extra_args"
      script_morloc_shell "$MORLOC_BIN/morloc-shell" "$version" "$envname" "$envfile" "$extra_args"
      print_success "Switched user profiles to $version-$envname and built all required containers"
  fi

  if [ "$update_dev" = "true" ]; then
      script_menv_dev         "$MORLOC_BIN/menv-dev"         "$envname" "$envfile" "$extra_args"
      script_morloc_dev_shell "$MORLOC_BIN/morloc-shell-dev" "$envname" "$envfile" "$extra_args"
      print_success "Switched dev profiles to $version-$envname and built all required containers"
  fi

  return 0
}

reset_environment() {
  reset_update_dev="$1"
  reset_update_usr="$2"

  version=$("$MORLOC_BIN/menv" morloc --version 2>/dev/null)
  if [ $? -ne 0 ]
  then
      print_error "morloc does not appear to be installed, nothing needs to be reset"
      return 1
  else
      print_info "Currently using morloc v$version"
  fi

  if [ "$reset_update_usr" = "true" ]; then
      script_menv             "$MORLOC_BIN/menv"         "$version"
      script_morloc_shell     "$MORLOC_BIN/morloc-shell" "$version"
      print_success "Successfully reset user profiles to default environment"
  fi

  if [ "$reset_update_dev" = "true" ]; then
      script_menv_dev         "$MORLOC_BIN/menv-dev"
      script_morloc_dev_shell "$MORLOC_BIN/morloc-shell-dev"
      print_success "Successfully reset dev profiles to default environment"
  fi

  return 0
}

list_local_environment() {

    # Check if directory doesn't exist
    if [ ! -d "$MORLOC_DEPENDENCY_DIR" ]; then
        print_info "No dependency environments defined. To add an environment, create a Dockerfile in the $MORLOC_DEPENDENCY_DIR directory"
        return 0
    fi

    # Check if directory is empty or has no .Dockerfile files
    found=0
    for file in "$MORLOC_DEPENDENCY_DIR"/*.Dockerfile; do
        # Check if glob matched anything (fails if no files exist)
        if [ -e "$file" ]; then
            found=1
            break
        fi
    done

    if [ "$found" -eq 0 ]; then
        print_info "No dependency environments defined"
        return 0
    fi

    current_env=$("$MORLOC_BIN/menv" sh -c "echo \$MORLOC_ENV_NAME" 2>/dev/null)

    # List all .Dockerfile files
    for file in "$MORLOC_DEPENDENCY_DIR"/*.Dockerfile; do
        if [ -e "$file" ]; then
            basename="${file##*/}"           # Get basename
            basename="${basename%.Dockerfile}"  # Remove .Dockerfile extension
            if [ "$basename" = "$current_env" ]; then
                printf "%s\t%s\t(current)\n" "$basename" "$file"
            else
                printf "%s\t%s\n" "$basename" "$file"
            fi
        fi
    done
}

init_environment() {
    envname="$1"
    envfile="$MORLOC_DEPENDENCY_DIR/$1.Dockerfile"

    # if MORLOC_DEPENDENCY_DIR does not exist, create the directory
    mkdir -p "$MORLOC_DEPENDENCY_DIR"

    if [ -e "$envfile" ]; then
        print_error "Cannot create $envfile, file already exists"
        exit 1
    fi

    cat << EOF > "$envfile"
# Automatically generated section, DO NOT MODIFY
# ----------------------------------------------
ARG CONTAINER_BASE
FROM \${CONTAINER_BASE}
LABEL morloc.environment="$envname"
ENV MORLOC_ENV_NAME="$envname"
# End of automatically generated section
# ----------------------------------------------

# Add custom setup below this line
EOF

    print_success "Created stub Dockerfile at $envfile, edit as needed"
    exit 0
}

# Help for env subcommand
show_env_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename "$0") env [OPTIONS] [ENV]

Select an environment. The environment is defined as a Dockerfile
that builds on a version-specific morloc image.

${BOLD}OPTIONS${RESET}:
  -h, --help      Show this help message
      --list      List all locally defined environments
      --init ENV  Create a stub Dockerfile
      --reset     Reset to the default environment
  -x, --extra ARG Extra arguments for the container
      --dev       Act only on the dev profiles
      --usr       Act only on the user profiles

${BOLD}EXAMPLES${RESET}:
  $(basename "$0") env --list
  $(basename "$0") env --init ml
  $(basename "$0") env ml
  $(basename "$0") env app --extra "-p 8000:8000"
EOF
}

cmd_env() {
    # Parse install subcommand arguments
    env=""
    update_dev="true"
    update_usr="true"
    reset="false"
    extra_args=""
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
            --dev)
                shift
                update_dev="true"
                update_usr="false"
                ;;
            --usr)
                shift
                update_dev="false"
                update_usr="true"
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
        reset_environment "$update_dev" "$update_usr"
    else
        if [ -z "$env" ]; then
          print_error "No environment specified"
          show_env_help
        else
          update_environment "$env" "$update_dev" "$update_usr" "$extra_args"
        fi
    fi

    exit 0
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

    # Dispatch subcommand
    case "${1:-}" in
        install)   shift; cmd_install "$@" ;;
        uninstall) shift; cmd_uninstall "$@" ;;
        update)    shift; cmd_update "$@" ;;
        select)    shift; cmd_select "$@" ;;
        env)       shift; cmd_env "$@" ;;
        info)      shift; cmd_info "$@" ;;
        "")        show_help; exit 0 ;;
        *)         print_error "Unknown command: $1"; show_help; exit 1 ;;
    esac
}

# }}}

# Run main function with all arguments
main "$@"
