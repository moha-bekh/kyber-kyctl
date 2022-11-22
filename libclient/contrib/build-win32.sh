#!/bin/bash

set -e

BASE_DIR=$(realpath $(dirname $0))

# Workspace directories
WORKSPACE_ROOT=$BASE_DIR/..
VLC_SRCDIR="$WORKSPACE_ROOT/external/vlc"

# Build configuration
NJOBS=$(grep -c ^processor /proc/cpuinfo)
PLATFORM_NAME="x86_64-w64-mingw32"
CROSS_PREFIX="x86_64-w64-mingw32"

# Intermediate directories
WORK_DIR="$BASE_DIR/work/$PLATFORM_NAME"
BUILD_DIR="$WORK_DIR/build"
SRC_DIR="$WORK_DIR/src"

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build libkynput and its native dependencies.

OPTIONS:
   -h            Show some help
   -o <path>     Install the built binaries in the absolute path
EOF
}

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
    # mingw-w64-tools openjdk-11-jdk-headless
    local init_cwd=$(pwd)

    # Get sources
    cd $WORKSPACE_ROOT/external/vlc

    export EXTRACONTRIBFLAGS=$(cat << EOF
--disable-aom --disable-rav1e --disable-dvdcss --disable-dvdnav --disable-dvdread --disable-theora
--disable-upnp --disable-vncclient --disable-vpx --disable-bluray
--disable-protobuf --disable-protoc --disable-x264 --disable-x265 --disable-live555
--disable-medialibrary --disable-nfs --disable-ass --disable-caca --disable-decklink
--disable-goom --disable-mpg123 --disable-sqlite --disable-ssh2 --disable-twolame --disable-taglib
--disable-lua --disable-luac --disable-aribb24 --disable-fribidi
--disable-a52 --disable-aribb25 --disable-asdcplib --disable-cddb --disable-gsm
--disable-mpcdec --disable-lame --disable-kate --disable-mad --disable-mysofa --disable-projectM
--disable-schroedinger --disable-shout --disable-sidplay2 --disable-zvbi --disable-srt --disable-ncurses
--disable-flac --disable-dav1d --disable-libxml2 --disable-gpg-error
--disable-soxr --disable-chromaprint --disable-spatialaudio
EOF
)

    export EXTRACONFIGFLAGS=$(cat << EOF
--disable-vlc --disable-dca --disable-zvbi --disable-vpx --disable-macosx --disable-faad --disable-fribidi
--disable-qt --disable-skins2 --disable-vcd --disable-vlm --disable-nls --disable-notify --disable-live555
--enable-swscale --disable-projectm --disable-libass --disable-libxml2 --disable-goom --disable-dvdread
--disable-dvdnav --disable-bluray --disable-linsys --disable-libva --disable-gme --disable-tremor --disable-vorbis
--disable-fluidsynth --disable-jack --disable-pulse --disable-mtp --disable-ogg --disable-speex --disable-theora
--disable-flac --disable-screen --disable-freetype --disable-taglib --disable-smb2 --disable-sparkle
--disable-addonmanagermodules --disable-mad --disable-chromecast --disable-lua --disable-medialibrary --disable-a52
--disable-rav1e --disable-jpeg --disable-ncurses --disable-chromaprint --disable-shout --disable-schroedinger
--disable-soxr --disable-spatialaudio --disable-caca
EOF
)

    ./extras/package/win32/build.sh -r -i none -z -a x86_64 -o $OUT_DIR

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
export PKG_CONFIG_ALLOW_CROSS=1
export PKG_CONFIG_LIBDIR=$OUT_DIR/lib/pkgconfig

# Build
build_libvlc

echo "Build completed. Update your PATH"
echo "export PATH=$OUT_DIR/bin:\$PATH"
echo "export PKG_CONFIG_ALLOW_CROSS=1"
echo "export PKG_CONFIG_LIBDIR=$PKG_CONFIG_LIBDIR"
