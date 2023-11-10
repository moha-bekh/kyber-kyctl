use crate::{ConnectionParams, Result};

pub(crate) fn create_host(conn_params: &ConnectionParams) -> String {
    format!(
        "{host}:{port}",
        host = conn_params.host,
        port = conn_params.port
    )
}

pub(crate) fn create_reqwest(_conn_params: ConnectionParams) -> Result<reqwest::Client> {
    let client = reqwest::ClientBuilder::new().build()?;

    Ok(client)
}
