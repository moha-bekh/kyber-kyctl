#!/bin/bash

set -e
set -x

BASE_DIR=$(realpath $(dirname $0))

usage()
{
cat << EOF
usage: $0 [options] <action>

Build kyctl application for Windows (cross-compiled).

OPTIONS:
   -h            Show some help
   -k            Checkout dependency sources
   -f            Force reset the dependencies to their pinned version
   -s            Skip native dependencies build
   -d            Build only native dependencies
   -o <path>     Install the built binaries in the absolute path (Default: ./rootfs-x86_64-w64-mingw32)
   -w            Hide console window for controller.exe (Windows GUI mode)

ACTIONS:
   build                Build both controller and libclient (default)
   build-controller     Build controller only
   build-libclient      Build libclient only
   clippy               Run clippy on both controller and libclient
   clippy-controller    Run clippy on controller only
   clippy-libclient     Run clippy on libclient only
   test                 Run tests
EOF
}

git_clone() {
    local REPO=$1
    local REF=$2

    # Extract directory name from repo path (e.g., "kyber.stream/core/kymux.git" -> "kymux")
    local DIR_NAME=$(basename ${REPO} .git)

    if [[ -z "${CI_JOB_TOKEN}" ]]; then
        local FULL_REPO_URI="git@gitlab.com:${REPO}"
    else
        local FULL_REPO_URI="https://gitlab-ci-token:${CI_JOB_TOKEN}@gitlab.com/${REPO}"
    fi

    echo "Repo: '${FULL_REPO_URI}'"

    # Clone if directory doesn't exist
    if [[ ! -d ${DIR_NAME} ]]; then
        git clone --recurse-submodules ${FULL_REPO_URI}
    fi

    # Enter directory, fetch and reset
    cd ${DIR_NAME}
    git fetch
    git reset --hard

    # If no ref specified, track remote main branch
    if [[ -z "${REF}" ]]; then
        REF="main"
    fi

    git checkout $REF || exit
    git submodule update --init --recursive
    cd ..
}

generate_kyctl_cargo_config() {
    echo "Generating kyctl .cargo/config.toml"
    mkdir -p .cargo
    cat > .cargo/config.toml << 'EOF'
[patch.crates-io]
avservice-types = { path = "../kymedia/avservice-types" }
inputservice-types = { path = "../kynput/inputservice-types" }
kyaudioreg = { path = "../kymedia/kyaudioreg" }
kycom = { path = "../kymux/kycom" }
kymux = { path = "../kymux/kymux" }
libkypc = { path = "../kyutil/libkypc" }
libkynput = { path = "../kynput/libkynput" }
libkynput-rs = { path = "../kynput/libkynput-rs" }
libkynput-sys = { path = "../kynput/libkynput-rs/libkynput-sys" }
EOF
}

generate_kynput_cargo_config() {
    echo "Generating kynput .cargo/config.toml"
    mkdir -p ../kynput/.cargo
    cat > ../kynput/.cargo/config.toml << 'EOF'
[patch.crates-io]
libkypc = { path = "../kyutil/libkypc" }
kycom = { path = "../kymux/kycom" }
EOF
}

generate_kymedia_cargo_config() {
    echo "Generating kymedia .cargo/config.toml"
    mkdir -p ../kymedia/.cargo
    cat > ../kymedia/.cargo/config.toml << 'EOF'
[patch.crates-io]
libkypc = { path = "../kyutil/libkypc" }
kycom = { path = "../kymux/kycom" }
EOF
}

checkout_deps() {
    # Save current directory and go to parent to clone deps as siblings
    local KYCTL_DIR=$(pwd)
    cd ..

    git_clone "kyber.stream/core/kymux.git"
    git_clone "kyber.stream/core/kyutil.git"
    git_clone "kyber.stream/core/kynput.git"
    git_clone "kyber.stream/core/kymedia.git"

    cd "$KYCTL_DIR"
}

