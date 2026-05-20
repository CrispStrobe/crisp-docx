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

/// Which LLM provider to talk to. Many providers ship the OpenAI Chat
/// Completions API verbatim under a different host (Groq, OpenRouter,
/// Together, Cerebras, Mistral, Nebius, Scaleway, Poe) — they all reuse
/// the same OpenAI client impl, just with a different name + default
/// base URL.
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
    /// OpenRouter's OpenAI-compatible aggregator at
    /// `https://openrouter.ai/api/v1`.
    OpenRouter,
    /// Together.ai's OpenAI-compatible endpoint at
    /// `https://api.together.xyz/v1`.
    Together,
    /// Cerebras' OpenAI-compatible inference API at
    /// `https://api.cerebras.ai/v1`.
    Cerebras,
    /// Mistral's OpenAI-compatible API at `https://api.mistral.ai/v1`.
    Mistral,
    /// Nebius AI Studio's OpenAI-compatible API at
    /// `https://api.studio.nebius.ai/v1`.
    Nebius,
    /// Scaleway's OpenAI-compatible inference API at
    /// `https://api.scaleway.ai/v1`.
    Scaleway,
    /// Poe's OpenAI-compatible API at `https://api.poe.com/v1`.
    Poe,
    /// Google Gemini's OpenAI-compatible endpoint at
    /// `https://generativelanguage.googleapis.com/v1beta/openai`.
    Google,
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
            ProviderKind::Anthropic => Ok(Box::new(anthropic::AnthropicProvider::new(self)?)),
            ProviderKind::Ollama => Ok(Box::new(ollama::OllamaProvider::new(self)?)),
            // OpenAI-compatible providers — same wire format, different
            // (name, default_base_url) tuples.
            ProviderKind::OpenAi => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "openai",
                "https://api.openai.com/v1",
            )?)),
            ProviderKind::Groq => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "groq",
                "https://api.groq.com/openai/v1",
            )?)),
            ProviderKind::OpenRouter => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "openrouter",
                "https://openrouter.ai/api/v1",
            )?)),
            ProviderKind::Together => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "together",
                "https://api.together.xyz/v1",
            )?)),
            ProviderKind::Cerebras => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "cerebras",
                "https://api.cerebras.ai/v1",
            )?)),
            ProviderKind::Mistral => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "mistral",
                "https://api.mistral.ai/v1",
            )?)),
            ProviderKind::Nebius => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "nebius",
                "https://api.studio.nebius.ai/v1",
            )?)),
            ProviderKind::Scaleway => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "scaleway",
                "https://api.scaleway.ai/v1",
            )?)),
            ProviderKind::Poe => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "poe",
                "https://api.poe.com/v1",
            )?)),
            ProviderKind::Google => Ok(Box::new(openai::OpenAiProvider::new(
                self,
                "google",
                "https://generativelanguage.googleapis.com/v1beta/openai",
            )?)),
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
