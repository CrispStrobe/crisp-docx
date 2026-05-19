//! Live integration tests against real LLM providers. Gated on env
//! vars so they never fail on CI / no-network machines:
//!
//!   - `CRISP_DOCX_LLM_LIVE_OPENAI=1` + `OPENAI_API_KEY=…`
//!   - `CRISP_DOCX_LLM_LIVE_ANTHROPIC=1` + `ANTHROPIC_API_KEY=…`
//!   - `CRISP_DOCX_LLM_LIVE_GROQ=1` + `GROQ_API_KEY=…`
//!   - `CRISP_DOCX_LLM_LIVE_OLLAMA=1` (assumes a local Ollama at
//!     http://localhost:11434 has at least one model pulled)
//!
//! Run a single backend with e.g.:
//!
//!   CRISP_DOCX_LLM_LIVE_OPENAI=1 OPENAI_API_KEY=sk-… \
//!     cargo test -p crisp-docx-llm --test live live_openai
//!
//! Each test asks the provider to translate "The dog is sleeping." from
//! English to German and asserts the output contains at least one
//! plausible German cognate (Hund, schläft, Der, schläft). LLMs are
//! non-deterministic so we don't pin exact strings.

use crisp_docx_llm::{LlmTranslator, ProviderConfig, ProviderKind};

fn enabled(var: &str) -> bool {
    std::env::var(var).ok().as_deref() == Some("1")
}

fn looks_like_german(out: &str) -> bool {
    let lower = out.to_lowercase();
    let cognates = ["hund", "schläft", "schlaft", "der hund", "schlafen"];
    cognates.iter().any(|c| lower.contains(c))
}

#[tokio::test]
async fn live_openai_translates_to_german() {
    if !enabled("CRISP_DOCX_LLM_LIVE_OPENAI") {
        eprintln!("CRISP_DOCX_LLM_LIVE_OPENAI not set; skipping");
        return;
    }
    let Ok(key) = std::env::var("OPENAI_API_KEY") else {
        eprintln!("OPENAI_API_KEY missing; skipping");
        return;
    };
    let t = LlmTranslator::new()
        .add_provider(ProviderConfig {
            kind: ProviderKind::OpenAi,
            api_key: Some(key),
            model: std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into()),
            base_url: None,
        })
        .unwrap();
    let out = t
        .translate_text("The dog is sleeping.", "English", "German")
        .await
        .expect("openai translate");
    eprintln!("OpenAI → {:?}", out);
    assert!(looks_like_german(&out), "unexpected: {:?}", out);
}

#[tokio::test]
async fn live_anthropic_translates_to_german() {
    if !enabled("CRISP_DOCX_LLM_LIVE_ANTHROPIC") {
        eprintln!("CRISP_DOCX_LLM_LIVE_ANTHROPIC not set; skipping");
        return;
    }
    let Ok(key) = std::env::var("ANTHROPIC_API_KEY") else {
        eprintln!("ANTHROPIC_API_KEY missing; skipping");
        return;
    };
    let t = LlmTranslator::new()
        .add_provider(ProviderConfig {
            kind: ProviderKind::Anthropic,
            api_key: Some(key),
            model: std::env::var("ANTHROPIC_MODEL")
                .unwrap_or_else(|_| "claude-3-5-sonnet-20241022".into()),
            base_url: None,
        })
        .unwrap();
    let out = t
        .translate_text("The dog is sleeping.", "English", "German")
        .await
        .expect("anthropic translate");
    eprintln!("Anthropic → {:?}", out);
    assert!(looks_like_german(&out), "unexpected: {:?}", out);
}

#[tokio::test]
async fn live_groq_translates_to_german() {
    if !enabled("CRISP_DOCX_LLM_LIVE_GROQ") {
        eprintln!("CRISP_DOCX_LLM_LIVE_GROQ not set; skipping");
        return;
    }
    let Ok(key) = std::env::var("GROQ_API_KEY") else {
        eprintln!("GROQ_API_KEY missing; skipping");
        return;
    };
    let t = LlmTranslator::new()
        .add_provider(ProviderConfig {
            kind: ProviderKind::Groq,
            api_key: Some(key),
            model: std::env::var("GROQ_MODEL").unwrap_or_else(|_| "llama-3.3-70b-versatile".into()),
            base_url: None,
        })
        .unwrap();
    let out = t
        .translate_text("The dog is sleeping.", "English", "German")
        .await
        .expect("groq translate");
    eprintln!("Groq → {:?}", out);
    assert!(looks_like_german(&out), "unexpected: {:?}", out);
}

#[tokio::test]
async fn live_ollama_translates_to_german() {
    if !enabled("CRISP_DOCX_LLM_LIVE_OLLAMA") {
        eprintln!("CRISP_DOCX_LLM_LIVE_OLLAMA not set; skipping");
        return;
    }
    let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3.2".into());
    let t = LlmTranslator::new()
        .add_provider(ProviderConfig {
            kind: ProviderKind::Ollama,
            api_key: None,
            model,
            base_url: std::env::var("OLLAMA_BASE_URL").ok(),
        })
        .unwrap();
    let out = t
        .translate_text("The dog is sleeping.", "English", "German")
        .await
        .expect("ollama translate");
    eprintln!("Ollama → {:?}", out);
    assert!(looks_like_german(&out), "unexpected: {:?}", out);
}
