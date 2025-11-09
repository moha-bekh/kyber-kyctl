//! Session management for Kyber Controller.
//!
//! # Session Lifecycle
//!
//! ## Creation (POST /session/login)
//! 1. Credentials validated by auth backend
//! 2. UUID v4 generated as session identifier
//! 3. Session data stored in encrypted cookie: `{uid: UUID, exp: timestamp}`
//! 4. ClientSession created in Controller with auth tokens
//! 5. One-time WebSocket token returned to client
//!
//! ## Validation (Every HTTP Request)
//! 1. SessionMiddleware decrypts cookie
//! 2. IdentityMiddleware checks expiration
//! 3. Identity extractor validates session exists
//! 4. Handler executes if valid, 401 if invalid
//!
//! ## Renewal (POST /session/renew)
//! 1. Client provides existing session cookie + new credentials
//! 2. New credentials validated
//! 3. Username must match existing session
//! 4. Expiration updated in both cookie and Controller state
//!
//! ## Termination
//! - **Manual**: POST /session/logout → `Identity::logout()` → cookie purged
//! - **Expiration**: Background task checks every 30s → `Controller::stop()`
//!
//! # Expiration Enforcement
//!
//! Two mechanisms (both required):
//!
//! 1. **Request-Time (Middleware)**:
//!    - Runs on every HTTP request
//!    - Immediate validation and purge
//!    - Handles HTTP-based access
//!
//! 2. **Background Task (30s interval)**:
//!    - Runs independently of HTTP requests
//!    - Handles WebSocket-only connections
//!    - Without this, a WebSocket-only client could continue after JWT expiry
//!
//! # Configuration
//!
//! ```toml
//! [controller.session]
//! disable_same_site_protection = false  # Set true for cross-origin scenarios
//! ```

/// One-time authentication tokens generated per session.
///
/// These tokens are used for authenticating secondary connections (WebSocket, QUIC)
/// after the initial HTTP authentication. Each token is randomly generated and
/// can only be used once.
pub(crate) struct ClientAuthTokens {
    /// 128-character alphanumeric token for WebSocket authentication
    pub(crate) websocket: String,
}

impl ClientAuthTokens {
    /// Generates new random tokens for a session.
    ///
    /// Tokens are cryptographically random 128-character alphanumeric strings,
    /// providing sufficient entropy to prevent brute-force attacks.
    pub(crate) fn generate() -> Self {
        use rand::distr::SampleString;
        const TOKEN_STRING_LEN: usize = 128;

        let mut rng = rand::rng();
        Self {
            websocket: rand::distr::Alphanumeric.sample_string(&mut rng, TOKEN_STRING_LEN),
        }
    }
}

/// Session module configuration.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// If true, sets SameSite cookie attribute to None instead of Strict.
    ///
    /// **Security Warning**: Disabling SameSite protection makes Kyber vulnerable
    /// to CSRF attacks.
    pub disable_same_site_protection: bool,
}

pub mod expiration;
pub mod identity;
