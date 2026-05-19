//! Provider abstraction and concrete impls.
//!
//! Each provider is a struct that knows its endpoint, auth, and request /
//! response shapes. The [`Provider`] trait normalises them behind a single
//! `translate(prompt) -> Result<String>` method so [`crate::LlmTranslator`]
//! can fall back across them transparently.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Error;

mod anthropic;
mod ollama;
mod openai;

/// Which LLM provider to talk to. Groq uses OpenAI's wire format with a
/// different base URL, so it reuses the OpenAI client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// OpenAI's `/v1/chat/completions` (default `https://api.openai.com/v1`).
    OpenAi,
    /// Anthropic's `/v1/messages` API.
    Anthropic,
    /// A local Ollama server (`http://localhost:11434/api` by default).
    Ollama,
    /// Groq's OpenAI-compatible API at `https://api.groq.com/openai/v1`.
    Groq,
}

/// Configuration for a single provider in the fallback chain.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Which provider this is.
    pub kind: ProviderKind,
    /// API key. Required for OpenAI, Anthropic, Groq; ignored for Ollama.
    pub api_key: Option<String>,
    /// Model name (e.g. `gpt-4o-mini`, `claude-3-5-sonnet-20241022`,
    /// `llama3.2`).
    pub model: String,
    /// Override the default base URL. Use to point OpenAI/Anthropic/Ollama
    /// at a proxy, or to talk to a different Ollama host.
    pub base_url: Option<String>,
}

impl ProviderConfig {
    pub(crate) fn into_provider(self) -> Result<Box<dyn Provider>, Error> {
        match self.kind {
            ProviderKind::OpenAi => Ok(Box::new(openai::OpenAiProvider::new(self, false)?)),
            ProviderKind::Groq => Ok(Box::new(openai::OpenAiProvider::new(self, true)?)),
            ProviderKind::Anthropic => Ok(Box::new(anthropic::AnthropicProvider::new(self)?)),
            ProviderKind::Ollama => Ok(Box::new(ollama::OllamaProvider::new(self)?)),
        }
    }
}

/// A model entry as returned by a provider's catalogue endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier the API accepts as `model`.
    pub id: String,
    /// Free-form capability summary (param count, context window, etc.).
    pub capabilities: String,
}

/// The abstraction every provider impl exposes.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider tag for logs and errors. Stable static string.
    fn name(&self) -> &'static str;

    /// Run a single prompt and return the model's text response.
    async fn translate(&self, prompt: &str, opts: &TranslateOptions) -> Result<String, Error>;

    /// List the models this provider exposes. Used by the CLI's `--list`
    /// helper. Not part of the hot translation path.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, Error>;
}

/// Tunables for a single translation call. Defaults match the Python
/// LLMTranslator: `temperature=0.3`, `max_tokens=4000`, 60 s timeout.
#[derive(Debug, Clone)]
pub struct TranslateOptions {
    /// Sampling temperature (0.0..1.0).
    pub temperature: f32,
    /// Maximum tokens the model may emit.
    pub max_tokens: u32,
}

impl Default for TranslateOptions {
    fn default() -> Self {
        Self {
            temperature: 0.3,
            max_tokens: 4000,
        }
    }
}
