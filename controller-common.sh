# Project Kyber: controller-common.sh
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
# Common build functions for all platforms
# Functions are prefixed by platform (linux_, macos_, win32_) to avoid conflicts

# Linux platform functions
controller_linux_do_build()
{
    local out_dir=$1
    cargo build -p controller --features=auth-jwt,cli --release
    cp target/release/controller "$out_dir/bin"
    cp target/release/jwt-gen "$out_dir/bin"
}

controller_linux_do_clippy()
{
    cargo clippy -p controller --features=auth-jwt,cli -- -D warnings
}

# macOS platform functions
controller_macos_do_build()
{
    local cargo_target=$1
    local out_dir=$2

    # Build
    cargo build \
        -p controller \
        --target "$cargo_target"  \
        --features=auth-jwt,cli \
        --release

    # Install
    mkdir -p "$out_dir/bin"
    cp "target/$cargo_target/release/controller" "$out_dir/bin"
    cp "target/$cargo_target/release/jwt-gen" "$out_dir/bin"
}

controller_macos_do_clippy()
{
    local cargo_target=$1

    cargo clippy \
        -p controller \
        --target "$cargo_target" \
        --features=auth-jwt,cli \
        -- -D warnings
}

controller_macos_do_test()
{
    cargo test -p controller
}

# Windows platform functions
controller_win32_do_build()
{
    local out_dir=$1
    local hide_console_feature=$2

    # Build controller package (produces controller.exe and jwt-gen.exe)
    cargo build \
        -p controller \
        --target x86_64-pc-windows-gnu \
        --features=auth-jwt,cli \
        --release \
        $hide_console_feature

    # Build service package (produces service.exe)
    cargo build \
        -p service \
        --target x86_64-pc-windows-gnu \
        --release

    cp \
        target/x86_64-pc-windows-gnu/release/controller.exe \
        target/x86_64-pc-windows-gnu/release/service.exe \
        target/x86_64-pc-windows-gnu/release/jwt-gen.exe \
        "$out_dir/bin"
}

controller_win32_do_clippy()
{
    # Clippy for controller package
    cargo clippy \
        -p controller \
        --target x86_64-pc-windows-gnu \
        --features=auth-jwt,cli \
        -- -D warnings

    # Clippy for service package
    cargo clippy \
        -p service \
        --target x86_64-pc-windows-gnu \
        -- -D warnings
}
