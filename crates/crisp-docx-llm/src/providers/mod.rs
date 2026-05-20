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
pub mod nmt;
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

/// A language with both a human-readable display name and an
/// ISO-639-1 code. LLM-backed providers use [`Self::name`] in their
/// natural-language prompt; NMT backends use [`Self::code`] verbatim.
///
/// Construct from either form:
///
/// ```
/// use crisp_docx_llm::Language;
/// assert_eq!(Language::from("English").code, "en");
/// assert_eq!(Language::from("de").name, "German");
/// assert_eq!(Language::from("Klingon").name, "Klingon"); // pass-through
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Language {
    /// Human-readable name (e.g. `"German"`). Used by LLM prompts.
    pub name: String,
    /// ISO-639-1 code (e.g. `"de"`). Used by NMT models.
    pub code: String,
}

impl Language {
    /// Build a [`Language`] from a free-form string. Recognises both
    /// known display names (`"English"`, `"German"`, `"français"`, …)
    /// and ISO codes (`"en"`, `"de"`, `"fr"`). Unknown inputs pass
    /// through verbatim — the same string is reused for both `name`
    /// and `code` so NMT backends still get a chance, even if the
    /// model rejects unknown codes.
    pub fn parse(s: &str) -> Self {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Self {
                name: String::new(),
                code: String::new(),
            };
        }
        // Short all-ASCII alphabetic? Treat as a code; look up name.
        let looks_like_code =
            trimmed.len() <= 3 && trimmed.chars().all(|c| c.is_ascii_alphabetic());
        if looks_like_code {
            let code = trimmed.to_ascii_lowercase();
            let name = name_for_code(&code).unwrap_or(trimmed);
            return Self {
                name: name.to_string(),
                code,
            };
        }
        // Longer / non-ASCII → treat as a name; look up code.
        match code_for_name(trimmed) {
            Some(code) => Self {
                name: trimmed.to_string(),
                code: code.to_string(),
            },
            None => Self {
                name: trimmed.to_string(),
                code: trimmed.to_string(),
            },
        }
    }
}

impl From<&str> for Language {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

impl From<String> for Language {
    fn from(s: String) -> Self {
        Self::parse(&s)
    }
}

/// Map an ISO code to its English display name (best-effort).
fn name_for_code(code: &str) -> Option<&'static str> {
    match code {
        "en" => Some("English"),
        "de" => Some("German"),
        "fr" => Some("French"),
        "es" => Some("Spanish"),
        "it" => Some("Italian"),
        "pt" => Some("Portuguese"),
        "nl" => Some("Dutch"),
        "pl" => Some("Polish"),
        "ru" => Some("Russian"),
        "uk" => Some("Ukrainian"),
        "cs" => Some("Czech"),
        "sv" => Some("Swedish"),
        "no" => Some("Norwegian"),
        "da" => Some("Danish"),
        "fi" => Some("Finnish"),
        "el" => Some("Greek"),
        "tr" => Some("Turkish"),
        "ar" => Some("Arabic"),
        "he" => Some("Hebrew"),
        "zh" => Some("Chinese"),
        "ja" => Some("Japanese"),
        "ko" => Some("Korean"),
        "hi" => Some("Hindi"),
        "vi" => Some("Vietnamese"),
        "th" => Some("Thai"),
        "id" => Some("Indonesian"),
        "ro" => Some("Romanian"),
        "hu" => Some("Hungarian"),
        "bg" => Some("Bulgarian"),
        "sr" => Some("Serbian"),
        "hr" => Some("Croatian"),
        "sl" => Some("Slovenian"),
        "sk" => Some("Slovak"),
        _ => None,
    }
}

/// Map a display name (case-insensitive) to its ISO code.
fn code_for_name(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "english" => "en",
        "german" | "deutsch" => "de",
        "french" | "français" | "francais" => "fr",
        "spanish" | "español" | "espanol" | "castellano" => "es",
        "italian" | "italiano" => "it",
        "portuguese" | "português" | "portugues" => "pt",
        "dutch" | "nederlands" => "nl",
        "polish" | "polski" => "pl",
        "russian" | "русский" | "russky" | "russkiy" => "ru",
        "ukrainian" | "українська" | "ukrayinska" => "uk",
        "czech" | "čeština" | "cestina" => "cs",
        "swedish" | "svenska" => "sv",
        "norwegian" | "norsk" => "no",
        "danish" | "dansk" => "da",
        "finnish" | "suomi" => "fi",
        "greek" | "ελληνικά" => "el",
        "turkish" | "türkçe" | "turkce" => "tr",
        "arabic" | "العربية" => "ar",
        "hebrew" | "עברית" => "he",
        "chinese" | "中文" | "mandarin" => "zh",
        "japanese" | "日本語" => "ja",
        "korean" | "한국어" => "ko",
        "hindi" | "हिन्दी" => "hi",
        "vietnamese" | "tiếng việt" | "tieng viet" => "vi",
        "thai" | "ไทย" => "th",
        "indonesian" | "bahasa indonesia" => "id",
        "romanian" | "română" | "romana" => "ro",
        "hungarian" | "magyar" => "hu",
        "bulgarian" | "български" => "bg",
        "serbian" | "српски" | "srpski" => "sr",
        "croatian" | "hrvatski" => "hr",
        "slovenian" | "slovenščina" | "slovenski" => "sl",
        "slovak" | "slovenský" | "slovensky" => "sk",
        _ => return None,
    })
}

/// The abstraction every provider impl exposes.
///
/// `translate(text, src, tgt, opts)` takes the raw source text plus a
/// language pair. Each provider decides how to phrase the task:
/// LLM-backed providers (OpenAI / Anthropic / Ollama / Groq /
/// OpenRouter / Together / Cerebras / Mistral / Nebius / Scaleway /
/// Poe / Google) build a chat-completion prompt internally; NMT
/// backends like CrispASR's m2m100 / wmt21 (under the `nmt` feature)
/// pass `(text, src.code, tgt.code)` straight to the model.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider tag for logs and errors. Stable static string.
    fn name(&self) -> &'static str;

    /// Translate `text` from `src` to `tgt`. The [`Language`] type
    /// carries both a display name (for LLM prompts) and an ISO code
    /// (for NMT backends) so each provider can pick the right
    /// representation without doing its own lookup.
    async fn translate(
        &self,
        text: &str,
        src: &Language,
        tgt: &Language,
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
    src: &Language,
    tgt: &Language,
    style: PromptStyle,
) -> String {
    let clause = match style {
        PromptStyle::PreserveOrder => {
            "Preserve the word order as much as possible for alignment purposes."
        }
        PromptStyle::Fluent => "Provide a natural, fluent translation.",
    };
    let src_name = &src.name;
    let tgt_name = &tgt.name;
    format!(
        "Translate the following text from {src_name} to {tgt_name}. {clause} \
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