build_deps() {
    # Always regenerate dependency configs if directories exist
    if [ -d "../kynput" ]; then
        generate_kynput_cargo_config
    fi
    if [ -d "../kymedia" ]; then
        generate_kymedia_cargo_config
    fi

    # Build kyaudioreg
    cd ../kymedia
    ./build-win32.sh -o $OUT_DIR build-kyaudioreg
    cd -

    # Build contribs
    cd contrib
    ./build-win32.sh -o $OUT_DIR
    cd -

    # Build kynput
    cd ../kynput
    ./build-win32.sh $CONSOLE_FLAG -o $OUT_DIR
    cd -

    # Build avserver
    cd ../kymedia
    ./build-win32.sh $CONSOLE_FLAG -o $OUT_DIR build-avserver
    cd -
}

FETCH_KYBER_DEPS=false
BUILD_DEPS=true
FORCE=false
ACTION="build"
HIDE_CONSOLE=false
CONSOLE_FLAG=""
OUT_DIR=$(pwd)/rootfs-x86_64-w64-mingw32

while getopts "hkfsdwo:" OPTION
do
    case $OPTION in
        h)
            usage
            exit 1
            ;;
        k)
            FETCH_KYBER_DEPS=true
            ;;
        f)
            FORCE=true
            ;;
        s)
            BUILD_DEPS=false
            ;;
        d)
            ACTION="none"
            ;;
        w)
            HIDE_CONSOLE=true
            ;;
        o)
            OUT_DIR=$OPTARG
            ;;
    esac
done
shift $(($OPTIND - 1))

# Get action from positional argument
if [ "x$1" != "x" ]; then
    ACTION=$1
    shift
fi

# Ensure no extra arguments
if [ "x$1" != "x" ]; then
    echo "Error: Unexpected argument '$1'"
    usage
    exit 1
fi

# Skip dependencies for controller-only builds (controller doesn't need native deps)
if [ "$ACTION" = "build-controller" ] || [ "$ACTION" = "clippy-controller" ]; then
    BUILD_DEPS=false
fi

# Build local repository
export PATH=$OUT_DIR/bin:$PATH
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR=$OUT_DIR/lib/pkgconfig

# Always generate kyctl .cargo/config.toml
generate_kyctl_cargo_config

# Checkout dependencies
if [ "$FETCH_KYBER_DEPS" = true ]; then
    checkout_deps
else
    echo "Skip dependencies source fetch"
fi

# Create output directories
mkdir -p $OUT_DIR/bin
mkdir -p $OUT_DIR/share

# Prepare hide console feature flag
if [ "$HIDE_CONSOLE" = true ]; then
    HIDE_CONSOLE_FEATURE="--features=win32-hide-console"
    CONSOLE_FLAG="-w"
else
    HIDE_CONSOLE_FEATURE=""
    CONSOLE_FLAG=""
fi

# Build dependencies
if [ "$BUILD_DEPS" = true ]; then
    build_deps
else
    echo "Skip dependencies build"
fi

# Source common build functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/controller-common.sh"
source "$SCRIPT_DIR/libclient-common.sh"

# Run action
case "$ACTION" in
    "build")
        controller_win32_do_build "$OUT_DIR" "$HIDE_CONSOLE_FEATURE"
        libclient_win32_do_build "$OUT_DIR"
    ;;
    "build-controller")
        controller_win32_do_build "$OUT_DIR" "$HIDE_CONSOLE_FEATURE"
    ;;
    "build-libclient")
        libclient_win32_do_build "$OUT_DIR"
    ;;
    "clippy")
        controller_win32_do_clippy
        libclient_win32_do_clippy
    ;;
    "clippy-controller")
        controller_win32_do_clippy
    ;;
    "clippy-libclient")
        libclient_win32_do_clippy
    ;;
    "test")
        echo "Test functionality not yet implemented"
    ;;
    "none")
        echo "No action requested"
    ;;
    *)
        echo "Error: Invalid action '$ACTION'"
        echo "Valid actions: build, build-controller, build-libclient, clippy, clippy-controller, clippy-libclient, test"
        usage
        exit 1
    ;;
esac

# Return to script directory
cd "$SCRIPT_DIR"
