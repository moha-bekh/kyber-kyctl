# Project Kyber: build-linux.sh
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

Build kyctl application.

OPTIONS:
   -h            Show some help
   -k            Checkout dependency sources
   -f            Force reset the dependencies to their pinned version
   -s            Skip native dependencies build
   -d            Build only native dependencies
   -o <path>     Install the built binaries in the absolute path (Default: ./rootfs-x86_64-linux-gnu)

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
vlc-rs = { path = "../kymedia/external/vlc-rs" }
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

    # Always regenerate dependency configs if directories exist
    if [ -d "../kynput" ]; then
        generate_kynput_cargo_config
    fi
    if [ -d "../kymedia" ]; then
        generate_kymedia_cargo_config
    fi
}

build_deps() {
    # Always regenerate dependency configs if directories exist
    if [ -d "../kynput" ]; then
        generate_kynput_cargo_config
    fi
    if [ -d "../kymedia" ]; then
        generate_kymedia_cargo_config
    fi

    # Build kymedia
    cd ../kymedia
    ./build-linux.sh -o $OUT_DIR
    cd -

    # Build kynput
    cd ../kynput
    ./build-linux.sh -o $OUT_DIR
    cd -
}

FETCH_KYBER_DEPS=false
BUILD_DEPS=true
FORCE=false
ACTION="build"
OUT_DIR=$(pwd)/rootfs-x86_64-linux-gnu

while getopts "hkfsdpo:" OPTION
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
export LD_LIBRARY_PATH=$OUT_DIR/lib:$OUT_DIR/lib/x86_64-linux-gnu:$OUT_DIR/lib64
export PKG_CONFIG_PATH=$OUT_DIR/lib/pkgconfig:$OUT_DIR/lib/x86_64-linux-gnu/pkgconfig:$OUT_DIR/lib64/pkgconfig

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

# Source common build functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/controller-common.sh"
source "$SCRIPT_DIR/libclient-common.sh"

# Run action
case "$ACTION" in
    "build")
        controller_linux_do_build "$OUT_DIR"
        libclient_linux_do_build "$OUT_DIR"
    ;;
    "build-controller")
        controller_linux_do_build "$OUT_DIR"
    ;;
    "build-libclient")
        libclient_linux_do_build "$OUT_DIR"
    ;;
    "clippy")
        controller_linux_do_clippy
        libclient_linux_do_clippy
    ;;
    "clippy-controller")
        controller_linux_do_clippy
    ;;
    "clippy-libclient")
        libclient_linux_do_clippy
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
