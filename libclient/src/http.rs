use log::error;

use crate::tls;
use crate::{Error, Result};

pub(crate) struct HttpClient {
    base_url: String,
    client: reqwest::Client,
}

impl HttpClient {
    pub(crate) fn new(host: &str, ip: &str, port: u16, tls_config: &tls::Config) -> Result<Self> {
        // Prepare target address
        let addr = format!("{ip}:{port}");
        let addr = addr.parse().map_err(|err| {
            error!("Fail to parse address {addr}: {err:?}");
            Error::Rejected
        })?;

        // Load TLS certificate
        let certificate = reqwest::Certificate::from_der(&tls_config.certificate)?;

        // Create client
        let client = reqwest::ClientBuilder::new()
            .resolve(host, addr)
            .add_root_certificate(certificate)
            .build()?;

        let base_url = format!("https://{host}:{port}");
        Ok(Self { base_url, client })
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
}
