use reqwest::IntoUrl;

use crate::platform::http as platform_http;
use crate::{ConnectionParams, Result};

pub(crate) struct HttpClient {
    base_url: String,
    client: reqwest::Client,
    host: String,
}

impl HttpClient {
    pub(crate) fn new(conn_params: ConnectionParams) -> Result<Self> {
        let host = platform_http::create_host(&conn_params);
        let base_url = format!("https://{host}");

        let client = platform_http::create_reqwest(conn_params)?;

        Ok(Self {
            base_url,
            client,
            host,
        })
    }

    pub(crate) fn get<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        platform_http::configure_request(self.client.get(url))
    }

    pub(crate) fn post<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        platform_http::configure_request(self.client.post(url))
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    #[allow(dead_code)]
    pub(crate) fn host(&self) -> &str {
        &self.host
    }
}
