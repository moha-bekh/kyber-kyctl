#!/bin/bash

set -e

realpath() {
    [[ $1 = /* ]] && echo "$1" || echo "$PWD/${1#./}"
}

# Architecture configuration
BUILD_ARCH=`uname -m | cut -d. -f1`
HOST_ARCH="x86_64"

BASE_DIR=$(realpath $(dirname $0))

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build libkynput.

OPTIONS:
   -h            Show some help
   -s            Skip native dependencies build
   -c            Run clippy
   -t            Run tests
   -o <path>     Install the built binaries in the absolute path
   -a <arch>     Use the specified arch (default: $HOST_ARCH)
EOF
}

panic() {
    echo $1
    exit 1
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

get_cargo_target()
{
    case $1 in
        x86_64)
            echo "x86_64-apple-darwin"
        ;;
        arm64)
            echo "aarch64-apple-darwin"
        ;;
        *)
            echo "Unexpected arch $1"
            exit 1
        ;;
    esac
}

BUILD_DEPS=true
BUILD_KYBER_DEPS=false
ACTION="build"

while getopts "hskcto:a:" OPTION
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
        t)
            ACTION="test"
            ;;
        o)
            OUT_DIR=$OPTARG
            ;;
        a)
            HOST_ARCH=$OPTARG
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

    ./build-macos.sh -a $HOST_ARCH -o $OUT_DIR
    cd -

    # Cargo config
    echo "Add extra cargo configuration to find libkynput-rs"

    mkdir -p .cargo
    echo '[patch.crates-io]' > .cargo/config.toml
    echo 'kymux = { path = "kymux/kymux" }' >> .cargo/config.toml
    echo 'kymux_client = { path = "kymux/kymux_client" }' >> .cargo/config.toml
    echo 'libkynput-rs = { path = "libkynput/libkynput-rs" }' >> .cargo/config.toml
    echo 'libkynput-sys = { path = "libkynput/libkynput-rs/libkynput-sys" }' >> .cargo/config.toml
    echo 'libkypc = { path = "libkypc" }' >> .cargo/config.toml
fi

# Build dependencies
if [ "$BUILD_DEPS" = true ]; then
    ./libclient/contrib/build-macos.sh -a $HOST_ARCH -o $OUT_DIR
else
    echo "Skip dependencies build"
fi

# Output directory
export PATH=$OUT_DIR/bin:$PATH
export PKG_CONFIG_PATH=$OUT_DIR/lib/pkgconfig

# Prepare cargo configuration
if [ $HOST_ARCH != $BUILD_ARCH ]; then
    export PKG_CONFIG_ALLOW_CROSS=1
fi

CARGO_TARGET=`get_cargo_target $HOST_ARCH`

# Run action
case "$ACTION" in
    "build")
        # Build
        cargo cinstall -p libclient \
            --target $CARGO_TARGET \
            --release \
            --prefix=$OUT_DIR

        cargo build \
            --target $CARGO_TARGET  \
            --release
    ;;
    "clippy")
        cargo clippy \
            --target $CARGO_TARGET \
            --features capi -- -D warnings -A clippy::missing_safety_doc
    ;;
    "test")
        cargo test
    ;;
esac
