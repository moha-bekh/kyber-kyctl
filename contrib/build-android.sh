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

BASE_DIR=$(realpath $(dirname $0))

# Workspace directories
WORKSPACE_ROOT=$BASE_DIR/..
VLC_SRCDIR="$WORKSPACE_ROOT/external/vlc"
VLCJNI_SRCDIR="$WORKSPACE_ROOT/external/libvlcjni"

# Build configuration
NJOBS=$(grep -c ^processor /proc/cpuinfo)

# Architecture configuration
HOST_ARCH="arm64"

usage()
{
cat << EOF
usage: $0 [options]

Build contribs for macOS

OPTIONS:
   -h            Show some help
   -o <path>     Install the built binaries in the absolute path
   -a <arch>     Use the specified arch (default: $HOST_ARCH)
EOF
}

get_actual_arch() {
    if [ "$1" = "aarch64" ]; then
        echo "arm64"
    elif [ "$1" = "armv7a" ]; then
        echo "arm"
    else
        echo "$1"
    fi
}

get_vlc_arch() {
    if [ "$1" = "arm64" ]; then
        echo "aarch64-linux-android"
    elif [ "$1" = "arm" ]; then
        echo "arm-linux-androideabi"
    else
        echo "Invalid arch $1"
        exit 1
    fi
}

get_android_arch() {
    if [ "$1" = "arm64" ]; then
        echo "arm64-v8a"
    elif [ "$1" = "arm" ]; then
        echo "armeabi-v7a"
    else
        echo "Invalid arch $1"
        exit 1
    fi
}

panic() {
    echo $1
    exit 1
}

build_libvlc() {
    local init_cwd=$(pwd)

    cd $VLC_SRCDIR

    # libvlcjni must be placed in vlc tree
    cp -a $VLCJNI_SRCDIR .
    libvlcjni/buildsystem/compile-libvlc.sh -a $HOST_ARCH

    # Install
    mkdir -p $OUT_DIR/include $OUT_DIR/lib/pkgconfig
    cp -a build-android-`get_vlc_arch $HOST_ARCH`/install/include/vlc $OUT_DIR/include
    cp -a libvlcjni/libvlc/jni/libs/`get_android_arch $HOST_ARCH`/libvlc.so $OUT_DIR/lib

    cat << EOF > $OUT_DIR/lib/pkgconfig/libvlc.pc
prefix=$OUT_DIR
exec_prefix=\${prefix}
libdir=\${exec_prefix}/lib
includedir=\${prefix}/include

Name: libvlc
Description:
Version: 4.0.0
Libs: -L\${libdir} -lvlc
Cflags: -I\${includedir}
EOF

    cd $init_cwd
}

while getopts "ho:a:" OPTION
do
    case $OPTION in
        h)
            usage
            exit 1
            ;;
        o)
            OUT_DIR=$OPTARG
            ;;
        a)
            HOST_ARCH=$OPTARG
        ;;
        *)
            usage
            exit 1
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

# Build configuration
HOST_ARCH=`get_actual_arch $HOST_ARCH`
PLATFORM_NAME="${HOST_ARCH}-linux-android"
WORK_DIR="$BASE_DIR/work/$PLATFORM_NAME"
BUILD_DIR="$WORK_DIR/build"
SRC_DIR="$WORK_DIR/src"

# Create folders
mkdir -p $SRC_DIR $BUILD_DIR $OUT_DIR

# Setup build env
export PKG_CONFIG_PATH=$OUT_DIR/lib/pkgconfig

# Build
build_libvlc
