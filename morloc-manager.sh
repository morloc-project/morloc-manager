#!/usr/bin/env sh

# Morloc Manager

# {{{ constants and system info

PROGRAM_NAME="morloc-manager"
VERSION="0.4.0"

CONTAINER_ENGINE_VERSION=""
CONTAINER_ENGINE=""

SHARED_MEMORY_SIZE=4g

CONTAINER_BASE_FULL=ghcr.io/morloc-project/morloc/morloc-full
CONTAINER_BASE_TINY=ghcr.io/morloc-project/morloc/morloc-tiny
CONTAINER_BASE_TEST=ghcr.io/morloc-project/morloc/morloc-test

THIS_SCRIPT_URL="https://raw.githubusercontent.com/morloc-project/morloc-manager/refs/heads/main/morloc-manager.sh"

if command -v podman >/dev/null 2>&1; then
    CONTAINER_ENGINE_VERSION=$(podman --version 2>/dev/null | sed 's/.* //')
    CONTAINER_ENGINE="podman"
elif command -v docker >/dev/null 2>&1; then
    CONTAINER_ENGINE_VERSION=$(docker --version 2>/dev/null | sed 's/.* //')
    CONTAINER_ENGINE="docker"
fi

# location of modules and other data will be stored for all morloc versions
MORLOC_DATA_HOME=${XDG_DATA_HOME:-~/.local/share}/morloc

# location of global morloc config and version specific configs will be stored
MORLOC_CONFIG_HOME=${XDG_CONFIG_HOME:-~/.config}/morloc

# location of all program state may be stored (may always be safely deleted
# when programs are not running)
MORLOC_STATE_HOME=${XDG_STATE_HOME:-~/.local/state}/morloc

# location of all cached data for morloc programs
MORLOC_CACHE_HOME=${XDG_CACHE_HOME:-~/.cache}/morloc

MORLOC_INSTALL_DIR="${MORLOC_DATA_HOME#$HOME/}/versions"
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
            shell_name=$(ps -p $ -o comm= 2>/dev/null | sed 's/^-//' || echo "sh")
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
    local path="$1"
    # Remove trailing slashes (but keep root /)
    if [ "$path" != "/" ]; then
        path="${path%/}"
    fi
    # Handle multiple consecutive slashes
    while [ "$path" != "${path//\/\//\/}" ]; do
        path="${path//\/\//\/}"
    done
    echo "$path"
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

    sleep 0.5

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
            # Use the portable '.' command for POSIX shells
            # shellcheck disable=SC1090
            if [ -f "$config_file" ] && . "$config_file" 2>/dev/null; then
                print_success "Configuration file sourced successfully"

                # Verify the PATH was updated
                if is_in_path "$MORLOC_BIN"; then
                    print_success "$MORLOC_BIN is now in your current PATH"
                else
                    print_warning "PATH update may not have taken effect immediately"
                    print_warning "Try opening a new terminal if the directory isn't accessible"
                fi
            else
                print_warning "Could not source $config_file automatically"
                print_warning "The PATH will be available in new shell sessions"
                print_warning "To update current session manually, run: . \"$config_file\""
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
    # Add a small delay to ensure filesystem consistency
    sleep 1

    if command -v "$test_command" >/dev/null 2>&1 && "$test_command" >/dev/null 2>&1; then
        print_success "✓ PATH test passed - executable files in ~/.foo/bin are accessible"
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

    # Improved error handling that works across platforms
    cleanup() {
        # Clean up any temporary files
        rm -f "$MORLOC_BIN"/path-test-* 2>/dev/null || true
    }

    # Set up signal handlers
    trap 'cleanup; log_error "Script interrupted"; exit 1' INT TERM
    trap 'cleanup' EXIT

    ### Configuration ####

    # Show current status
    print_info "Setting up Morloc bin:"

    morloc_bin_exists=$( if [ -d "$MORLOC_BIN" ]; then echo 0; else echo 1; fi )
    morloc_bin_is_in_path=$( if is_in_path "$MORLOC_BIN"; then echo 0; else echo 1; fi )

    printf "  Target Morloc bin folder: $MORLOC_BIN "

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
            echo "  ${GREEN}✓ All systems go!${RESET}"
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

    echo "${YELLOW}This script will:${RESET}"
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
            break
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
        exit 1
    fi

    print_info "Using configuration file: $config_file"

    # Add to configuration file
    if ! add_to_config_file "$config_file"; then
        print_error "Failed to update configuration file"
        exit 1
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
        echo "${GREEN}✓ All systems go!${RESET}"
        echo "  • Directory created: $MORLOC_BIN"
        echo "  • PATH updated and active"
        echo "  • Executable test passed"
        echo ""
        echo "${YELLOW}Ready to use:${RESET}"
        echo "  • Place executable files in: $MORLOC_BIN"
        echo "  • They will be accessible by name from anywhere"
    else
        echo "${YELLOW}Setup complete with minor issues:${RESET}"
        echo "  • Directory created: $MORLOC_BIN"
        echo "  • PATH updated in configuration file"
        echo "  • Executable test failed (shell caching or permissions)"
        echo ""
        echo "${YELLOW}Troubleshooting:${RESET}"
        echo "  • Try opening a new terminal"

        if [ "$shell_name" = "fish" ]; then
            echo "  • For fish shell, run: exec fish"
            echo "  • Verify with: echo \$PATH | grep .foo/bin"
        else
            echo "  • Verify with: echo \$PATH | grep '\\.foo/bin'"
            echo "  • Source manually: . \"${config_file}\""
        fi

        echo "  • Test manually: ls -la \"$MORLOC_BIN\""
    fi

    # Platform-specific notes
    case "${operating_system}" in
        "Darwin")
            echo ""
            echo "${BLUE}macOS Note:${RESET} Terminal.app may need to be restarted for PATH changes"
            ;;
        "Linux")
            # Check for WSL
            if [ -n "$WSL_DISTRO_NAME" ] || [ -n "$WSLENV" ] || grep -qi microsoft /proc/version 2>/dev/null; then
                echo ""
                echo "${BLUE}WSL Note:${RESET} Windows Terminal may need to be restarted for PATH changes"
            fi
            ;;
    esac
}

