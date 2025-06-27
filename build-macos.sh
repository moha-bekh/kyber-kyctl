#!/bin/bash

set -e

# Architecture configuration
BUILD_ARCH=`uname -m | cut -d. -f1`
HOST_ARCH="x86_64"

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build controller.

OPTIONS:
   -h            Show some help
   -k            Build Kyber dependencies
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

BUILD_KYBER_DEPS=false
ACTION="build"

while getopts "hkcto:a:" OPTION
do
    case $OPTION in
        h)
            usage
            exit 1
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

CARGO_TARGET=`get_cargo_target $HOST_ARCH`

# Build kyber dependencies
if [ "$BUILD_KYBER_DEPS" = true ]; then
    if [[ ! -d avserver ]]; then
        git_clone "kyber-labs/kyber-streamer/avserver.git"
    fi

    if [[ ! -d kymux ]]; then
        git_clone "kyber-labs/kyber-streamer/kymux.git"
    fi

    if [[ ! -d libkypc ]]; then
        git_clone "kyber-labs/kyber-streamer/libkypc.git"
    fi

    if [[ ! -d libkynput ]]; then
        git_clone "kyber-labs/kyber-streamer/libkynput.git"
    fi

    # Cargo config
    echo "Add extra cargo configuration to find rust dependencies"

    mkdir -p .cargo
    echo '[patch.crates-io]' > .cargo/config.toml
    echo 'avservice-types = { path = "avserver/avservice-types" }' >> .cargo/config.toml
    echo 'inputservice-types = { path = "libkynput/inputservice-types" }' >> .cargo/config.toml
    echo 'kymux = { path = "kymux/kymux" }' >> .cargo/config.toml
    echo 'libkypc = { path = "libkypc" }' >> .cargo/config.toml
fi

# Run action
case "$ACTION" in
    "build")
        # Build
        cargo build \
            --target $CARGO_TARGET  \
            --features=auth-jwt,cli \
            --release

        # Install
        mkdir -p $OUT_DIR/bin
        cp target/$CARGO_TARGET/release/controller $OUT_DIR/bin
        cp target/$CARGO_TARGET/release/jwt-gen $OUT_DIR/bin
    ;;
    "clippy")
        cargo clippy \
            --target $CARGO_TARGET \
            --features=auth-jwt,cli \
            -- -D warnings
    ;;
    "test")
        cargo test
    ;;
esac
