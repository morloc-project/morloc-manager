#!/usr/bin/env sh

# Morloc Manager

# {{{ constants and system info

PROGRAM_NAME="morloc-manager"
VERSION="0.1.0"

# Only print in color if we are attached to a tty
if [ -t 2 ]; then
    # ANSI color codes
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    BLUE='\033[0;34m'
    NC='\033[0m' # No Color
else
    BLUE=""; GREEN=""; YELLOW=""; RED=""; NC=""
fi

# no container found
CONTAINER_ENGINE_VERSION=""
CONTAINER_ENGINE=""

SHARED_MEMORY_SIZE=4g

CONTAINER_BASE_FULL=ghcr.io/morloc-project/morloc/morloc-full
CONTAINER_BASE_TINY=ghcr.io/morloc-project/morloc/morloc-tiny
CONTAINER_BASE_TEST=ghcr.io/morloc-project/morloc/morloc-test

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
           -v \$HOME/.morloc/$tag:\$HOME/.morloc \\
           -v \$PWD:\$HOME \\
           -w \$HOME \\
           ghcr.io/morloc-project/morloc/morloc-full:$tag "\$@"
EOF

    if [ $? -ne 0 ]
    then
        print_error "Failed to get run `menv morloc --version`"
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
           -v \$HOME/.morloc/$tag:\$HOME/.morloc \\
           -v \$PWD:\$HOME \\
           -w \$HOME \\
           ghcr.io/morloc-project/morloc/morloc-full:$tag /bin/bash
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
    tag="local"

    print_info "Creating menv-dev at '$script_path'"

    mock_home=".morloc/$tag/home"
    mkdir -p "$HOME/$mock_home/.stack"
    mkdir -p "$HOME/$mock_home/.local/bin"
    cat << EOF > "$script_path"
# automatically generated script, do not modify
$CONTAINER_ENGINE run --shm-size=$SHARED_MEMORY_SIZE \\
           --rm \\
           -e HOME=\$HOME \\
           -e PATH="\$HOME/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \\
           -v \$HOME/.morloc/$tag:\$HOME/.morloc \\
           -v \$HOME/$mock_home/.stack:\$HOME/.stack \\
           -v \$HOME/$mock_home/.local/bin:\$HOME/.local/bin \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           ghcr.io/morloc-project/morloc/morloc-test "\$@"
EOF
    chmod 755 $script_path
}

script_morloc_dev_shell() {
    script_path=$1
    tag="local"
    mock_home=".morloc/$tag/home"

    print_info "Creating dev shell at '$script_path'"

    mkdir -p "$HOME/$mock_home/.stack"
    mkdir -p "$HOME/$mock_home/.local/bin"
    cat << EOF > "$script_path"
# automatically generated script, do not modify
$CONTAINER_ENGINE run --shm-size=$SHARED_MEMORY_SIZE \\
           --rm \\
           -it \\
           -e HOME=\$HOME \\
           -e PATH="\$HOME/$mock_home/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" \\
           -v \$HOME/.morloc:\$HOME/.morloc \\
           -v \$HOME/$mock_home/.local/bin:\$HOME/.local/bin \\
           -v \$HOME/$mock_home/.stack:\$HOME/.stack \\
           -v \$PWD:\$HOME/work \\
           -w \$HOME/work \\
           ghcr.io/morloc-project/morloc/morloc-test /bin/bash
EOF
    chmod 755 $script_path
}

# }}}
# {{{ main help and version

# Help function
show_help() {
    cat << EOF
${PROGRAM_NAME} ${VERSION} - manage morloc containerized installation

USAGE: $0 [OPTIONS] COMMAND [ARGS...]

OPTIONS:
  -h, --help     Show this help message
  -v, --version  Show version information

COMMANDS:
  install    Install morloc containers, scripts, and home
  uninstall  Remove morloc containers, scripts, and home

EXAMPLES:
  $0 install
  $0 uninstall
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
USAGE: $0 install [OPTIONS] <version>

Setup morloc containers, scripts, and home for either the latest version
of Morloc or for the specified version.

Creates four executable scripts:

 1. menv: runs commands in a Morloc container. Examples:
    $ menv morloc make -o foo foo.loc
    $ menv ./foo double 21

 2. morloc-shell: enter the "full" container in a shell
    - contains Python, R, and C++ compiler
    - contains vim and other conveniences

 3. menv-dev: runs commands in a dev container
    - contains Haskell tools for building from source
    - can access to all system executables

 4. morloc-shell-dev: enter the dev shell

OPTIONS:
  -h, --help     Show this help message
  -f, --force    Force installation (overwrite existing)

ARGUMENTS:
  version        Version to install

EXAMPLES:
  $0 install
  $0 install 0.54.2
  $0 install --force
EOF
}

