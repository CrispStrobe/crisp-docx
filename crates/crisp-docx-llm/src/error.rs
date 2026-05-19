//! Crate-wide error type.

use thiserror::Error;

/// All errors surfaced by the LLM clients.
#[derive(Debug, Error)]
pub enum Error {
    /// HTTP request failed (network, TLS, timeout).
    #[error("HTTP error talking to {provider}: {source}")]
    Http {
        /// Provider name, for context.
        provider: &'static str,
        /// Underlying reqwest error.
        #[source]
        source: reqwest::Error,
    },

    /// Provider returned a non-2xx status. Body included for diagnosis.
    #[error("{provider} returned HTTP {status}: {body}")]
    Api {
        /// Provider name.
        provider: &'static str,
        /// HTTP status code.
        status: u16,
        /// Truncated response body.
        body: String,
    },

    /// Provider response wasn't shaped how we expected (missing field,
    /// unexpected JSON layout).
    #[error("{provider} returned a malformed response: {reason}")]
    BadResponse {
        /// Provider name.
        provider: &'static str,
        /// Human-readable description of what was wrong.
        reason: String,
    },

    /// Caller asked for translation but no providers were configured.
    #[error("no providers configured — call add_provider() first")]
    NoProviders,

    /// All configured providers failed in turn; the last error is bubbled
    /// up here for convenience.
    #[error("all providers failed; last error: {last}")]
    AllProvidersFailed {
        /// Stringified last error from the chain.
        last: String,
    },

    /// Configuration was missing a required field.
    #[error("invalid provider config: {0}")]
    Config(String),

    /// JSON serialization / parse failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Generic I/O (rare; some providers need filesystem-y things).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