# }}}
# {{{ define scripts
script_menv() {
    script_path=$1
    tag=$2

    print_info "Creating menv at '$script_path' with Morloc v${tag}"

    cat << EOF > "$script_path"
# automatically generated script, do not modify
$CONTAINER_ENGINE run --rm \\
           --shm-size=$SHARED_MEMORY_SIZE \\
           -e HOME=\$HOME \\
           -v \$HOME/${MORLOC_INSTALL_DIR}/$tag:\$HOME/${MORLOC_DATA_HOME#$HOME/} \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           $CONTAINER_BASE_FULL:$tag "\$@"

EOF

    if [ $? -ne 0 ]
    then
        print_error "Failed to get run 'menv morloc --version'"
    fi

    observed_version=$(menv morloc --version)
    if [ "$observed_version" != "$tag" ]
    then
        print_warning "Observed version ($observed_version) is different from expected version ($tag)"
    fi

    chmod 755 $script_path
    print_info "$script_path made executable"
}

script_morloc_shell() {
    script_path=$1
    tag=$2

    print_info "Creating morloc-shell at '$script_path' with Morloc v${tag}"

    cat << EOF > "$script_path"
# automatically generated script, do not modify
$CONTAINER_ENGINE run --rm \\
           --shm-size=$SHARED_MEMORY_SIZE \\
           -it \\
           -e HOME=\$HOME \\
           -v \$HOME/${MORLOC_INSTALL_DIR}/$tag:\$HOME/${MORLOC_DATA_HOME#$HOME/} \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           $CONTAINER_BASE_FULL:$tag /bin/bash
EOF

    observed_version=$(menv morloc --version)
    if [ $? -ne 0 ]
    then
        print_error "Failed to get run `menv morloc --version`"
    fi

    if [ "$observed_version" != "$tag" ]
    then
        print_warning "Observed version ($observed_version) is different from expected version ($tag)"
    fi

    chmod 755 $script_path
}

script_menv_dev() {
    script_path=$1
    tag=${LOCAL_VERSION}

    print_info "Creating menv-dev at '$script_path'"

    mock_home="${MORLOC_INSTALL_DIR}/$tag/home"
    mkdir -p "$HOME/$mock_home/.stack"
    mkdir -p "$HOME/$mock_home/.local/bin"
    cat << EOF > "$script_path"
# automatically generated script, do not modify
$CONTAINER_ENGINE run --shm-size=$SHARED_MEMORY_SIZE \\
           --rm \\
           -e HOME=\$HOME \\
           -e PATH="\$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \\
           -v \$HOME/${MORLOC_INSTALL_DIR}/$tag:\$HOME/${MORLOC_DATA_HOME#$HOME/}} \\
           -v \$HOME/$mock_home/.stack:\$HOME/.stack \\
           -v \$HOME/$mock_home/.local/bin:\$HOME/.local/bin \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           $CONTAINER_BASE_TEST "\$@"

EOF
    chmod 755 $script_path
}

script_morloc_dev_shell() {
    script_path=$1
    tag=${LOCAL_VERSION}
    mock_home="${MORLOC_INSTALL_DIR}/$tag/home"

    print_info "Creating dev shell at '$script_path'"

    mkdir -p "$HOME/$mock_home/.stack"
    mkdir -p "$HOME/$mock_home/.local/bin"
    cat << EOF > "$script_path"
# automatically generated script, do not modify
$CONTAINER_ENGINE run --shm-size=$SHARED_MEMORY_SIZE \\
           --rm \\
           -it \\
           -e HOME=\$HOME \\
           -e PATH="\$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \\
           -v \$HOME/${MORLOC_INSTALL_DIR}/$tag:\$HOME/${MORLOC_DATA_HOME#$HOME/} \\
           -v \$HOME/$mock_home/.local/bin:\$HOME/.local/bin \\
           -v \$HOME/$mock_home/.stack:\$HOME/.stack \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           $CONTAINER_BASE_TEST /bin/bash
EOF
    chmod 755 $script_path
}

# }}}
# {{{ main help and version

# Version function
show_version() {
    echo "${VERSION}"
}

show_help() {
    cat << EOF
${BOLD}$(basename $0)${RESET} ${VERSION} - manage morloc containerized installation

${BOLD}USAGE${RESET}: $(basename $0) [OPTIONS] COMMAND [ARGS...]

${BOLD}OPTIONS${RESET}:
  -h, --help     Show this help message
  -v, --version  Show this manager version

${BOLD}COMMANDS${RESET}:
  ${BOLD}${GREEN}install${RESET}    Install morloc containers, scripts, and home
  ${BOLD}${GREEN}uninstall${RESET}  Remove morloc containers, scripts, and home
  ${BOLD}${GREEN}update${RESET}     Pull the latest version of this script
  ${BOLD}${GREEN}select${RESET}     Choose a new Morloc version
  ${BOLD}${GREEN}info${RESET}       Print info about manager, installs and containers

${BOLD}EXAMPLES${RESET}:
  $(basename $0) install
  $(basename $0) uninstall
  $(basename $0) --help
EOF
}

# }}}
# {{{ install subcommand

# Help for install subcommand
show_install_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename $0) install [OPTIONS] <version>

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
    - can access to all system executables

 4. ${BOLD}${GREEN}morloc-shell-dev${RESET}: enter the dev shell

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message

${BOLD}ARGUMENTS${RESET}:
  version        Version to install

${BOLD}EXAMPLES${RESET}:
  $(basename $0) install
  $(basename $0) install 0.54.2
EOF
}

# Install subcommand
cmd_install() {
    verbose=false

    # calling these "undefined" instead of empty strings for better debugging
    version="undefined"
    tag="undefined"

    # Parse install subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_install_help
                exit 0
                ;;
            -*)
                print_error "Unknown option for install: $1"
                show_install_help
                exit 1
                ;;
            *)
                if [ $version = "undefined" ]; then
                    version="$1"
                else
                    print_error "Multiple version installation not supported: $1"
                    exit 1
                fi
                shift
                ;;
        esac
    done

    if [ $version = "undefined" ]; then
        print_info "Installing latest Morloc version"
        tag="edge"
    else
        print_info "Installing Morloc v$version"
        tag=$version
    fi

    add_morloc_bin_to_path
    if [ $? -ne 0 ]
    then
        exit 1
    fi

    print_info "Copying this install script to $MORLOC_BIN"
    if [ $(normalize_path $MORLOC_BIN/$PROGRAM_NAME) = $(normalize_path $0) ]
    then
        print_point "$(basename $0) is already on there!"
    else
        cp $0 $MORLOC_BIN/$PROGRAM_NAME
    fi

    print_info "Looking for a container engine"

    # check if an appropriate container engine is installed
    if [ -z "$CONTAINER_ENGINE" ]; then
        print_error "No container engine found, please install podman or docker"
        exit 1
    else
        print_info "Using $CONTAINER_ENGINE $CONTAINER_ENGINE_VERSION as a container engine"
    fi

    if [ "$version" = "undefined" ]
    then
        print_info "Attempting to pull containers for Morloc tag '$tag'"
    else
        print_info "Attempting to pull containers for Morloc version $version"
    fi

    $CONTAINER_ENGINE pull $CONTAINER_BASE_TINY:${tag}
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull container 'tiny'"
        echo "  Are you sure this Morloc version is defined?"
        exit 1
    fi

    # pull container
    $CONTAINER_ENGINE pull $CONTAINER_BASE_FULL:${tag}
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull container 'full'"
        echo "  Are you sure this Morloc version is defined?"
        exit 1
    fi

    $CONTAINER_ENGINE pull $CONTAINER_BASE_TEST
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull container 'dev'"
        echo "  Are you sure this Morloc version is defined?"
        exit 1
    fi

    # get Morloc version from container
    # filter out the carriage return that podman helpfully provided
    if [ "$version" = "undefined" ]
    then
        detected_version=$($CONTAINER_ENGINE run -it $CONTAINER_BASE_FULL:edge morloc --version | tr -d '\r\n')
        if [ $? -ne 0 ]
        then
            print_error "Failed to detect version from morloc container"
            exit 1
        fi

        if [ $detected_version = "" ]
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
    create_directory $morloc_data_home
    if [ $? -ne 0 ]
    then
        print_error "Failed to create morloc home directory at '$morloc_data_home'"
        exit 1
    fi
    create_directory $morloc_data_home/include
    create_directory $morloc_data_home/lib
    create_directory $morloc_data_home/opt
    create_directory $morloc_data_home/src/morloc/plane
    create_directory $morloc_data_home/tmp

    print_info "Created $morloc_data_home"

    # create morloc scripts
    script_menv             "$MORLOC_BIN/menv" $version
    script_morloc_shell     "$MORLOC_BIN/morloc-shell" $version
    script_menv_dev         "$MORLOC_BIN/menv-dev"
    script_morloc_dev_shell "$MORLOC_BIN/morloc-shell-dev"

    print_info "Initializing morloc libraries"
    menv morloc init -f
    if [ $? -ne 0 ]
    then
        print_error "Failed to build morloc libraries"
        exit 1
    fi

    print_success "Morloc v$version installed successfully"
}

