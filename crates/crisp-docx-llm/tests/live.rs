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

/// Generic live test for OpenAI-compatible providers: takes a provider
/// kind + env-var prefix and runs the same translate-to-German assertion.
macro_rules! live_openai_compatible {
    ($name:ident, $kind:ident, $env_flag:literal, $key_env:literal, $model_env:literal, $default_model:literal) => {
        #[tokio::test]
        async fn $name() {
            if !enabled($env_flag) {
                eprintln!("{} not set; skipping", $env_flag);
                return;
            }
            let Ok(key) = std::env::var($key_env) else {
                eprintln!("{} missing; skipping", $key_env);
                return;
            };
            let t = LlmTranslator::new()
                .add_provider(ProviderConfig {
                    kind: ProviderKind::$kind,
                    api_key: Some(key),
                    model: std::env::var($model_env).unwrap_or_else(|_| $default_model.into()),
                    base_url: None,
                })
                .unwrap();
            let out = t
                .translate_text("The dog is sleeping.", "English", "German")
                .await
                .unwrap_or_else(|e| panic!("{}: {e}", stringify!($name)));
            eprintln!("{} → {:?}", stringify!($kind), out);
            assert!(looks_like_german(&out), "unexpected: {:?}", out);
        }
    };
}

live_openai_compatible!(
    live_openrouter_translates_to_german,
    OpenRouter,
    "CRISP_DOCX_LLM_LIVE_OPENROUTER",
    "OPENROUTER_API_KEY",
    "OPENROUTER_MODEL",
    "meta-llama/llama-3.3-70b-instruct"
);

live_openai_compatible!(
    live_together_translates_to_german,
    Together,
    "CRISP_DOCX_LLM_LIVE_TOGETHER",
    "TOGETHER_API_KEY",
    "TOGETHER_MODEL",
    "meta-llama/Llama-3.3-70B-Instruct-Turbo"
);

live_openai_compatible!(
    live_cerebras_translates_to_german,
    Cerebras,
    "CRISP_DOCX_LLM_LIVE_CEREBRAS",
    "CEREBRAS_API_KEY",
    "CEREBRAS_MODEL",
    "llama-3.3-70b"
);

live_openai_compatible!(
    live_nebius_translates_to_german,
    Nebius,
    "CRISP_DOCX_LLM_LIVE_NEBIUS",
    "NEBIUS_API_KEY",
    "NEBIUS_MODEL",
    "meta-llama/Llama-3.3-70B-Instruct"
);

live_openai_compatible!(
    live_scaleway_translates_to_german,
    Scaleway,
    "CRISP_DOCX_LLM_LIVE_SCALEWAY",
    "SCALEWAY_API_KEY",
    "SCALEWAY_MODEL",
    "llama-3.3-70b-instruct"
);

live_openai_compatible!(
    live_mistral_translates_to_german,
    Mistral,
    "CRISP_DOCX_LLM_LIVE_MISTRAL",
    "MISTRAL_API_KEY",
    "MISTRAL_MODEL",
    "mistral-large-latest"
);

live_openai_compatible!(
    live_poe_translates_to_german,
    Poe,
    "CRISP_DOCX_LLM_LIVE_POE",
    "POE_API_KEY",
    "POE_MODEL",
    "GPT-4o-mini"
);

live_openai_compatible!(
    live_google_translates_to_german,
    Google,
    "CRISP_DOCX_LLM_LIVE_GOOGLE",
    "GOOGLEAI_API_KEY",
    "GOOGLE_MODEL",
    "gemini-2.0-flash"
);
