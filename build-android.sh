#!/bin/bash

set -e
set -x

BASE_DIR=$(realpath $(dirname $0))

# Architecture configuration
HOST_ARCH="arm64"

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build libclient.

OPTIONS:
   -h            Show some help
   -k            Build Kyber dependencies
   -s            Skip native dependencies build
   -c            Run clippy
   -o <path>     Install the built binaries in the absolute path
   -a <arch>     Use the specified arch (default: $HOST_ARCH)
EOF
}

get_actual_arch() {
    if [ "$1" = "aarch64" ]; then
        echo "arm64"
    else
        echo "$1"
    fi
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

BUILD_DEPS=true
BUILD_KYBER_DEPS=false
ACTION="build"

while getopts "hskco:a:" OPTION
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
        a)
            HOST_ARCH=`get_actual_arch $OPTARG`
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
    echo 'kycom = { path = "../kymux/kycom" }' >> .cargo/config.toml
    echo 'libkypc = { path = "../libkypc" }' >> .cargo/config.toml

    ./build-android.sh -a $HOST_ARCH -o $OUT_DIR
    cd -

    # Cargo config
    echo "Add extra cargo configuration to find libkynput-rs"

    mkdir -p .cargo
    echo '[patch.crates-io]' > .cargo/config.toml
    echo 'kymux = { path = "kymux/kymux" }' >> .cargo/config.toml
    echo 'kycom = { path = "kymux/kycom" }' >> .cargo/config.toml
    echo 'keycode = { path = "libkynput/external/keycode/keycode" }' >> .cargo/config.toml
    echo 'keycode_macro = { path = "libkynput/external/keycode/keycode_macro" }' >> .cargo/config.toml
    echo 'libkynput = { path = "libkynput/libkynput" }' >> .cargo/config.toml
    echo 'libkynput-rs = { path = "libkynput/libkynput-rs" }' >> .cargo/config.toml
    echo 'libkynput-sys = { path = "libkynput/libkynput-rs/libkynput-sys" }' >> .cargo/config.toml
    echo 'libkypc = { path = "libkypc" }' >> .cargo/config.toml
fi

# Build dependencies
if [ "$BUILD_DEPS" = true ]; then
    ./contrib/build-android.sh -a $HOST_ARCH -o $OUT_DIR
else
    echo "Skip dependencies build"
fi

# Output directory
export PATH=$OUT_DIR/bin:$ANDROID_NDK/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH
export PKG_CONFIG_LIBDIR=$OUT_DIR/lib/pkgconfig
export PKG_CONFIG_ALLOW_CROSS=1

ANDROID_API=21

case $HOST_ARCH in
    armv7a)
        CARGO_TARGET="armv7-linux-androideabi"
        ANDROID_ARCH="armv7a-linux-androideabi"

        export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="armv7a-linux-androideabi$ANDROID_API-clang"
    ;;
    arm64)
        CARGO_TARGET="aarch64-linux-android"
        ANDROID_ARCH="aarch64-linux-android"

        export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="aarch64-linux-android$ANDROID_API-clang"
    ;;
    *)
        echo "Unexpected arch $1"
        exit 1
    ;;
esac

export TARGET_AR=llvm-ar
export TARGET_CC=$ANDROID_ARCH$ANDROID_API-clang
export RUSTFLAGS="-L native=$ANDROID_NDK/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$ANDROID_ARCH/$ANDROID_API"

# Build
case "$ACTION" in
    "build")
        cargo cinstall \
            --target $CARGO_TARGET \
            -p libclient \
            --release \
            --prefix=$OUT_DIR \
            --features=desktop,kymux,rtp
    ;;
    "clippy")
        cargo clippy \
            --target $CARGO_TARGET \
            --features=capi,desktop,kymux,rtp \
            -- \
            -D warnings -A clippy::missing_safety_doc
    ;;
esac
