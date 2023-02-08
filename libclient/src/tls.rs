use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use log::error;

use crate::{Error, Result};

pub(crate) struct Config {
    pub(crate) certificate: Vec<u8>,
}

fn get_resource_path(resource_name: &str) -> PathBuf {
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        path.push(resource_name);

        path
    } else {
        PathBuf::from(resource_name)
    }
}

pub(crate) fn load_tls_certificate(cert_path: &str) -> Result<Config> {
    let cert_path = get_resource_path(cert_path);

    let cert_file = File::open(&cert_path).map_err(|e| {
        error!("Failed to open file {cert_path:?}: {e:?}");
        Error::FileNotFound(cert_path.clone())
    })?;

    let mut cert_reader = BufReader::new(&cert_file);

    let certificate = match rustls_pemfile::read_one(&mut cert_reader)? {
        Some(rustls_pemfile::Item::X509Certificate(cert)) => cert,
        _ => {
            error!("{cert_path:?} doesn't contain a X509 Certificate");
            return Err(Error::TlsInvalidFile);
        }
    };

    Ok(Config { certificate })
}
