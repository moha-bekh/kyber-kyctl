# Project Kyber: build-android.sh
# Copyright © 2022-2026 Kyber SAS
# SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
#
# This file is both under dual license: AGPLv3 and a Commercial one.
#
# ----
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.
#
# You should have received a copy of the GNU Affero General Public License
# along with this program.  If not, see <http://www.gnu.org/licenses/>.

#!/bin/bash

set -e
set -x

BASE_DIR=$(realpath $(dirname $0))

usage()
{
cat << EOF
usage: $0 [options] <action>

Build kyctl application for Android.

OPTIONS:
   -h            Show some help
   -k            Checkout dependency sources
   -f            Force reset the dependencies to their pinned version
   -s            Skip native dependencies build
   -d            Build only native dependencies
   -a <arch>     Architecture (arm64 or armv7a, default: arm64)
   -o <path>     Install the built binaries in the absolute path (Default: ./rootfs-<target>)

ACTIONS:
   build                Build libclient (default)
   build-libclient      Build libclient only
   clippy               Run clippy on libclient
   clippy-libclient     Run clippy on libclient only

Note: Android builds only support libclient, not controller
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

get_actual_arch() {
    if [ "$1" = "aarch64" ]; then
        echo "arm64"
    else
        echo "$1"
    fi
}

get_cargo_target() {
    case $1 in
        armv7a)
            echo "armv7-linux-androideabi"
        ;;
        arm64)
            echo "aarch64-linux-android"
        ;;
        *)
            echo "Unexpected arch $1"
            exit 1
        ;;
    esac
}

get_vlc_target() {
    case $1 in
        armv7a)
            echo "arm-linux-androideabi"
        ;;
        arm64)
            echo "aarch64-linux-android"
        ;;
        *)
            echo "Unexpected arch $1" >&2
            exit 1
        ;;
    esac
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

    VLC_CONTRIB_TARGET=$(get_vlc_target $HOST_ARCH)

    # Build kyaudioreg
    KYAUDIOREG_OUT_DIR="$(pwd)/../kymedia/external/vlc/contrib/$VLC_CONTRIB_TARGET"
    cd ../kymedia
    ./build-android.sh -a $HOST_ARCH -o "$KYAUDIOREG_OUT_DIR" build-kyaudioreg
    cd -

    # Build contribs
    cd contrib
    ./build-android.sh -a $HOST_ARCH -o $OUT_DIR
    cd -

    # Build kynput
    cd ../kynput
    ./build-android.sh -a $HOST_ARCH -o $OUT_DIR
    cd -
}

FETCH_KYBER_DEPS=false
BUILD_DEPS=true
FORCE=false
ACTION="build"
HOST_ARCH="arm64"
OUT_DIR=""

while getopts "hkfsdka:o:" OPTION
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
        a)
            HOST_ARCH=$(get_actual_arch $OPTARG)
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

# Get cargo target for this architecture
CARGO_TARGET=$(get_cargo_target $HOST_ARCH)

# Set default output directory if not specified
if [ -z "$OUT_DIR" ]; then
    OUT_DIR=$(pwd)/rootfs-${CARGO_TARGET}
fi

# Android doesn't support controller builds
if [ "$ACTION" = "build-controller" ] || [ "$ACTION" = "clippy-controller" ]; then
    echo "Error: Android builds only support libclient, not controller"
    usage
    exit 1
fi

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

# Build dependencies
if [ "$BUILD_DEPS" = true ]; then
    build_deps
else
    echo "Skip dependencies build"
fi

# Set up Android environment
if [ -z "$ANDROID_NDK" ]; then
    echo "Error: ANDROID_NDK environment variable must be set"
    exit 1
fi

export PATH=$OUT_DIR/bin:$ANDROID_NDK/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH
export PKG_CONFIG_LIBDIR=$OUT_DIR/lib/pkgconfig
export PKG_CONFIG_ALLOW_CROSS=1

ANDROID_API=21

case $HOST_ARCH in
    armv7a)
        ANDROID_ARCH="armv7a-linux-androideabi"
        export CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER="armv7a-linux-androideabi$ANDROID_API-clang"
    ;;
    arm64)
        ANDROID_ARCH="aarch64-linux-android"
        export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="aarch64-linux-android$ANDROID_API-clang"
    ;;
    *)
        echo "Unexpected arch $HOST_ARCH"
        exit 1
    ;;
esac

export TARGET_AR=llvm-ar
export TARGET_CC=$ANDROID_ARCH$ANDROID_API-clang
export RUSTFLAGS="-L native=$ANDROID_NDK/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$ANDROID_ARCH/$ANDROID_API"

# Source common build functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/libclient-common.sh"

# Run action
case "$ACTION" in
    "build"|"build-libclient")
        libclient_android_do_build "$CARGO_TARGET" "$OUT_DIR"
    ;;
    "clippy"|"clippy-libclient")
        libclient_android_do_clippy "$CARGO_TARGET"
    ;;
    "none")
        echo "No action requested"
    ;;
    *)
        echo "Error: Invalid action '$ACTION'"
        echo "Valid actions: build, build-libclient, clippy, clippy-libclient"
        usage
        exit 1
    ;;
esac

# Return to script directory
cd "$SCRIPT_DIR"
