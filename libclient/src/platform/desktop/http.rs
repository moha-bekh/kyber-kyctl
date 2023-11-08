use crate::{ConnectionParams, Result};

pub(crate) fn create_host(conn_params: &ConnectionParams) -> String {
    format!(
        "{tls_host}:{port}",
        tls_host = conn_params.tls_host,
        port = conn_params.port
    )
}

pub(crate) fn create_reqwest(conn_params: ConnectionParams) -> Result<reqwest::Client> {
    let certificate = reqwest::Certificate::from_der(&conn_params.tls_config.root_certificate)?;

    let client = reqwest::ClientBuilder::new()
        .resolve(&conn_params.tls_host, conn_params.addr)
        .add_root_certificate(certificate)
        .build()?;

    Ok(client)
}
