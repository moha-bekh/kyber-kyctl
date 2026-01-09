#!/bin/bash

set -e
set -x

BASE_DIR=$(realpath $(dirname $0))

# Source common build functions
source "$BASE_DIR/libclient-common.sh"

panic() {
    echo $1
    exit 1
}

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build libclient.

OPTIONS:
   -h            Show some help
   -k            Fetch Kyber dependencies
   -c            Run clippy
   -j <path>     Install JS files in the absolute path
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

while getopts "hkcj:" OPTION
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
        j)
            ACTION="package-js"
            JS_OUT_DIR=$OPTARG
            mkdir -p $JS_OUT_DIR
            ;;
    esac
done
shift $(($OPTIND - 1))

if [ "x$1" != "x" ]; then
    usage
    exit 1
fi

# Get subprojects
if [ "$BUILD_KYBER_DEPS" = true ]; then
    if [[ ! -d kymux ]]; then
        git_clone "kyber-labs/kyber-streamer/kymux.git"
    fi

    if [[ ! -d libkynput ]]; then
        git_clone "kyber-labs/kyber-streamer/libkynput.git"
    fi

    # Setup cargo config
    mkdir -p .cargo
    echo '[patch.crates-io]' > .cargo/config.toml
    echo 'kymux = { path = "kymux/kymux" }' >> .cargo/config.toml
    echo 'kycom = { path = "kymux/kycom" }' >> .cargo/config.toml
    echo 'keycode = { path = "libkynput/external/keycode/keycode" }' >> .cargo/config.toml
    echo 'keycode_macro = { path = "libkynput/external/keycode/keycode_macro" }' >> .cargo/config.toml
    echo 'libkynput = { path = "libkynput/libkynput" }' >> .cargo/config.toml
    echo 'libkynput-rs = { path = "libkynput/libkynput-rs" }' >> .cargo/config.toml
    echo 'libkynput-sys = { path = "libkynput/libkynput-rs/libkynput-sys" }' >> .cargo/config.toml
fi

# Build
case "$ACTION" in
    "build")
        libclient_wasm_do_build
    ;;
    "clippy")
        libclient_wasm_do_clippy
    ;;
    "package-js")
        libclient_wasm_do_package_js "$BASE_DIR" "$JS_OUT_DIR"
    ;;
esac
