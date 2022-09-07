use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct GenerateSdpRequest {
    pub yuv444: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GenerateSdpResponse {
    pub params: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartRequest {
    pub ip: String,
    pub port: u16,
    pub yuv444: bool,
    pub inputs: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartResponse {
    pub ip: String,
}
