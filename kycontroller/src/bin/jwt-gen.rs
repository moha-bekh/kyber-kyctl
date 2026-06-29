// Project Kyber: jwt-gen.rs
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

use std::{fs, io::Read, path::PathBuf};

use clap::{Args, Parser, Subcommand};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
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

    /// Generate signing keys for JWT authentication.
    Keys(KeysArgs),
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

#[derive(Args)]
struct KeysArgs {
    /// Key algorithm to generate [possible values: HS256, RS256].
    #[arg(long, default_value = "HS256")]
    algorithm: Algorithm,

    /// Output directory for key files (RS256 only).
    ///
    /// When provided, writes `jwt-private.pem` and `jwt-public.pem` to the directory.
    /// When omitted, prints keys to stdout.
    #[arg(long, value_name = "PATH")]
    output_dir: Option<PathBuf>,

    /// RSA key size in bits (RS256 only).
    #[arg(long, default_value = "2048")]
    bits: u32,
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
            let mut file = fs::File::open(path).expect("Invalid key path");
            file.read_to_end(&mut key)
                .expect("Failed to read key content");
            extract_encoding_key(args.algorithm, &key)
        }
        _ => unreachable!(),
    };

    let token = encode(&Header::new(args.algorithm), &claims, &key).unwrap();

    println!("{token}");
}

fn generate_keys(args: KeysArgs) {
    match args.algorithm {
        Algorithm::HS256 => generate_hs256_key(),
        Algorithm::RS256 => generate_rs256_keys(args.output_dir, args.bits),
        _ => panic!("Unsupported algorithm (supported: HS256, RS256)"),
    }
}

fn generate_hs256_key() {
    let key: [u8; 32] = rand::random();
    println!("{}", hex::encode(key));
}

fn generate_rs256_keys(output_dir: Option<PathBuf>, bits: u32) {
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};

    let private_key = rsa::RsaPrivateKey::new(&mut rand_core_06::OsRng, bits as usize)
        .expect("Failed to generate RSA key pair");

    let private_pem = private_key
        .to_pkcs8_pem(LineEnding::LF)
        .expect("Failed to encode private key");
    let public_pem = private_key
        .to_public_key()
        .to_public_key_pem(LineEnding::LF)
        .expect("Failed to encode public key");

    match output_dir {
        Some(dir) => {
            fs::create_dir_all(&dir).expect("Failed to create output directory");

            let private_path = dir.join("jwt-private.pem");
            let public_path = dir.join("jwt-public.pem");

            fs::write(&private_path, private_pem.as_bytes()).expect("Failed to write private key");
            fs::write(&public_path, public_pem.as_bytes()).expect("Failed to write public key");

            eprintln!("Wrote private key: {}", private_path.display());
            eprintln!("Wrote public key:  {}", public_path.display());
        }
        None => {
            println!("# Private Key (use with jwt-gen token --key-path)");
            print!("{}", &*private_pem);
            println!("# Public Key (configure in kycontroller)");
            print!("{public_pem}");
        }
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Token(args) => generate_token(args),
        Commands::Keys(args) => generate_keys(args),
    }
}
