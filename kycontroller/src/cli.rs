// Project Kyber: cli.rs
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

use std::path::PathBuf;

use clap::Parser;
use log::LevelFilter;

#[derive(Debug, Parser)]
#[command(version = env!("VERGEN_GIT_DESCRIBE"), about = "Kyber controller daemon")]
pub(crate) struct Cli {
    /// Path to the configuration file. When omitted, the default
    /// executable-relative location is used.
    #[arg(short = 'c', long = "config", value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Log verbosity level. Overrides KYBER_LOG_LEVEL and the config file.
    #[arg(short = 'v', long = "log-level", value_name = "LEVEL")]
    pub log_level: Option<LevelFilter>,
}
