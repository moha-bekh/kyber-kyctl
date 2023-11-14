#!/bin/bash

set -e
set -x

BASE_DIR=$(realpath $(dirname $0))

# Workspace directories
WORKSPACE_ROOT=$BASE_DIR/..

# Build configuration
NJOBS=$(grep -c ^processor /proc/cpuinfo)
PLATFORM_NAME="x86_64-w64-mingw32"
CROSS_PREFIX="x86_64-w64-mingw32"

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build libkynput and its native dependencies.

OPTIONS:
   -h            Show some help
   -k            Build Kyber dependencies
   -s            Skip native dependencies build
   -c            Run clippy
   -o <path>     Install the built binaries in the absolute path
EOF
}

git_clone() {
    local REPO=$1
    local BRANCH=$2

    if [[ -z "${CI_JOB_TOKEN}" ]]; then
        local FULL_REPO_URI="git@gitlab.com:${REPO}"
    else
        local FULL_REPO_URI="https://gitlab-ci-token:${CI_JOB_TOKEN}@gitlab.com/${REPO}"
    fi

    echo "Repo: '${FULL_REPO_URI}'"
    if [[ -z "${BRANCH}" ]]; then
        git clone --recurse-submodules ${FULL_REPO_URI}
    else
    git clone --recurse-submodules ${FULL_REPO_URI} -b $BRANCH
    fi
}

BUILD_DEPS=true
BUILD_KYBER_DEPS=false
ACTION="build"

while getopts "hskco:" OPTION
do
    case $OPTION in
        h)
            usage
            exit 1
            ;;
        s)
            BUILD_DEPS=false
            ;;
        k)
            BUILD_KYBER_DEPS=true
            ;;
        c)
            ACTION="clippy"
            ;;
        o)
            OUT_DIR=$OPTARG
            ;;
    esac
done
shift $(($OPTIND - 1))

if [ "x$1" != "x" ]; then
    usage
    exit 1
fi

if [ -z ${OUT_DIR} ]; then
    usage
    exit 1
fi

# Create folders
mkdir -p $OUT_DIR

# Build kyber dependencies
if [ "$BUILD_KYBER_DEPS" = true ]; then
    if [[ ! -d kymux ]]; then
        git_clone "kyber-labs/kyber-streamer/kymux.git"
    fi

    if [[ ! -d libkynput ]]; then
        git_clone "kyber-labs/kyber-streamer/libkynput.git"
    fi

    if [[ ! -d libkypc ]]; then
        git_clone "kyber-labs/kyber-streamer/libkypc.git"
    fi

    # Build libkynput
    cd libkynput

    mkdir -p .cargo
    echo '[patch.crates-io]' > .cargo/config.toml
    echo 'kymux_client = { path = "../kymux/kymux_client" }' >> .cargo/config.toml
    echo 'libkypc = { path = "../libkypc" }' >> .cargo/config.toml

    ./build-win32.sh -o $OUT_DIR
    cd -

    # Cargo config
    echo "Add extra cargo configuration to find libkynput-rs"

    mkdir -p .cargo
    echo '[patch.crates-io]' > .cargo/config.toml
    echo 'kymux = { path = "kymux/kymux" }' >> .cargo/config.toml
    echo 'kymux_client = { path = "kymux/kymux_client" }' >> .cargo/config.toml
    echo 'kycom = { path = "kymux/kyproto/kycom" }' >> .cargo/config.toml
    echo 'kynet = { path = "kymux/kyproto/kynet" }' >> .cargo/config.toml
    echo 'kyproto = { path = "kymux/kyproto/kyproto" }' >> .cargo/config.toml
    echo 'keycode = { path = "libkynput/external/keycode/keycode" }' >> .cargo/config.toml
    echo 'keycode_macro = { path = "libkynput/external/keycode/keycode_macro" }' >> .cargo/config.toml
    echo 'libkynput = { path = "libkynput/libkynput" }' >> .cargo/config.toml
    echo 'libkynput-rs = { path = "libkynput/libkynput-rs" }' >> .cargo/config.toml
    echo 'libkynput-sys = { path = "libkynput/libkynput-rs/libkynput-sys" }' >> .cargo/config.toml
    echo 'libkypc = { path = "libkypc" }' >> .cargo/config.toml
fi

# Build dependencies
if [ "$BUILD_DEPS" = true ]; then
    ./contrib/build-win32.sh -o $OUT_DIR
else
    echo "Skip dependencies build"
fi

# Build
export PATH=$OUT_DIR/bin:$PATH
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR=$OUT_DIR/lib/pkgconfig

# Run action
case "$ACTION" in
    "build")
        cargo cinstall -p libclient \
            --release \
            --prefix=$OUT_DIR \
            --target x86_64-pc-windows-gnu \
            --dlltool x86_64-w64-mingw32-dlltool \
            --features=desktop,kymux,rtp

        cargo build \
            --target x86_64-pc-windows-gnu \
            --features=desktop,kymux,rtp
    ;;
    "clippy")
        cargo clippy \
            --target x86_64-pc-windows-gnu \
            --features=capi,desktop,kymux,rtp \
            -- -D warnings -A clippy::missing_safety_doc
    ;;
esac

echo "Build completed. Update your PATH"
echo "export PATH=$OUT_DIR/bin:\$PATH"
echo "export PKG_CONFIG_ALLOW_CROSS=1"
echo "export PKG_CONFIG_LIBDIR=$PKG_CONFIG_LIBDIR"
