use std::fs::File;
use std::io::BufReader;

use log::error;

use crate::utils::get_resource_path;
use crate::{Error, Result};

pub(crate) struct Config {
    pub(crate) certificate: Vec<u8>,
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

    let certificate = match rustls_pemfile::read_one(&mut cert_reader)? {
        Some(Item::X509Certificate(cert)) => cert,
        _ => {
            error!("{cert_path} doesn't contain a X509 Certificate");
            return Err(Error::TlsInvalidFile);
        }
    };

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
        certificate,
        private_key,
    })
}
