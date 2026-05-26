#!/bin/bash
# Project Kyber: build-win32.sh
# Copyright © 2022-2026 Kyber SAS
# SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
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

Build kyctl application for Windows (cross-compiled).

OPTIONS:
   -h            Show some help
   -k            Checkout dependency sources
   -f            Force reset the dependencies to their pinned version
   -s            Skip native dependencies build
   -d            Build only native dependencies
   -o <path>     Install the built binaries in the absolute path (Default: ./rootfs-x86_64-w64-mingw32)
   -w            Hide console window for kycontroller.exe (Windows GUI mode)
   --no-tray     Disable Windows taskbar tray icon (enabled by default)

ACTIONS:
   build                Build both kycontroller and kyclient (default)
   build-kycontroller   Build kycontroller only
   build-kyclient      Build kyclient only
   clippy               Run clippy on both kycontroller and kyclient
   clippy-kycontroller  Run clippy on kycontroller only
   clippy-kyclient     Run clippy on kyclient only
   test                 Run tests
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

checkout_deps() {
    git_clone "core/kymux.git"
    git_clone "core/kyutil.git"
    git_clone "core/kynput.git"
    git_clone "core/kymedia.git"

    echo "Generating .cargo/config.toml"
    mkdir -p .cargo
    cat > .cargo/config.toml << 'EOF'
[patch.crates-io]
kyavservice-types = { path = "./kymedia/kyavservice-types" }
kynputservice-types = { path = "./kynput/kynputservice-types" }
kyaudioreg = { path = "./kymedia/kyaudioreg" }
kycom = { path = "./kymux/kycom" }
kymux = { path = "./kymux/kymux" }
libkypc = { path = "./kyutil/libkypc" }
kynput = { path = "./kynput/kynput" }
kynput-rs = { path = "./kynput/kynput-rs" }
kynput-sys = { path = "./kynput/kynput-rs/kynput-sys" }
vlc-rs = { path = "./kymedia/subprojects/vlc-rs" }
EOF
}

build_deps() {

    # Build kymedia
    cd ./kymedia
    ./build-win32.sh $CONSOLE_FLAG -o $OUT_DIR
    cd -

    # Build kynput
    cd ./kynput
    ./build-win32.sh $CONSOLE_FLAG -o $OUT_DIR
    cd -
}

FETCH_KYBER_DEPS=false
BUILD_DEPS=true
FORCE=false
ACTION="build"
HIDE_CONSOLE=false
ENABLE_TRAY=true
CONSOLE_FLAG=""
OUT_DIR=$(pwd)/rootfs-x86_64-w64-mingw32

# Handle long options first
for arg in "$@"; do
    case $arg in
        --no-tray)
            ENABLE_TRAY=false
            ;;
    esac
done

# Filter out long options for getopts
ARGS=()
for arg in "$@"; do
    case $arg in
        --no-tray)
            ;;
        *)
            ARGS+=("$arg")
            ;;
    esac
done
set -- "${ARGS[@]}"

while getopts "hkfsdwo:" OPTION
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
        w)
            HIDE_CONSOLE=true
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

# Skip dependencies for kycontroller-only builds (kycontroller doesn't need native deps)
if [ "$ACTION" = "build-kycontroller" ] || [ "$ACTION" = "clippy-kycontroller" ]; then
    BUILD_DEPS=false
fi

# Build local repository
export PATH=$OUT_DIR/bin:$PATH
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR=$OUT_DIR/lib/pkgconfig

# Checkout dependencies
if [ "$FETCH_KYBER_DEPS" = true ]; then
    checkout_deps
else
    echo "Skip dependencies source fetch"
fi

# Create output directories
mkdir -p $OUT_DIR/bin
mkdir -p $OUT_DIR/share

# Prepare feature flags
EXTRA_FEATURES=""
if [ "$HIDE_CONSOLE" = true ]; then
    EXTRA_FEATURES="${EXTRA_FEATURES},win32-hide-console"
    CONSOLE_FLAG="-w"
fi
if [ "$ENABLE_TRAY" = true ]; then
    EXTRA_FEATURES="${EXTRA_FEATURES},win32-tray"
fi
HIDE_CONSOLE_FEATURE=""
if [ -n "$EXTRA_FEATURES" ]; then
    HIDE_CONSOLE_FEATURE="--features=${EXTRA_FEATURES#,}"
fi

# Build dependencies
if [ "$BUILD_DEPS" = true ]; then
    build_deps
else
    echo "Skip dependencies build"
fi

# Source common build functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/kycontroller-common.sh"
source "$SCRIPT_DIR/kyclient-common.sh"

# Run action
case "$ACTION" in
    "build")
        kycontroller_win32_do_build "$OUT_DIR" "$HIDE_CONSOLE_FEATURE"
        kyclient_win32_do_build "$OUT_DIR"
    ;;
    "build-kycontroller")
        kycontroller_win32_do_build "$OUT_DIR" "$HIDE_CONSOLE_FEATURE"
    ;;
    "build-kyclient")
        kyclient_win32_do_build "$OUT_DIR"
    ;;
    "clippy")
        kycontroller_win32_do_clippy
        kyclient_win32_do_clippy
    ;;
    "clippy-kycontroller")
        kycontroller_win32_do_clippy
    ;;
    "clippy-kyclient")
        kyclient_win32_do_clippy
    ;;
    "test")
        echo "Test functionality not yet implemented"
    ;;
    "none")
        echo "No action requested"
    ;;
    *)
        echo "Error: Invalid action '$ACTION'"
        echo "Valid actions: build, build-kycontroller, build-kyclient, clippy, clippy-kycontroller, clippy-kyclient, test"
        usage
        exit 1
    ;;
esac

# Return to script directory
cd "$SCRIPT_DIR"
