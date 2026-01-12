# kyctl

Cross-platform remote desktop control tool using QUIC-based streaming.

## Overview

kyctl is a cross-platform remote desktop control application consisting of:
- **controller**: Server/host application that runs on the machine being controlled
- **libclient**: Client application that connects to the controller to view and control the remote desktop

The system uses QUIC protocol for low-latency streaming and supports keyboard, mouse, audio, and video streaming.

## Supported Platforms

- Linux (x86_64)
- Windows (x86_64, cross-compiled from Linux)
- macOS (arm64/Apple Silicon, x86_64/Intel)

## Build

### Get submodules

The project depends on external libraries managed as git submodules:

```bash
git submodule update --init
```

### Linux

#### Debian: Install required system packages

```bash
sudo apt-get install -y \
    build-essential cmake meson ninja-build pkg-config \
    llvm-dev libclang-dev clang libssl-dev \
    autoconf libtool curl git wget unzip \
    python3 python3-setuptools python3-mako python3-venv \
    lua5.4 liblua5.4-dev \
    autopoint gettext bison flex nasm ant ragel help2man \
    libdrm-dev libgbm-dev glslang-dev liblcms2-dev libpulse-dev \
    libxcb-xkb-dev libxkbcommon-dev libxcb-xtest0-dev \
    libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
    libwayland-dev wayland-protocols \
    libssl-dev libevdev-dev libudev-dev libinput-dev libidn11

sudo apt-get build-dep -y vlc
```

Tested distros
* Debian Trixie
* Ubuntu 24.04

#### Arch: Install required system packages

```bash
pacman -S base-devel cmake meson ninja pkg-config llvm clang openssl-1.1 \
    autoconf libtool curl git wget unzip python3 python-setuptools lua gettext \
    bison flex nasm ant ragel help2man libdrm mesa glslang lcms2 pulseaudio  \
    libxcb libxkbcommon xcb-util xcb-util-renderutil wayland wayland-protocols \
    libevdev libinput libidn11 ffmpeg a52dec vulkan-headers
```

#### Install Rust and required Rust tools

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup install 1.89.0 && rustup default 1.89.0
cargo install --locked cargo-c@0.10.15
```

#### Build

```bash
./build-linux.sh
```

The script outputs files in folder `rootfs-x86_64-linux-gnu`.

Note that there are some options in the build script to avoid some build steps:
* `-n`: Skip git fetch/checkout (preserves your local changes)
* `-N`: Fetch sources only (don't build)
* `-f`: Force reset dependencies to pinned version
* `-s`: Skip the dependencies build
* `-d`: Build only native dependencies
* `-o`: Output directory (must be absolute path)

Available build actions (default: build):
* `build` - Build both controller and libclient
* `build-controller` - Build controller only
* `build-libclient` - Build libclient only
* `clippy` - Run clippy on both controller and libclient
* `clippy-controller` - Run clippy on controller only
* `clippy-libclient` - Run clippy on libclient only

### Windows

#### Install required system packages

```bash
sudo apt-get install -y \
    build-essential cmake meson ninja-build pkg-config \
    llvm-dev libclang-dev clang libssl-dev \
    mingw-w64 mingw-w64-tools \
    autoconf libtool curl git wget zip unzip \
    python3 python3-setuptools python3-mako python3-venv \
    lua5.4 gettext bison flex ant ragel help2man nasm
```

#### Install Rust and required Rust tools

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup install 1.89.0 && rustup default 1.89.0
rustup target add x86_64-pc-windows-gnu
cargo install --locked cargo-c@0.10.15
```

#### Build

```bash
./build-win32.sh
```

Note that there are some options in the build script to avoid some build steps:
* `-n`: Skip git fetch/checkout (preserves your local changes)
* `-N`: Fetch sources only (don't build)
* `-f`: Force reset dependencies to pinned version
* `-s`: Skip the dependencies build
* `-d`: Build only native dependencies
* `-H`: Hide console window for controller.exe
* `-o`: Output directory (must be absolute path)

Available build actions (default: build):
* `build` - Build both controller and libclient
* `build-controller` - Build controller only
* `build-libclient` - Build libclient only
* `clippy` - Run clippy on both controller and libclient
* `clippy-controller` - Run clippy on controller only
* `clippy-libclient` - Run clippy on libclient only

### macOS

#### Install required system packages

```bash
brew install \
    m4 nasm openssl@3 pkg-config \
    autoconf automake libtool \
    cmake gettext meson ragel ant help2man
```

#### Install Rust and required Rust tools

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup install 1.89.0 && rustup default 1.89.0
rustup target add x86_64-apple-darwin aarch64-apple-darwin
cargo install --locked cargo-c@0.10.15
```

#### Build

```bash
./build-macos.sh -a <arch>
```

`<arch>` can be:
* `arm64` - Apple Silicon (default)
* `x86_64` - Intel Macs

Note that there are some options in the build script to avoid some build steps:
* `-n`: Skip git fetch/checkout (preserves your local changes)
* `-N`: Fetch sources only (don't build)
* `-f`: Force reset dependencies to pinned version
* `-s`: Skip the dependencies build
* `-d`: Build only native dependencies
* `-k`: Build Kyber dependencies (for CI)
* `-c`: Run clippy (equivalent to 'clippy' action)
* `-a <arch>`: Architecture (x86_64 or arm64, default: arm64)
* `-o <path>`: Output directory (must be absolute path)

Available build actions (default: build):
* `build` - Build both controller and libclient
* `build-controller` - Build controller only
* `build-libclient` - Build libclient only
* `clippy` - Run clippy on both controller and libclient
* `clippy-controller` - Run clippy on controller only
* `clippy-libclient` - Run clippy on libclient only

## Some tips

### My video decoder crashes

Especially on Linux with an Nvidia driver, the video decoding part can crash.

Software decode can be enable with `--software-decode` to workaround the issue.

### I want to run the controller and the client on the same machine

It is possible to connect to `localhost`. However, the client must be started
with `--audio=false --inputs=false` options.
