#!/bin/bash
# Project Kyber: kycontroller-common.sh
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

# Common build functions for all platforms
# Functions are prefixed by platform (linux_, macos_, win32_) to avoid conflicts

# Linux platform functions
kycontroller_linux_do_build()
{
    local out_dir=$1
    cargo build -p kycontroller --features=auth-jwt,auth-oidc,cli --release
    cp target/release/kycontroller "$out_dir/bin"
    cp target/release/jwt-gen "$out_dir/bin"
}

kycontroller_linux_do_clippy()
{
    cargo clippy -p kycontroller --features=auth-jwt,auth-oidc,cli -- -D warnings
}

kycontroller_linux_do_test()
{
    cargo test -p kycontroller --features=auth-jwt,auth-oidc,cli --release
}

# macOS platform functions
kycontroller_macos_do_build()
{
    local cargo_target=$1
    local out_dir=$2

    # Build
    cargo build \
        -p kycontroller \
        --target "$cargo_target"  \
        --features=auth-jwt,auth-oidc,cli \
        --release

    # Install
    mkdir -p "$out_dir/bin"
    cp "target/$cargo_target/release/kycontroller" "$out_dir/bin"
    cp "target/$cargo_target/release/jwt-gen" "$out_dir/bin"
}

kycontroller_macos_do_clippy()
{
    local cargo_target=$1

    cargo clippy \
        -p kycontroller \
        --target "$cargo_target" \
        --features=auth-jwt,auth-oidc,cli \
        -- -D warnings
}

kycontroller_macos_do_test()
{
    cargo test -p kycontroller
}

# Windows platform functions
kycontroller_win32_do_build()
{
    local out_dir=$1
    local hide_console_feature=$2

    # Build kycontroller package (produces kycontroller.exe and jwt-gen.exe)
    cargo build \
        -p kycontroller \
        --target x86_64-pc-windows-gnu \
        --features=auth-jwt,auth-oidc,cli \
        --release \
        $hide_console_feature

    # Build kyservice package (produces kyservice.exe)
    cargo build \
        -p kyservice \
        --target x86_64-pc-windows-gnu \
        --release

    cp \
        target/x86_64-pc-windows-gnu/release/kycontroller.exe \
        target/x86_64-pc-windows-gnu/release/kyservice.exe \
        target/x86_64-pc-windows-gnu/release/jwt-gen.exe \
        "$out_dir/bin"
}

kycontroller_win32_do_clippy()
{
    # Clippy for kycontroller package
    cargo clippy \
        -p kycontroller \
        --target x86_64-pc-windows-gnu \
        --features=auth-jwt,auth-oidc,cli \
        -- -D warnings

    # Clippy for kyservice package
    cargo clippy \
        -p kyservice \
        --target x86_64-pc-windows-gnu \
        -- -D warnings
}
