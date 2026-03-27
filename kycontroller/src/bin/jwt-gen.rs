// Project Kyber: jwt-gen.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
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

use std::{fs::File, io::Read, path::PathBuf};

use clap::{Args, Parser, Subcommand};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::Serialize;

const DEFAULT_DEV_KEY: &[u8] = b"kyber-authentication-development-key";

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a JWT token.
    Token(TokenArgs),
}

#[derive(Args)]
struct TokenArgs {
    /// Standard audience claim (`aud`).
    ///
    /// In the scope of kyber this is used to identify the streaming session.
    #[arg(visible_alias = "aud", long, default_value = "kyber")]
    audience: String,

    /// Standard subject claim (`sub`).
    ///
    /// Identifies the token owner.
    #[arg(visible_alias = "sub", long, default_value = "test user")]
    subject: String,

    /// Expiration time of the token (`exp`).
    ///
    /// Describe the token time to live.
    #[arg(
        visible_alias = "exp",
        long,
        default_value = "2h",
        value_name = "HUMAN TIME"
    )]
    expiration: humantime::Duration,

    /// Token encoding algorithm.
    #[arg(long, default_value = "HS256")]
    algorithm: Algorithm,

    /// Plain encoding key.
    #[command(flatten)]
    key: TokenKey,
}

#[derive(Args)]
#[group(multiple = false)]
struct TokenKey {
    /// Specify the key as a raw string argument.
    #[arg(long, value_name = "RAW KEY")]
    key_plain: Option<String>,

    /// Specify a file path containing the key/certificate.
    #[arg(long, value_name = "PATH")]
    key_path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct Claims {
    aud: String,
    iat: u64,
    exp: u64,
    sub: String,
}

fn extract_encoding_key(alg: Algorithm, key: &[u8]) -> EncodingKey {
    match alg {
        Algorithm::HS256 => EncodingKey::from_secret(key),
        Algorithm::RS256 => EncodingKey::from_rsa_pem(key).unwrap(),
        _ => panic!("Unsupported algorithm"),
    }
}

fn generate_token(args: TokenArgs) {
    let aud = args.audience;
    let iat = jsonwebtoken::get_current_timestamp();
    let exp = iat + args.expiration.as_secs();
    let sub = args.subject;
    let claims = Claims { aud, iat, exp, sub };

    let key = match (args.key.key_plain, args.key.key_path) {
        (None, None) => extract_encoding_key(args.algorithm, DEFAULT_DEV_KEY),
        (Some(plain), None) => extract_encoding_key(args.algorithm, plain.as_bytes()),
        (None, Some(path)) => {
            let mut key = Vec::new();
            let mut file = File::open(path).expect("Invalid key path");
            file.read_to_end(&mut key)
                .expect("Failed to read key content");
            extract_encoding_key(args.algorithm, &key)
        }
        _ => unreachable!(),
    };

    let token = encode(&Header::new(args.algorithm), &claims, &key).unwrap();

    println!("{token}");
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Token(args) => generate_token(args),
    }
}
