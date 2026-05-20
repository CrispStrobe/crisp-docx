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
#[cfg(feature = "nmt")]
mod nmt;
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
    /// Offline NMT backed by a CrispASR GGUF model (m2m100 / wmt21 /
    /// madlad / gemma4-e2b). The `model` field of [`ProviderConfig`]
    /// is the GGUF file path; `api_key` and `base_url` are ignored.
    /// Requires the `nmt` Cargo feature.
    Nmt,
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
            #[cfg(feature = "nmt")]
            ProviderKind::Nmt => Ok(Box::new(nmt::NmtProvider::new(self)?)),
            #[cfg(not(feature = "nmt"))]
            ProviderKind::Nmt => {
                let _ = self;
                Err(Error::Config(
                    "nmt provider requires the `nmt` Cargo feature to be enabled \
                     (rebuild crisp-docx-llm with `--features nmt`)"
                        .into(),
                ))
            }
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
///
/// `translate(text, src_lang, tgt_lang, opts)` takes the raw source
/// text plus a language pair. Each provider decides how to phrase the
/// task: LLM-backed providers (OpenAI / Anthropic / Ollama / Groq /
/// OpenRouter / Together / Cerebras / Mistral / Nebius / Scaleway /
/// Poe / Google) build a chat-completion prompt internally; NMT
/// backends like CrispASR's m2m100 / wmt21 (under the `nmt` feature)
/// pass `(text, src_lang, tgt_lang)` straight to the model.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider tag for logs and errors. Stable static string.
    fn name(&self) -> &'static str;

    /// Translate `text` from `src_lang` to `tgt_lang`. The language
    /// strings are free-form on the caller side (e.g. `"English"`,
    /// `"German"`); NMT backends with a fixed code vocabulary do their
    /// own name→code lookup internally.
    async fn translate(
        &self,
        text: &str,
        src_lang: &str,
        tgt_lang: &str,
        opts: &TranslateOptions,
    ) -> Result<String, Error>;

    /// List the models this provider exposes. Used by the CLI's `--list`
    /// helper. Not part of the hot translation path.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, Error>;
}

/// Build the canonical "translate from X to Y" prompt that all HTTP-
/// based LLM providers send. Pulled out so every OpenAI-compatible /
/// Anthropic / Ollama impl phrases the task identically — and so
/// callers can override it via [`TranslateOptions::prompt_style`].
pub fn build_translation_prompt(
    text: &str,
    src_lang: &str,
    tgt_lang: &str,
    style: PromptStyle,
) -> String {
    let clause = match style {
        PromptStyle::PreserveOrder => {
            "Preserve the word order as much as possible for alignment purposes."
        }
        PromptStyle::Fluent => "Provide a natural, fluent translation.",
    };
    format!(
        "Translate the following text from {src_lang} to {tgt_lang}. {clause} \
         Return ONLY the translation:\n\n{text}"
    )
}

/// Which prompt phrasing to use. Defaults to `PreserveOrder` to match
/// the Python `LLMTranslator(use_alignment=True)` baseline.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptStyle {
    /// Ask the model to preserve word order (good for downstream
    /// alignment-based format reattachment).
    #[default]
    PreserveOrder,
    /// Ask the model for a natural / fluent translation (no word-order
    /// constraint).
    Fluent,
}

/// Tunables for a single translation call. Defaults match the Python
/// LLMTranslator: `temperature=0.3`, `max_tokens=4000`, 60 s timeout.
#[derive(Debug, Clone)]
pub struct TranslateOptions {
    /// Sampling temperature (0.0..1.0). Ignored by NMT backends.
    pub temperature: f32,
    /// Maximum tokens the model may emit.
    pub max_tokens: u32,
    /// Prompt style for LLM-backed providers. NMT backends ignore this.
    pub prompt_style: PromptStyle,
}

impl Default for TranslateOptions {
    fn default() -> Self {
        Self {
            prompt_style: PromptStyle::default(),
            temperature: 0.3,
            max_tokens: 4000,
        }
    }
}