# }}}
# {{{ uninstall subcommand

# Function to remove all containers for a given image
# Usage: remove_containers_for "image_name"
remove_containers_for_version() {
    image_name="$1"

    if [ -z "$image_name" ]; then
        print_error "Image name required missing"
        return 1
    fi

    if [ -z "$CONTAINER_ENGINE" ]; then
        print_error "CONTAINER_ENGINE variable not set"
        return 1
    fi

    print_info "Removing containers for $image_name using $CONTAINER_ENGINE ..."

    # Get container IDs
    container_ids=$($CONTAINER_ENGINE ps -a --filter "ancestor=$image_name" --format '{{.ID}}' 2>/dev/null)

    if [ -n "$container_ids" ]; then
        echo "Found containers: $container_ids"
        if $CONTAINER_ENGINE rm -f $container_ids; then
            print_success "Containers removed successfully"
        else
            print_error "Error removing containers"
            return 1
        fi
    else
        print_warning "No containers found for $image_name"
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
    container_ids=$($CONTAINER_ENGINE ps -a --filter "ancestor=$base_image" --format '{{.ID}}' 2>/dev/null)

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
${BOLD}USAGE${RESET}: $(basename $0) uninstall [OPTIONS] <version>

Remove Morloc home (or specfic versions) and all associated containers

${BOLD}OPTIONS${RESET}:
  -h, --help     Show this help message

${BOLD}ARGUMENTS${RESET}:
  version        Version to remove (optional, remove everything by default)

${BOLD}EXAMPLES${RESET}:
  $(basename $0) uninstall
  $(basename $0) uninstall 0.52.4
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
            -*)
                print_error "Unknown option for uninstall: $1"
                show_remove_help
                exit 1
                ;;
            *)
                if [ -z "$version" ]; then
                    version="$1"
                else
                    print_error "Multiple version are not supported yet: $1"
                    exit 1
                fi
                shift
                ;;
        esac
    done

    if [ -z $version ]
    then
        morloc_home="$HOME/${MORLOC_INSTALL_DIR}"
        if [[ -d "$morloc_home" ]]
        then
            rm -rf "$morloc_home"
            if [[ $? -ne 0 ]]
            then
                print_error "Failed to remove morloc home directory '$morloc_home'"
            else
                print_success "Removed morloc home directory '$morloc_home'"
            fi
        else
            print_warning "Cannot remove morloc home directory '$morloc_home', it does not exist"
        fi

        # remove all containers/images for all Morloc tags
        remove_all_containers_and_images $CONTAINER_BASE_FULL
        remove_all_containers_and_images $CONTAINER_BASE_TINY
        remove_all_containers_and_images $CONTAINER_BASE_TEST
    else
        morloc_home="$HOME/${MORLOC_INSTALL_DIR}/$version"
        if [[ -d "$morloc_home" ]]
        then
            print_info "Morloc home '$morloc_home' found, deleting"
            rm -rf "$morloc_home"
            if [[ $? -ne 0 ]]
            then
                print_error "Failed to remove morloc home directory '$morloc_home'"
            else
                print_success "Removed morloc directory '$morloc_home'"
            fi
        else
            print_warning "Cannot remove morloc directory '$morloc_home', it does not exist"
        fi
        remove_containers_for_version $CONTAINER_BASE_FULL:$version
    fi

    print_success "Removed containers and Morloc home, scripts remain"
}

