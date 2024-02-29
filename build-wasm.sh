#!/bin/bash

set -e
set -x

BASE_DIR=$(realpath $(dirname $0))

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
    echo 'kymux_client = { path = "kymux/kymux_client" }' >> .cargo/config.toml
    echo 'kycom = { path = "kymux/kyproto/kycom" }' >> .cargo/config.toml
    echo 'kynet = { path = "kymux/kyproto/kynet" }' >> .cargo/config.toml
    echo 'kyproto = { path = "kymux/kyproto/kyproto" }' >> .cargo/config.toml
    echo 'keycode = { path = "libkynput/external/keycode/keycode" }' >> .cargo/config.toml
    echo 'keycode_macro = { path = "libkynput/external/keycode/keycode_macro" }' >> .cargo/config.toml
    echo 'libkynput = { path = "libkynput/libkynput" }' >> .cargo/config.toml
    echo 'libkynput-rs = { path = "libkynput/libkynput-rs" }' >> .cargo/config.toml
    echo 'libkynput-sys = { path = "libkynput/libkynput-rs/libkynput-sys" }' >> .cargo/config.toml
fi

# Build
case "$ACTION" in
    "build")
        export RUSTFLAGS=--cfg=web_sys_unstable_apis

        cargo build \
            --target wasm32-unknown-unknown \
            -p libclient \
            --release \
            --features=kymux,web,websocket
    ;;
    "clippy")
        export RUSTFLAGS=--cfg=web_sys_unstable_apis

        cargo clippy \
            --target wasm32-unknown-unknown \
            -p libclient \
            --release \
            --features=kymux,web,websocket \
            -- -D warnings -A clippy::missing_safety_doc
    ;;
    "package-js")
        JS_SRC_DIR=$BASE_DIR/libclient/js
        cp \
            $JS_SRC_DIR/audio_decoder.js \
            $JS_SRC_DIR/audio_demuxer_worker.js \
            $JS_SRC_DIR/audio_worklet.js \
            $JS_SRC_DIR/mutex.js \
            $JS_SRC_DIR/video_decoder.js \
            $JS_SRC_DIR/video_demuxer_worker.js \
            $JS_SRC_DIR/video_renderer_worker.js \
             \
        $JS_OUT_DIR
    ;;
esac
