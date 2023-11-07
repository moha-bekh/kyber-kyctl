use crate::{ConnectionParams, Result};

pub(crate) struct HttpClient {
    base_url: String,
    client: reqwest::Client,
}

impl HttpClient {
    pub(crate) fn new(conn_params: ConnectionParams) -> Result<Self> {
        let host = format!(
            "{tls_host}:{port}",
            tls_host = conn_params.tls_host,
            port = conn_params.port
        );
        let base_url = format!("https://{host}");

        // Load TLS certificate
        let certificate = reqwest::Certificate::from_der(&conn_params.tls_config.root_certificate)?;

        // Create client
        let client = reqwest::ClientBuilder::new()
            .resolve(&conn_params.tls_host, conn_params.addr)
            .add_root_certificate(certificate)
            .build()?;

        Ok(Self { base_url, client })
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
}
