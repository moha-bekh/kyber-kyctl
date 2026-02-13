#!/bin/bash
# Project Kyber: kyclient-common.sh
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

# Common build functions for all platforms
# Functions are prefixed by platform (linux_, win32_, macos_, android_, ios_, wasm_) to avoid conflicts

# Linux platform functions
kyclient_linux_do_build()
{
    local out_dir=$1

    cargo cinstall \
        -p kyclient \
        --release \
        --prefix="$out_dir" \
        --features=desktop,kymux,rtp

    cargo build \
        -p kyclient \
        --release \
        --features=desktop,kymux,rtp
}

kyclient_linux_do_clippy()
{
    cargo clippy \
        -p kyclient \
        --features=capi,desktop,kymux,rtp \
        -- -D warnings -A clippy::missing_safety_doc
}

# Win32 platform functions
kyclient_win32_do_build()
{
    local out_dir=$1

    cargo cinstall \
        -p kyclient \
        --release \
        --prefix="$out_dir" \
        --target x86_64-pc-windows-gnu \
        --dlltool x86_64-w64-mingw32-dlltool \
        --features=desktop,kymux,rtp

    cargo build \
        -p kyclient \
        --target x86_64-pc-windows-gnu \
        --release \
        --features=desktop,kymux,rtp
}

kyclient_win32_do_clippy()
{
    cargo clippy \
        -p kyclient \
        --target x86_64-pc-windows-gnu \
        --features=capi,desktop,kymux,rtp \
        -- -D warnings -A clippy::missing_safety_doc
}

# macOS platform functions
kyclient_macos_do_build()
{
    local cargo_target=$1
    local out_dir=$2

    cargo cinstall \
        -p kyclient \
        --target "$cargo_target" \
        --release \
        --prefix="$out_dir" \
        --features=desktop,kymux,rtp

    cargo build \
        -p kyclient \
        --target "$cargo_target" \
        --release \
        --features=desktop,kymux,rtp
}

kyclient_macos_do_clippy()
{
    local cargo_target=$1

    cargo clippy \
        -p kyclient \
        --target "$cargo_target" \
        --features=capi,desktop,kymux,rtp \
        -- -D warnings -A clippy::missing_safety_doc
}

kyclient_macos_do_test()
{
    cargo test -p kyclient
}

# Android platform functions
kyclient_android_do_build()
{
    local cargo_target=$1
    local out_dir=$2

    cargo cinstall \
        --target "$cargo_target" \
        -p kyclient \
        --release \
        --prefix="$out_dir" \
        --features=desktop,kymux,rtp
}

kyclient_android_do_clippy()
{
    local cargo_target=$1

    cargo clippy \
        -p kyclient \
        --target "$cargo_target" \
        --features=capi,desktop,kymux,rtp \
        -- -D warnings -A clippy::missing_safety_doc
}

# iOS platform functions
kyclient_ios_do_build()
{
    local cargo_target=$1
    local cargoc_flags=$2
    local out_dir=$3

    cargo cinstall \
        -p kyclient \
        --target="$cargo_target" \
        $cargoc_flags \
        --release \
        --library-type=staticlib \
        --prefix="$out_dir" \
        --features=desktop,kymux,rtp
}

# WASM platform functions
kyclient_wasm_do_build()
{
    export RUSTFLAGS=--cfg=web_sys_unstable_apis

    cargo build \
        --target wasm32-unknown-unknown \
        -p kyclient \
        --release \
        --features=kymux,web,websocket
}

kyclient_wasm_do_clippy()
{
    export RUSTFLAGS=--cfg=web_sys_unstable_apis

    cargo clippy \
        --target wasm32-unknown-unknown \
        -p kyclient \
        --release \
        --features=kymux,web,websocket \
        -- -D warnings -A clippy::missing_safety_doc -A unexpected-cfgs
}

kyclient_wasm_do_package_js()
{
    local base_dir=$1
    local js_out_dir=$2

    local js_src_dir="$base_dir/kywebplayer/js"

    cp \
        "$js_src_dir/audio_worklet.js" \
        "$js_src_dir/spinlock.js" \
        "$js_src_dir/video_renderer_worker.js" \
        "$js_out_dir"
}
