// Project Kyber: build_version.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

// Shared build-script logic for the version baked into the binary. Included via
// `include!("../build_version.rs")` from each crate's build.rs.

use std::process::Command;

use vergen::EmitBuilder;

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn emit_git_version() {
    // Fetch tags when none are reachable (shallow or detached submodule
    // checkout) so git-describe resolves the release tag. Best-effort: offline
    // builds fail the fetch and fall back to the short SHA below. vergen emits
    // rerun-if-changed on the git HEAD, so this only runs when the commit moves.
    // Bound a stalled HTTP fetch so it can't hang the build (aborts below
    // 1 KB/s for 15s); ignored for ssh remotes, which CI does not use.
    if git_output(&["describe", "--tags"]).is_none() {
        Command::new("git")
            .args(["fetch", "--tags", "--quiet"])
            .env("GIT_HTTP_LOW_SPEED_LIMIT", "1000")
            .env("GIT_HTTP_LOW_SPEED_TIME", "15")
            .status()
            .ok();
    }

    EmitBuilder::builder()
        .git_describe(false, true, None)
        .emit()
        .expect("vergen failed to emit git describe");

    // vergen leaves VERGEN_GIT_DESCRIBE empty when no tag is reachable (e.g. the
    // fetch above failed offline). Fall back to the short commit SHA so the
    // logged revision is never blank.
    if git_output(&["describe", "--tags"]).is_none()
        && let Some(sha) = git_output(&["rev-parse", "--short", "HEAD"])
    {
        println!("cargo:rustc-env=VERGEN_GIT_DESCRIBE={sha}");
    }
}
