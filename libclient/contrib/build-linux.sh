#!/bin/bash

set -e

BASE_DIR=$(realpath $(dirname $0))

# Workspace directories
WORKSPACE_ROOT=$BASE_DIR/..
VLC_SRCDIR="$WORKSPACE_ROOT/external/vlc"

# Build configuration
NJOBS=$(grep -c ^processor /proc/cpuinfo)
PLATFORM_NAME="x86_64-linux-gnu"

# Intermediate directories
WORK_DIR="$BASE_DIR/work/$PLATFORM_NAME"
BUILD_DIR="$WORK_DIR/build"
SRC_DIR="$WORK_DIR/src"

# Output directory
OUT_DIR="$BASE_DIR/$PLATFORM_NAME"

export PATH=$OUT_DIR/bin:$PATH
export PKG_CONFIG_PATH=$OUT_DIR/lib/pkgconfig:$OUT_DIR/lib/x86_64-linux-gnu/pkgconfig

panic() {
    echo $1
    exit 1
}

test_sha256() {
    local local_path=$1
    local expected_hash=$2

    local real_hash=$(sha256sum $local_path | awk '{print $1}')

    if [[ $real_hash == $expected_hash ]]; then
        return 0
    else
        return 1
    fi
}

download_tarball() {
    local uri=$1
    local tarball_hash=$2

    local filename=$(basename $uri)
    local local_path="$SRC_DIR/$filename"

    echo "Downloading $local_path"

    # Check if file exists and is valid
    if [[ -f $local_path ]]; then
        test_sha256 $local_path $tarball_hash

        if [[ $? -eq 0 ]]; then
            echo "File already exists and is valid"
            return 0
        fi

        echo "File found, but invalid"
        rm $local_path
    fi

    # Get and check file integrity
    wget $uri -O $local_path
    test_sha256 $local_path $tarball_hash
    if [[ $? -eq 0 ]]; then
        return 0
    fi

    return 1
}

build_libvlc() {
    local init_cwd=$(pwd)

    # Get sources
    cd $BUILD_DIR
    git clone $VLC_SRCDIR
    cd vlc

    # Build
    ./bootstrap
    mkdir build && cd build
    ../configure --disable-vlc --prefix=$OUT_DIR
    make -j $NJOBS
    make install -j $NJOBS

    cd $init_cwd
}

while getopts "ho:" OPTION
do
    case $OPTION in
        h)
            usage
            exit 1
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
mkdir -p $SRC_DIR $BUILD_DIR
mkdir -p $OUT_DIR

# Output directory
export PATH=$OUT_DIR/bin:$PATH
export PKG_CONFIG_PATH=$OUT_DIR/lib/pkgconfig:$OUT_DIR/lib/x86_64-linux-gnu/pkgconfig

# Build
build_libvlc

echo "Build completed. Update your PATH"
echo "export PATH=$OUT_DIR/bin:\$PATH"
echo "export PKG_CONFIG_PATH=$PKG_CONFIG_PATH"
echo "export LD_LIBRARY_PATH=$OUT_DIR/lib:$OUT_DIR/lib/x86_64-linux-gnu"
