#!/bin/bash

set -e

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build controller.

OPTIONS:
   -h            Show some help
   -k            Build Kyber dependencies
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

BUILD_KYBER_DEPS=false
ACTION="build"

while getopts "hkco:" OPTION
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
mkdir -p $OUT_DIR/bin

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

# Source common build functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/controller-common.sh"

# Run action
case "$ACTION" in
    "build-controller")
        controller_linux_do_build "$OUT_DIR"
    ;;
    "clippy-controller")
        controller_linux_do_clippy
    ;;
esac
