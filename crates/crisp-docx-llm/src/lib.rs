//! Async LLM HTTP clients for the crisp-docx translator pipeline.
//!
//! This is the Rust port of CrispTranslator's `translator.py::LLMTranslator`:
//! a thin client that knows how to talk to OpenAI, Anthropic, Ollama, and
//! Groq, with a uniform `translate_text(src, src_lang, tgt_lang)` surface
//! and a fallback chain (try first provider, on error try the next).
//!
//! Out of scope: NMT models (NLLB / OpusMT / Madlad / CT2). Those require
//! the PyTorch / HuggingFace ecosystem; see PARITY.md for the rationale.
//!
//! # Quick start
//!
//! ```no_run
//! use crisp_docx_llm::{LlmTranslator, ProviderConfig, ProviderKind};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let translator = LlmTranslator::new()
//!     .add_provider(ProviderConfig {
//!         kind: ProviderKind::OpenAi,
//!         api_key: Some(std::env::var("OPENAI_API_KEY")?),
//!         model: "gpt-4o-mini".into(),
//!         base_url: None,
//!     })?;
//! let de = translator
//!     .translate_text("The dog is sleeping.", "English", "German")
//!     .await?;
//! println!("{de}");
//! # Ok(()) }
//! ```

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod providers;
mod translator;

pub use error::Error;
pub use providers::{
    Language, ModelInfo, PromptStyle, Provider, ProviderConfig, ProviderKind, TranslateOptions,
};
pub use translator::LlmTranslator;
