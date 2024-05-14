#!/bin/bash

set -e

BASE_DIR=$(realpath $(dirname $0))

# Workspace directories
WORKSPACE_ROOT=$BASE_DIR/..
VLC_SRCDIR="$WORKSPACE_ROOT/external/vlc"
KYAUDIOREG_SRCDIR="$WORKSPACE_ROOT/external/kyaudioreg"

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

build_kyaudioreg() {
    cd "$KYAUDIOREG_SRCDIR"
    cargo cinstall \
        --release \
        --prefix="$OUT_DIR"
    cd -
}

build_libvlc() {
    local init_cwd=$(pwd)

    # Bootstrap
    cd $VLC_SRCDIR
    ./bootstrap

    # Build
    mkdir -p $BUILD_DIR/vlc
    cd $BUILD_DIR/vlc

    CONFIGFLAGS=$(cat << EOF
--disable-dca --disable-zvbi --disable-vpx --disable-faad --disable-fribidi
--disable-qt --disable-skins2 --disable-vcd --disable-vlm --disable-nls --disable-notify --disable-live555
--enable-swscale --disable-projectm --disable-libass --disable-libxml2 --disable-goom --disable-dvdread
--disable-dvdnav --disable-bluray --disable-linsys --disable-gme --disable-tremor --disable-vorbis
--disable-fluidsynth --disable-jack --disable-mtp --disable-speex --disable-theora
--disable-flac --disable-screen --disable-freetype --disable-taglib --disable-smb2 --disable-sparkle
--disable-addonmanagermodules --disable-mad --disable-chromecast --disable-lua --disable-medialibrary --disable-a52
--disable-rav1e --disable-jpeg --disable-ncurses --disable-chromaprint --disable-shout --disable-schroedinger
--disable-soxr --disable-x264 --disable-x26410b --disable-libplacebo
EOF
)

    $VLC_SRCDIR/configure --prefix=$OUT_DIR $CONFIGFLAGS
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
build_kyaudioreg
build_libvlc

echo "Build completed. Update your PATH"
echo "export PATH=$OUT_DIR/bin:\$PATH"
echo "export PKG_CONFIG_PATH=$PKG_CONFIG_PATH"
echo "export LD_LIBRARY_PATH=$OUT_DIR/lib:$OUT_DIR/lib/x86_64-linux-gnu"
