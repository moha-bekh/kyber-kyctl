#!/bin/bash
# Project Kyber: build-wasm.sh
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

set -e
set -x

BASE_DIR=$(realpath $(dirname $0))

usage()
{
cat << EOF
usage: $0 [options] <action>

Build kyctl application for WebAssembly.

OPTIONS:
   -h            Show some help
   -k            Checkout dependency sources
   -f            Force reset the dependencies to their pinned version
   -c            Run clippy
   -o <path>     Install the built binaries in the absolute path (Default: ./rootfs-wasm32)

ACTIONS:
   build                Build libclient (default)
   build-libclient      Build libclient only
   clippy               Run clippy on libclient
   clippy-libclient     Run clippy on libclient only
EOF
}

git_clone() {
    local REPO=$1
    local REF=$2

    # Extract directory name from repo path (e.g., "kyber.stream/core/kymux.git" -> "kymux")
    local DIR_NAME=$(basename ${REPO} .git)

    if [[ -z "${CI_JOB_TOKEN}" ]]; then
        local FULL_REPO_URI="git@gitlab.com:${ROOT_NAMESPACE}/${REPO}"
    else
        local FULL_REPO_URI="https://gitlab-ci-token:${CI_JOB_TOKEN}@gitlab.com/${ROOT_NAMESPACE}/${REPO}"
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
kynputservice-types = { path = "../kynput/kynputservice-types" }
keycode = { path = "../kynput/external/keycode/keycode" }
keycode_macro = { path = "../kynput/external/keycode/keycode_macro" }
kyaudioreg = { path = "../kymedia/kyaudioreg" }
kycom = { path = "../kymux/kycom" }
kymux = { path = "../kymux/kymux" }
libkypc = { path = "../kyutil/libkypc" }
kynput = { path = "../kynput/kynput" }
kynput-rs = { path = "../kynput/kynput-rs" }
kynput-sys = { path = "../kynput/kynput-rs/kynput-sys" }
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

    git_clone "core/kymux.git"
    git_clone "core/kyutil.git"
    git_clone "core/kynput.git"
    git_clone "core/kymedia.git"

    cd "$KYCTL_DIR"
}

FETCH_KYBER_DEPS=false
BUILD_KYBER_DEPS=false
FORCE=false
ACTION="build"

while getopts "hkfcj:" OPTION
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

# Always generate kyctl .cargo/config.toml
generate_kyctl_cargo_config

# Checkout dependencies
if [ "$FETCH_KYBER_DEPS" = true ]; then
    checkout_deps
else
    echo "Skip dependencies source fetch"
fi

# Build kynput WASM if needed
if [ "$BUILD_KYBER_DEPS" = true ]; then
    echo "Building kynput for WASM..."
    cd kynput
    ./build-wasm.sh -k
    cd -
fi

# Source common build functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/libclient-common.sh"

# Run action
case "$ACTION" in
    "build"|"build-libclient")
        libclient_wasm_do_build
    ;;
    "clippy"|"clippy-libclient")
        libclient_wasm_do_clippy
    ;;
    "package-js")
        libclient_wasm_do_package_js "$SCRIPT_DIR" "$JS_OUT_DIR"
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