# }}}
# {{{ update subcommand

# Help for install subcommand
show_update_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename $0) update

Update this install sccript

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message

${BOLD}EXAMPLES${RESET}:
  $(basename $0) update
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

    old_version=$($0 --version)

    tmp_script="/tmp/$PROGRAM_NAME"

    WGET_PATH=$(command -v wget 2>/dev/null || true)
    CURL_PATH=$(command -v curl 2>/dev/null || true)

    if [ -n "$WGET_PATH" ] && [ -x "$WGET_PATH" ]; then
      print_info "Checking for latest $PROGRAM_NAME script (using wget)"
      "$WGET_PATH" -q -O "$tmp_script" "$THIS_SCRIPT_URL"
    elif [ -n "$CURL_PATH" ] && [ -x "$CURL_PATH" ]; then
      print_info "Checking for latest $PROGRAM_NAME script (using wget)"
      "$CURL_PATH" -fsSL -o "$tmp_script" "$THIS_SCRIPT_URL"
    else
      print_error "Please install either wget or curl"
      rm -f "$tmp_script"
      exit 1
    fi

    if [ $? -ne 0 ]
    then
        print_error "Failed to retrieve script from '$THIS_SCRIPT_URL'"
        rm -f $tmp_script
        exit 1
    fi

    nlinesdiff=$(diff $tmp_script $0 | wc -l)
    if [ $nlinesdiff -ne 0 ]
    then
        print_info "Successfully pulled '$THIS_SCRIPT_URL'"
    else
        print_info "You are already using the latest version"
        rm -f $tmp_script
        exit 0
    fi

    print_info "Making script executable"
    chmod 755 $tmp_script
    if [ $? -ne 0 ]
    then
        print_exit "Failed to make new script executable, exiting"
        rm -f $tmp_script
        exit 1
    fi

    new_version=$($tmp_script --version)

    print_info "Replacing current script at '$0'"
    mv $tmp_script $0
    if [ $? -ne 0 ]
    then
        print_exit "Failed to replace current script, exiting"
        rm -f $tmp_script
        exit 1
    fi

    print_success "Updated from $old_version to $new_version"
}
# }}}
# {{{ select subcommand

