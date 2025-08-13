#!/usr/bin/env sh

# Morloc Manager

# {{{ constants and system info

PROGRAM_NAME="morloc-manager"
VERSION="0.1.0"

# ANSI color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# no container found
CONTAINER_ENGINE_VERSION=""
CONTAINER_ENGINE=""

if command -v podman >/dev/null 2>&1; then
    CONTAINER_ENGINE_VERSION=$(podman --version 2>/dev/null)
    CONTAINER_ENGINE="podman"
elif command -v docker >/dev/null 2>&1; then
    CONTAINER_ENGINE_VERSION=$(docker --version 2>/dev/null)
    CONTAINER_ENGINE="docker"
fi


# }}}
# {{{ printing functions

# Print colored output
print_info() {
    printf "${BLUE}[INFO]${NC} %s\n" "$1"
}

print_success() {
    printf "${GREEN}[SUCCESS]${NC} %s\n" "$1"
}

print_warning() {
    printf "${YELLOW}[WARNING]${NC} %s\n" "$1"
}

print_error() {
    printf "${RED}[ERROR]${NC} %s\n" "$1"
}

# }}}
# {{{ main help and version

# Help function
show_help() {
    cat << EOF
${PROGRAM_NAME} ${VERSION} - manage morloc containerized installation

Usage: $0 [OPTIONS] COMMAND [ARGS...]

OPTIONS:
  -h, --help     Show this help message
  -v, --version  Show version information

COMMANDS:
  install    Install morloc
  remove     Remove morloc

Examples:
  $0 install
  $0 remove
  $0 --help
EOF
}

# Version function
show_version() {
    echo "${PROGRAM_NAME} ${VERSION}"
}

# }}}
# {{{ install subcommand

# Help for install subcommand
show_install_help() {
    cat << EOF
show_install_help() {
    cat << EOF
${PROGRAM_NAME} install - install the morloc compiler and environment

Usage: $0 install [OPTIONS] <version>

OPTIONS:
  -h, --help     Show this help message
  -f, --force    Force installation (overwrite existing)

Arguments:
  version        Version to install

Examples:
  $0 install
  $0 install 0.54.2
  $0 install --force
EOF
}

# Install subcommand
cmd_install() {
    force=false
    verbose=false
    version=""
    
    # Parse install subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_install_help
                exit 0
                ;;
            -f|--force)
                force=true
                shift
                ;;
            -*)
                print_error "Unknown option for install: $1"
                show_install_help
                exit 1
                ;;
            *)
                if [ -z "$version" ]; then
                    version="$1"
                else
                    print_error "Multiple version installation not supported: $1"
                    exit 1
                fi
                shift
                ;;
        esac
    done
    
    if [ -z "$version" ]; then
        print_info "Installing Morloc, latest version"
    else
        print_info "Installing Morloc v$version"
    fi
    
    [ "$force" = true ] && print_info "Force mode enabled"
    
    if [ -z "$CONTAINER_ENGINE" ]; then
        print_error "No container engine found, please install podman or docker"
        exit 1
    else
        print_info "Using $CONTAINER_ENGINE $CONTAINER_ENGINE_VERSION as a container engine"
    fi
    
    print_info "Pretending to do installation stuff"

    print_success "Morloc v$version installed successfully"
}

# }}}
# {{{ remove subcommand

# Help for remove subcommand
show_remove_help() {
    cat << EOF
${PROGRAM_NAME} remove - remove morloc

Usage: $0 remove [OPTIONS] <version>

OPTIONS:
  -h, --help     Show this help message

Arguments:
  version        Version to remove (optional, remove everything by default)

Examples:
  $0 remove
  $0 remove 0.52.4
EOF
}

# Remove subcommand
cmd_remove() {
    version=""
    
    # Parse remove subcommand arguments
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help)
                show_remove_help
                exit 0
                ;;
            -*)
                print_error "Unknown option for remove: $1"
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
    
    print_info "Pretending to remove stuff"

    # Add your removal logic here
    if [ -z $version ]
    then
        print_success "Morloc v'$version' removed successfully"
    else
        print_success "Morloc removed successfully"
    fi
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
        remove)
            shift
            cmd_remove "$@"
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
