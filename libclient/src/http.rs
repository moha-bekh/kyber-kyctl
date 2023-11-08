use crate::{ConnectionParams, Result};

use crate::platform::http as platform_http;

pub(crate) struct HttpClient {
    base_url: String,
    client: reqwest::Client,
}

impl HttpClient {
    pub(crate) fn new(conn_params: ConnectionParams) -> Result<Self> {
        let host = platform_http::create_host(&conn_params);
        let base_url = format!("https://{host}");

        let client = platform_http::create_reqwest(conn_params)?;

        Ok(Self { base_url, client })
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }
}
