//! Offline NMT provider backed by CrispASR's text-to-text translate
//! pipeline.
//!
//! CrispASR ships several MT-capable GGUF backends —
//!
//! - **m2m100** — 100 languages, any-to-any.
//! - **m2m100-wmt21** — English-paired, direction-specific
//!   checkpoints (en→x or x→en), higher quality on the supported pairs.
//! - **madlad** — 419 languages via target-language prefix tag.
//! - **gemma4-e2b** — dual ASR + MT, 140+ langs.
//!
//! From the caller's side it's one model file (`*.gguf`) handed to
//! [`crispasr::Session::open`]; the backend is auto-detected from
//! GGUF metadata. We expose translation as a `Provider` so it slots
//! into the same fallback chain as the HTTP LLM providers — you can
//! configure CrispASR as the first choice and OpenAI as a fallback,
//! for example.
//!
//! Language strings: NMT models expect short ISO codes (`en`, `de`,
//! `fr`, …) while LLM prompts use human-readable names (`"English"`,
//! `"German"`). [`Self::translate`] does a name→code lookup via
//! [`map_lang_to_code`] for the major European pairs; ISO codes pass
//! through unchanged. Callers can avoid the lookup entirely by passing
//! the code directly.

use async_trait::async_trait;

use super::{Language, ModelInfo, Provider, ProviderConfig, TranslateOptions};
use crate::Error;

pub(crate) struct NmtProvider {
    model_path: String,
    session: std::sync::Mutex<crispasr::Session>,
}

impl NmtProvider {
    pub(crate) fn new(cfg: ProviderConfig) -> Result<Self, Error> {
        // `cfg.model` carries the GGUF path. `cfg.base_url`,
        // `cfg.api_key` are unused — NMT runs entirely in-process.
        let path = cfg.model;
        if path.is_empty() {
            return Err(Error::Config(
                "nmt provider requires `model` to be the GGUF file path".into(),
            ));
        }
        let session = crispasr::Session::open(&path).map_err(|e| Error::Config(format!(
            "failed to open CrispASR session for {path}: {e}"
        )))?;
        Ok(Self {
            model_path: path,
            session: std::sync::Mutex::new(session),
        })
    }
}

/// Best-effort mapping from a human-readable language name (`"German"`)
/// to the ISO-639-1 code (`"de"`) that NMT backends expect. Unknown
/// inputs fall through to the input verbatim — callers that already
/// pass a code get a no-op.
pub fn map_lang_to_code(lang: &str) -> String {
    let trimmed = lang.trim();
    if trimmed.len() <= 3 && trimmed.chars().all(|c| c.is_ascii_alphabetic()) {
        return trimmed.to_ascii_lowercase();
    }
    let lower = trimmed.to_ascii_lowercase();
    match lower.as_str() {
        // Common language names in English + native form. Not
        // exhaustive — m2m100 supports 100 langs — but covers the
        // pairs we exercise most.
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
        _ => return trimmed.to_string(),
    }
    .to_string()
}

#[async_trait]
impl Provider for NmtProvider {
    fn name(&self) -> &'static str {
        "nmt"
    }

    async fn translate(
        &self,
        text: &str,
        src: &Language,
        tgt: &Language,
        opts: &TranslateOptions,
    ) -> Result<String, Error> {
        let src_code = &src.code;
        let tgt_code = &tgt.code;
        // GGML inference is synchronous + CPU-bound. Block the current
        // async task — NMT calls on m2m100 are sub-second per paragraph;
        // batching is handled at the caller (LlmTranslator::translate_batch)
        // via `buffer_unordered`. Spawning each call onto its own
        // blocking thread would add tokio task overhead without any
        // throughput win since the model has a single inference state
        // behind the Mutex.
        let guard = self
            .session
            .lock()
            .map_err(|e| Error::Config(format!("nmt session mutex poisoned: {e}")))?;
        let out = guard
            .translate_text(text, src_code, tgt_code, opts.max_tokens as i32)
            .map_err(|e| Error::BadResponse {
                provider: "nmt",
                reason: format!("translate_text failed: {e}"),
            })?;
        Ok(out)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, Error> {
        // The "model" for NMT is the loaded GGUF file — return one
        // synthetic entry so the UI can display something coherent.
        Ok(vec![ModelInfo {
            id: self.model_path.clone(),
            capabilities: format!(
                "offline NMT (backend: {})",
                self.session
                    .lock()
                    .map(|s| s.backend())
                    .unwrap_or_else(|_| "unknown".to_string())
            ),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_names_resolve_to_iso_codes() {
        assert_eq!(map_lang_to_code("English"), "en");
        assert_eq!(map_lang_to_code("German"), "de");
        assert_eq!(map_lang_to_code("Deutsch"), "de");
        assert_eq!(map_lang_to_code("français"), "fr");
        assert_eq!(map_lang_to_code("中文"), "zh");
    }

    #[test]
    fn iso_codes_pass_through() {
        assert_eq!(map_lang_to_code("en"), "en");
        assert_eq!(map_lang_to_code("de"), "de");
        // Uppercase code → lowercase
        assert_eq!(map_lang_to_code("EN"), "en");
    }

    #[test]
    fn unknown_names_fall_through_verbatim() {
        // BCP-47 region codes m2m100 doesn't have a name for — pass
        // through so the caller can attempt it anyway.
        assert_eq!(map_lang_to_code("eo"), "eo"); // Esperanto code
        assert_eq!(map_lang_to_code("Klingon"), "Klingon");
    }
}
