#!/bin/bash

set -e

usage()
{
cat << EOF
usage: $0 -o <path> [options]

Build controller.

OPTIONS:
   -h            Show some help
   -c            Run clippy
   -o <path>     Install the built binaries in the absolute path
EOF
}

ACTION="build"

while getopts "hco:" OPTION
do
    case $OPTION in
        h)
            usage
            exit 1
            ;;
        c)
            ACTION="clippy"
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
mkdir -p $OUT_DIR/bin

# Run action
case "$ACTION" in
    "build")
        cargo build --release
        cp target/release/controller $OUT_DIR/bin
        cp controller/txproto_stream.lua $OUT_DIR/bin
    ;;
    "clippy")
        cargo clippy -- -D warnings
    ;;
esac