# Install subcommand
cmd_install() {
    force=false
    verbose=false

    # calling these "undefined" instead of empty strings for better debugging
    version="undefined"
    tag="undefined"
    local_bin="undefined"

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
            -b|--local-bin)
                shift
                local_bin="$1"
                shift
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

    [ "$force" = true ] && print_info "Force mode enabled"

    if [ $version = "undefined" ]; then
        print_info "No version specified, will retrieve latest version"
        tag="edge"
    else
        print_info "Installing Morloc v$version"
        tag=$version
    fi

    # check if an appropriate container engine is installed
    if [ -z "$CONTAINER_ENGINE" ]; then
        print_error "No container engine found, please install podman or docker"
        exit 1
    else
        print_info "Using $CONTAINER_ENGINE $CONTAINER_ENGINE_VERSION as a container engine"
    fi

    print_info "Attempting to pull containers for Morloc version $version"

    $CONTAINER_ENGINE pull $CONTAINER_BASE_TINY:${tag}
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull tiny container"
        exit 1
    fi

    # pull container
    $CONTAINER_ENGINE pull $CONTAINER_BASE_FULL:${tag}
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull full container"
        exit 1
    fi

    $CONTAINER_ENGINE pull $CONTAINER_BASE_TEST
    if [ $? -ne 0 ]
    then
        print_error "Failed to pull dev container"
        exit 1
    fi

    # get Morloc version from container
    # filter out the carriage return that podman helpfully provided
    detected_version=$(podman run -it $CONTAINER_BASE_FULL:edge morloc --version | tr -d '\r\n')
    if [ $? -ne 0 ]
    then
        print_error "Failed to detect version from morloc container"
        exit 1
    fi

    if [ $detected_version = "" ]
    then
        print_error "No Morloc version found - something went wrong"
    fi

    if [ $version -ne "" && $version -ne $detected_version ]
    then
        print_error "Expected the retrieved morloc version to '${version}', found '${detected_version}'"
    else
        print_success "Retrieved containers for Morloc version ${version}"
    fi

    version=$detected_version

    # check if a local path for storing exectuables is has been given
    # if not, define a default
    if [ $local_bin = "undefined" ]
    then
        local_bin="$HOME/.local/bin"
        print_info "No local path for executables given, defaulting to '${local_bin}'"
    fi

    # check if local executable path is present, if not create it
    if [ -d "$local_bin" ]
    then
        print_info "Local path '$local_bin' found"
    else
        print_error "Local path '$local_bin' not found, please create and add to PATH or choose a different path with option -b/--local-bin"
        exit 1
    fi

    # check if local executable path is in PATH, if not die
    if [[ ":$PATH:" == *":$local_bin:"* ]]; then
        print_info "'$local_bin' found in PATH."
    else
        print_error "'$local_bin' not found in PATH."
        exit 1
    fi

    morloc_home="$HOME/.morloc/$version"

    print_info "Setting Morloc home to '${morloc_home}'"

    # check to see if this morloc version is already installed
    if [ -d "$HOME/.morloc/$version" ]
    then
        if [ "$force" = false ]
        then
            print_success "Morloc v$version is already installed, exiting"
            exit 0
        else
            print_info "Morloc v$version is already installed, overwriting"
        fi
    fi

    # create .morloc/$version folder
    mkdir -p $morloc_home
    if [ $? -ne 0 ]
    then
        print_error "Failed to create morloc home directory at '$morloc_home'"
        exit 1
    fi

    print_info "Created $morloc_home"

    # create morloc scripts
    script_menv "$local_bin/menv" $version
    script_morloc_shell "$local_bin/morloc-shell" $version
    script_menv_dev "$local_bin/menv-dev"
    script_morloc_dev_shell "$local_bin/morloc-shell-dev"

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
USAGE: $0 uninstall [OPTIONS] <version>

Remove Morloc home (or specfic versions) and all associated containers

OPTIONS:
  -h, --help     Show this help message

ARGUMENTS:
  version        Version to remove (optional, remove everything by default)

EXAMPLES:
  $0 uninstall 
  $0 uninstall 0.52.4
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
        morloc_home="$HOME/.morloc"
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
        morloc_home="$HOME/.morloc/$version"
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

    print_success "Removed containers and Morloc home, scripts in ~/.local/bin remain"
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
