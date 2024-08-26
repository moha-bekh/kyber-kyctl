use std::fs::File;
use std::io::BufReader;

use log::{error, warn};

use crate::utils::get_resource_path;
use crate::{Error, Result};

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) cert_chain: Vec<Vec<u8>>,
    pub(crate) private_key: Vec<u8>,
}

fn open_reader(path: &str) -> Result<BufReader<File>> {
    let path = get_resource_path(path);

    let f = File::open(&path).map_err(|e| {
        error!("Failed to open file {path:?}: {e:?}");
        Error::FileNotFound { path }
    })?;

    Ok(BufReader::new(f))
}

pub(crate) fn load_config(cert_path: &str, key_path: &str) -> Result<Config> {
    use rustls_pemfile::Item;

    // Load certificate
    let mut cert_reader = open_reader(cert_path)?;

    let cert_chain = rustls_pemfile::read_all(&mut cert_reader)?;
    let cert_chain: Vec<_> = cert_chain
        .into_iter()
        .filter_map(|item| match item {
            Item::X509Certificate(cert) => Some(cert),
            _ => {
                warn!("{cert_path}: Ignore non X509 Certificate item");
                None
            }
        })
        .collect();

    // Load key
    let mut key_reader = open_reader(key_path)?;

    let private_key = match rustls_pemfile::read_one(&mut key_reader)? {
        Some(Item::RSAKey(key)) => key,
        Some(Item::PKCS8Key(key)) => key,
        _ => {
            error!("{key_path} doesn't contain a PKCS1/PKCS8 Key");
            return Err(Error::TlsInvalidFile);
        }
    };

    // Build configuration
    Ok(Config {
        cert_chain,
        private_key,
    })
}
