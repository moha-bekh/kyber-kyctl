pub(crate) struct ClientAuthTokens {
    pub(crate) websocket: String,
}

impl ClientAuthTokens {
    pub(crate) fn generate() -> Self {
        use rand::distr::SampleString;
        const TOKEN_STRING_LEN: usize = 128;

        let mut rng = rand::rng();
        Self {
            websocket: rand::distr::Alphanumeric.sample_string(&mut rng, TOKEN_STRING_LEN),
        }
    }
}

#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub disable_same_site_protection: bool,
}

pub mod expiration;
pub mod identity;