# Help for install subcommand
show_select_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename $0) select <version>

Set Morloc version.

${BOLD}OPTIONS${RESET}:
  -h, --help           Show this help message

${BOLD}ARGUMENTS${RESET}:
  version        Version to install

${BOLD}EXAMPLES${RESET}:
  $(basename $0) select 0.54.2
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
                if [ $version = "undefined" ]; then
                    version="$1"
                else
                    print_error "Multiple version installation not supported: $1"
                    exit 1
                fi
                shift
                ;;
        esac
    done

    if [[ $version = ${LOCAL_VERSION} ]]
    then
        print_error "Cannot set to '${LOCAL_VERSION}' version, please use dev containers"
        exit 1
    fi

    if [[ $version = "undefined" ]]
    then
        print_error "Please select a version"
        show_select_help
        exit 1
    fi

    add_morloc_bin_to_path

    if [[ -d $HOME/${MORLOC_INSTALL_DIR}/$version ]]
    then
        script_menv "$MORLOC_BIN/menv" $version
        script_morloc_shell "$MORLOC_BIN/morloc-shell" $version
    else
        print_error "Morloc version '$version' does not exist, install first"
        exit 1
    fi

    print_success "Swicted to Morloc version '$version'"
    exit 0
}

# }}}
# {{{ info subcommand

