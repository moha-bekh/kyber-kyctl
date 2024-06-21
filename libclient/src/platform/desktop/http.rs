use reqwest::RequestBuilder;

use crate::{ConnectionParams, Result};

pub(crate) fn create_host(conn_params: &ConnectionParams) -> String {
    let host = conn_params.tls_host.as_ref().unwrap_or(&conn_params.host);
    format!("{host}:{port}", port = conn_params.port)
}

pub(crate) fn create_reqwest(conn_params: ConnectionParams) -> Result<reqwest::Client> {
    let client_builder = reqwest::ClientBuilder::new();

    let client_builder = if let Some(tls_host) = &conn_params.tls_host {
        client_builder.resolve(tls_host, conn_params.addr)
    } else {
        client_builder
    };

    let client_builder = if let Some(tls_config) = &conn_params.tls_config {
        let certificate = reqwest::Certificate::from_der(&tls_config.root_certificate)?;

        client_builder.add_root_certificate(certificate)
    } else {
        client_builder
    };

    Ok(client_builder.build()?)
}

pub(crate) fn configure_request(builder: RequestBuilder) -> RequestBuilder {
    builder
}
