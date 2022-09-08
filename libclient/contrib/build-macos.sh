#!/bin/bash

set -e
set +x
QUIET="no"

realpath() {
    [[ $1 = /* ]] && echo "$1" || echo "$PWD/${1#./}"
}

# Architecture configuration
BUILD_ARCH=`uname -m | cut -d. -f1`
HOST_ARCH="x86_64"

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
    else
        echo "$1"
    fi
}

get_buildsystem_arch() {
    if [ "$1" = "arm64" ]; then
        echo "aarch64"
    else
        echo "$1"
    fi
}

# Workspace directories
CONTRIB_DIR=$(realpath $(dirname $0))
LIBCLIENT_ROOT=$CONTRIB_DIR/..

autotools_set_env()
{
    case $HOST_ARCH in
        x86_64)
            export AUTOTOOLS_HOST=x86_64-apple-darwin
            export CFLAGS="-arch x86_64"
            export LDFLAGS="-arch x86_64"
        ;;
        arm64)
            export AUTOTOOLS_HOST=aarch64-apple-darwin
            export CFLAGS="-arch arm64"
            export LDFLAGS="-arch arm64"
        ;;
        *)
            echo "Unexpected arch $HOST_ARCH"
            exit 1
        ;;
    esac
}

autotools_clear_env()
{
    unset CFLAGS
    unset LDFLAGS
    unset AUTOTOOLS_HOST
}

spushd()
{
    pushd "$1" > /dev/null
}

spopd()
{
    popd > /dev/null
}

out="/dev/stdout"
if [ "$QUIET" = "yes" ]; then
    out="/dev/null"
fi

panic() {
    echo $1
    exit 1
}

test_sha256() {
    local local_path=$1
    local expected_hash=$2

    local real_hash=$(shasum -a 256 $local_path | awk '{print $1}')

    if [[ $real_hash == $expected_hash ]]; then
        return 0
    else
        echo "sha invalid, have ${real_hash}, expected ${expected_hash}"
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
    curl -f -L -o $local_path $uri
    test_sha256 $local_path $tarball_hash
    if [[ $? -eq 0 ]]; then
        return 0
    fi

    return 1
}

build_ffmpeg() {
    local uri="https://git.ffmpeg.org/ffmpeg.git"
    local sha256="85a6b7f7b746a7bf71146b1530f19f699c4f0fb4"
    local work_dir="$BUILD_DIR/ffmpeg"

    # Get sources
    if ! [ -e $work_dir ]; then
        mkdir $work_dir
    fi
    spushd $work_dir
    if ! [ -e ffmpeg ]; then
        git clone $uri
    fi
    spushd ffmpeg
    git checkout $sha1

    # Build
    if ! [ -e build ]; then
        mkdir build
    fi
    spushd build

    COMMON_FLAGS="--prefix=$OUT_DIR --disable-shared --disable-sdl2 --disable-libxcb --disable-lzma"

    if [ $HOST_ARCH != $BUILD_ARCH ]; then
        case $HOST_ARCH in
            x86_64)
                BUILD_FLAGS="-target x86_64-apple-darwin -arch x86_64"
            ;;
            arm64)
                BUILD_FLAGS="-target arm-apple-darwin -arch arm64"
            ;;
            *)
                echo "Unexpected arch $HOST_ARCH"
                exit 1
            ;;
        esac

        ../configure \
            --enable-cross-compile --target-os=darwin --arch=$HOST_ARCH \
            --extra-cflags="$BUILD_FLAGS" --extra-ldflags="$BUILD_FLAGS" \
            $COMMON_FLAGS

    else
        ../configure \
            $COMMON_FLAGS
    fi

    make -j $NJOBS > $out
    make install > $out

    if [ $HOST_ARCH != $BUILD_ARCH ]; then
        unset CONFIGURE_EXTRA_FLAGS
    fi

    spopd
    spopd
    spopd
}

build_libvlc() {
    VLC_SRCDIR="$LIBCLIENT_ROOT/external/vlc"

    # Get sources
    spushd $VLC_SRCDIR

    # Configure build env
    export VLC_FORCE_KERNELVERSION=19

    #VLC_CONTRIB_SHA1=`./extras/ci/get-contrib-sha.sh`
    VLC_HOST_ARCH=`get_buildsystem_arch $HOST_ARCH`
    VLC_HOST="${VLC_HOST_ARCH}-apple-darwin${VLC_FORCE_KERNELVERSION}"
    VLC_BUILD="${BUILD_ARCH}-apple-darwin${VLC_FORCE_KERNELVERSION}"

    VLC_CONTRIB_SHA1="fc75a318d85232bed66adaf72e70b11deb94f975"
    VLC_CONTRIB_ARTIFACT="https://artifacts.videolan.org/vlc/macos-${HOST_ARCH}/vlc-contrib-${VLC_HOST_ARCH}-apple-darwin${VLC_FORCE_KERNELVERSION}-${VLC_CONTRIB_SHA1}.tar.bz2"

    VLC_CONTRIB_FOLDER="vlc-contrib-${VLC_HOST}-${VLC_BUILD}"
    VLC_BUILD_FOLDER="build-${VLC_HOST}-${VLC_BUILD}"

    OLD_PATH=$PATH
    export PATH=$VLC_SRCDIR/extras/tools/build/bin:$VLC_SRCDIR/contrib/$VLC_HOST/bin:$PATH

    # Build tools
    spushd extras/tools
    ./bootstrap > $out
    make > $out
    spopd

    # Build contrib
    spushd contrib
    if ! [ -e $VLC_CONTRIB_FOLDER ]; then
        mkdir $VLC_CONTRIB_FOLDER
    fi
    if ! [ -e $VLC_BUILD ]; then
        spushd $VLC_CONTRIB_FOLDER

        ../bootstrap --build=$VLC_BUILD --host=$VLC_HOST
        make prebuilt PREBUILT_URL=$VLC_CONTRIB_ARTIFACT > $out

        spopd
    fi
    spopd

    # Build libvlc
    if ! [ -e configure ]; then
        ./bootstrap > $out
    fi
    if ! [ -e ${VLC_BUILD_FOLDER} ]; then
        mkdir ${VLC_BUILD_FOLDER}
    fi

    SLIMFLAG="--disable-vlc --disable-dca --disable-zvbi --disable-vpx --disable-macosx --disable-faad --disable-fribidi --disable-qt --disable-skins2 --disable-vcd --disable-vlm --disable-nls --disable-notify --disable-live555 --enable-swscale --disable-projectm --disable-libass --disable-libxml2 --disable-goom --disable-dvdread --disable-dvdnav --disable-bluray --disable-linsys --disable-libva --disable-gme --disable-tremor --disable-vorbis --disable-fluidsynth --disable-jack --disable-pulse --disable-mtp --disable-ogg --disable-speex --disable-theora --disable-flac --disable-screen --disable-freetype --disable-taglib --disable-smb2 --disable-sparkle --disable-addonmanagermodules --disable-mad --disable-chromecast --disable-lua --disable-medialibrary --disable-a52 --disable-rav1e --disable-jpeg --disable-ncurses --disable-chromaprint"

    export CFLAGS="-g -arch $HOST_ARCH"
    export CXXFLAGS="-g -arch $HOST_ARCH"
    export OBJCFLAGS="-g -arch $HOST_ARCH"
    export LDFLAGS="-arch $HOST_ARCH"

    spushd ${VLC_BUILD_FOLDER}
    ../configure --build=$VLC_BUILD --host=$VLC_HOST --prefix=$OUT_DIR ${SLIMFLAG} > $out
    make -j $NJOBS > $out
    make install -j $NJOBS > $out
    spopd

    unset CFLAGS
    unset CXXFLAGS
    unset OBJCFLAGS
    unset LDFLAGS

    unset VLC_FORCE_KERNELVERSION

    export PATH=$OLD_PATH

    spopd
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
NJOBS=$(sysctl -n hw.ncpu)
HOST_ARCH=`get_actual_arch $HOST_ARCH`
BUILD_ARCH=`get_buildsystem_arch $BUILD_ARCH`
PLATFORM_NAME="${HOST_ARCH}-apple-darwin"

# Intermediate directories
WORK_DIR="$CONTRIB_DIR/work/$PLATFORM_NAME"
BUILD_DIR="$WORK_DIR/build"
SRC_DIR="$WORK_DIR/src"

# Create folders
mkdir -p $SRC_DIR $BUILD_DIR
mkdir -p $OUT_DIR

# Output directory
export PATH=$OUT_DIR/bin:$PATH
export PKG_CONFIG_LIBDIR=$OUT_DIR/lib/pkgconfig:$OUT_DIR/lib/${HOST_ARCH}-apple-darwin/pkgconfig

# Build
build_ffmpeg
build_libvlc
