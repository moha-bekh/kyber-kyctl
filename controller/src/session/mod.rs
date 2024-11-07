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

pub mod identity;
