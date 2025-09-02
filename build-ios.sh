#!/bin/bash

set -e

realpath() {
    [[ $1 = /* ]] && echo "$1" || echo "$PWD/${1#./}"
}

BASE_DIR=$(realpath $(dirname $0))
TARGET="ios"

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build libclient.

OPTIONS:
   -h            Show some help
   -k            Build Kyber dependencies
   -c            Run clippy
   -o <path>     Install the built binaries in the absolute path
   -t <target>   Use the specified target (default: $TARGET)
EOF
}

panic() {
    echo $1
    exit 1
}

get_cargo_target()
{
    case $1 in
        ios)
            echo "aarch64-apple-ios"
        ;;
        tvos)
            echo "aarch64-apple-tvos"
        ;;
        visionos)
            echo "aarch64-apple-visionos"
        ;;
        *)
            echo "Unexpected arch $1"
            exit 1
        ;;
    esac
}

get_cargoc_flags()
{
    case $1 in
        ios)
            echo ""
        ;;
        tvos)
            echo "-Zbuild-std=std,panic_abort"
        ;;
        visionos)
            echo "-Zbuild-std=std,panic_abort"
        ;;
        *)
            echo "Unexpected arch $1"
            exit 1
        ;;
    esac
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

ACTION="build"

while getopts "hko:t:" OPTION
do
    case $OPTION in
        h)
            usage
            exit 1
            ;;
        k)
            BUILD_KYBER_DEPS=true
            ;;
        o)
            OUT_DIR=$OPTARG
            ;;
        t)
            TARGET=$OPTARG
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

    ./build-ios.sh -t $TARGET -o $OUT_DIR
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

CARGO_TARGET=`get_cargo_target $TARGET`
CARGOC_FLAGS=`get_cargoc_flags $TARGET`

export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR=$OUT_DIR/lib/pkgconfig

# Run action
case "$ACTION" in
    "build")
        # Build
        cargo cinstall -p libclient \
            --target=$CARGO_TARGET \
            $CARGOC_FLAGS \
            --release \
            --library-type=staticlib \
            --prefix=$OUT_DIR \
            --features=desktop,kymux,rtp
    ;;
esac