# Help for install subcommand
show_info_help() {
    cat << EOF
${BOLD}USAGE${RESET}: $(basename $0) info

Print info on Morloc versions and check containers

${BOLD}OPTIONS${RESET}:
  -h, --help   Show this help message

${BOLD}EXAMPLES${RESET}:
  $(basename $0) info
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

    versions=$(ls $HOME/${MORLOC_INSTALL_DIR} | grep -v "${LOCAL_VERSION}")

    current_version=$(menv morloc --version)
    if [ $? -ne 0 ]
    then
        print_error "No current Morloc version set"
    fi

    dev_container=${CONTAINER_BASE_TEST}
    if $CONTAINER_ENGINE images --format '{{.Repository}}' | grep -q "^${dev_container}$"
    then
        printf "dev             ${GREEN}container exists${RESET}\n"
    else
        printf "dev             ${RED}container missing${RESET}\n"
    fi

    for version in $versions
    do
        selection="         "
        if [ $version = $current_version ]
        then
            selection=" selected"
        fi

        $0 "select" $version > /dev/null 2>&1
        if [ $? -ne 0 ]
        then
            print_error "Failed to switch to $version"
        fi

        version_container=${CONTAINER_BASE_FULL}:${version}

        if $CONTAINER_ENGINE images --format '{{.Repository}}:{{.Tag}}' | grep -q "^${version_container}$"
        then
            printf "${version}${selection} ${GREEN}container exists${RESET}\n"
        else
            printf "${version}${selection} ${RED}container missing${RESET}\n"
        fi

    done

    # switch back to original version
    $0 "select" $current_version > /dev/null 2>&1

    exit 0
}
# }}}
# {{{ main

# Main argument parsing

main() {
    case "$1" in
        -h|--help)
            show_help
            exit 0
            ;;
        -v|--version)
            show_version
            exit 0
            ;;
        install)
            shift
            cmd_install "$@"
            ;;
        uninstall)
            shift
            cmd_uninstall "$@"
            ;;
        update)
            shift
            cmd_update "$@"
            ;;
        select)
            shift
            cmd_select "$@"
            ;;
        info)
            shift
            cmd_info "$@"
            ;;
        "")
            print_error "No command specified"
            show_help
            exit 1
            ;;
        *)
            print_error "Unknown command: $1"
            show_help
            exit 1
            ;;
    esac
}

# }}}

# Run main function with all arguments
main "$@"
