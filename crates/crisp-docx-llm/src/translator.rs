//! Provider-fallback orchestrator.
//!
//! [`LlmTranslator`] holds an ordered list of providers and tries each in
//! turn. The first successful response wins. On a fatal error from one
//! provider, the next is tried. If they all fail, the last error is
//! bubbled up via [`Error::AllProvidersFailed`].

use crate::providers::{Language, Provider, ProviderConfig, TranslateOptions};
use crate::Error;

/// Translation runner. Construct with [`LlmTranslator::new`] then add
/// providers via [`add_provider`](Self::add_provider). Cheap to clone if
/// you wrap in `Arc`.
pub struct LlmTranslator {
    providers: Vec<Box<dyn Provider>>,
    options: TranslateOptions,
}

impl LlmTranslator {
    /// Empty translator. Add providers before calling `translate_*`.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            options: TranslateOptions::default(),
        }
    }

    /// Append a provider to the fallback chain. Earlier entries are
    /// tried first.
    pub fn add_provider(mut self, cfg: ProviderConfig) -> Result<Self, Error> {
        let p = cfg.into_provider()?;
        self.providers.push(p);
        Ok(self)
    }

    /// Set sampling temperature / max tokens. Defaults to
    /// `temperature=0.3`, `max_tokens=4000` (parity with the Python
    /// LLMTranslator).
    pub fn with_options(mut self, options: TranslateOptions) -> Self {
        self.options = options;
        self
    }

    /// How many providers are configured.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Names of configured providers, in order.
    pub fn provider_names(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    /// Translate a single string. Tries each provider in order;
    /// short-circuits on first success.
    ///
    /// `use_alignment_hint` reproduces the Python translator's
    /// "preserve word order for alignment" prompt variant. When `false`,
    /// a natural-fluency prompt is used.
    pub async fn translate_text(
        &self,
        text: &str,
        src_lang: &str,
        tgt_lang: &str,
    ) -> Result<String, Error> {
        self.translate_text_with(text, src_lang, tgt_lang, true)
            .await
    }

    /// Lower-level variant that lets the caller pick the prompt style.
    pub async fn translate_text_with(
        &self,
        text: &str,
        src_lang: &str,
        tgt_lang: &str,
        use_alignment_hint: bool,
    ) -> Result<String, Error> {
        if self.providers.is_empty() {
            return Err(Error::NoProviders);
        }
        let mut opts = self.options.clone();
        opts.prompt_style = if use_alignment_hint {
            crate::providers::PromptStyle::PreserveOrder
        } else {
            crate::providers::PromptStyle::Fluent
        };
        // Free-form strings (`"English"` / `"de"`) → typed Language
        // (carries both `.name` and `.code`) so each Provider impl
        // picks the right representation.
        let src = Language::parse(src_lang);
        let tgt = Language::parse(tgt_lang);
        let mut last_err: Option<Error> = None;
        for p in &self.providers {
            match p.translate(text, &src, &tgt, &opts).await {
                Ok(out) => return Ok(out),
                Err(e) => {
                    tracing::debug!(provider = p.name(), error = %e, "translate failed");
                    last_err = Some(e);
                }
            }
        }
        Err(Error::AllProvidersFailed {
            last: last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unknown".into()),
        })
    }

    /// Translate many strings concurrently. Falls back per-string, not
    /// per-batch — i.e. one string might land on OpenAI while another
    /// falls through to Ollama. The order of the returned vec matches
    /// the input.
    pub async fn translate_batch(
        &self,
        texts: &[String],
        src_lang: &str,
        tgt_lang: &str,
    ) -> Vec<Result<String, Error>> {
        let futures = texts
            .iter()
            .map(|t| async move { self.translate_text(t, src_lang, tgt_lang).await });
        futures::future::join_all(futures).await
    }

    /// Borrow the configured providers (for diagnostics, e.g. listing
    /// models from each).
    pub fn providers(&self) -> &[Box<dyn Provider>] {
        &self.providers
    }
}

impl Default for LlmTranslator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{build_translation_prompt, PromptStyle};

    #[test]
    fn prompt_uses_alignment_clause_when_requested() {
        let en = Language::parse("English");
        let de = Language::parse("German");
        let p = build_translation_prompt("Hello.", &en, &de, PromptStyle::PreserveOrder);
        assert!(p.contains("Hello."));
        assert!(p.contains("from English to German"));
        assert!(p.contains("Preserve the word order"));
        assert!(!p.contains("natural, fluent"));
    }

    #[test]
    fn prompt_uses_fluency_clause_otherwise() {
        let en = Language::parse("English");
        let de = Language::parse("German");
        let p = build_translation_prompt("Hello.", &en, &de, PromptStyle::Fluent);
        assert!(p.contains("natural, fluent"));
        assert!(!p.contains("Preserve the word order"));
    }

    #[test]
    fn language_parses_iso_code() {
        let l = Language::parse("de");
        assert_eq!(l.code, "de");
        assert_eq!(l.name, "German");
    }

    #[test]
    fn language_parses_human_name() {
        let l = Language::parse("German");
        assert_eq!(l.name, "German");
        assert_eq!(l.code, "de");
    }

    #[test]
    fn language_passes_unknown_through() {
        let l = Language::parse("Klingon");
        assert_eq!(l.name, "Klingon");
        assert_eq!(l.code, "Klingon");
    }

    #[test]
    fn language_handles_native_names() {
        assert_eq!(Language::parse("Deutsch").code, "de");
        assert_eq!(Language::parse("français").code, "fr");
        assert_eq!(Language::parse("中文").code, "zh");
    }

    #[tokio::test]
    async fn empty_translator_errors_no_providers() {
        let t = LlmTranslator::new();
        let err = t
            .translate_text("Hello.", "English", "German")
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NoProviders));
    }
}
