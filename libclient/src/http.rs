use std::net::SocketAddr;

use crate::tls;
use crate::Result;

pub(crate) struct HttpClient {
    base_url: String,
    client: reqwest::Client,
}

impl HttpClient {
    pub(crate) fn new(tls_host: &str, addr: &SocketAddr, tls_config: &tls::Config) -> Result<Self> {
        // Load TLS certificate
        let certificate = reqwest::Certificate::from_der(&tls_config.root_certificate)?;

        // Create client
        let client = reqwest::ClientBuilder::new()
            .resolve(tls_host, *addr)
            .add_root_certificate(certificate)
            .build()?;

        let base_url = format!("https://{tls_host}:{port}", port = addr.port());
        Ok(Self { base_url, client })
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
}
