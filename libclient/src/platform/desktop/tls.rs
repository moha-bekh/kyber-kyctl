use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use log::error;

use crate::{Error, Result};

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(crate) root_certificate: Vec<u8>,
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

pub(crate) fn load_tls_config(root_cert_path: &str) -> Result<Config> {
    let root_cert_path = get_resource_path(root_cert_path);

    let root_cert_file = File::open(&root_cert_path).map_err(|e| {
        error!("Failed to open file {root_cert_path:?}: {e:?}");
        Error::FileNotFound(root_cert_path.clone())
    })?;

    let mut root_cert_reader = BufReader::new(&root_cert_file);

    let root_certificate = match rustls_pemfile::read_one(&mut root_cert_reader)? {
        Some(rustls_pemfile::Item::X509Certificate(cert)) => cert,
        _ => {
            error!("{root_cert_path:?} doesn't contain a X509 Certificate");
            return Err(Error::TlsInvalidFile);
        }
    };

    Ok(Config {
        root_certificate: root_certificate.to_vec(),
    })
}
