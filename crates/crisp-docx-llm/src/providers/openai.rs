//! OpenAI Chat Completions wire format + every provider that ships the
//! same wire-shape under a different host.
//!
//! Most "AI gateway" services (Groq, OpenRouter, Together, Cerebras,
//! Mistral, Nebius, Scaleway, Poe) expose the OpenAI Chat Completions
//! API verbatim. They all accept the same request body
//! (`{model, messages, temperature, max_tokens}`), use
//! `Authorization: Bearer …` for auth, and return the same response
//! shape (`choices[0].message.content`). We reuse one [`OpenAiProvider`]
//! impl for all of them; only the static name + default base URL
//! differ.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{build_translation_prompt, ModelInfo, Provider, ProviderConfig, TranslateOptions};
use crate::Error;

pub(crate) struct OpenAiProvider {
    name: &'static str,
    api_key: String,
    model: String,
    base_url: String,
    http: reqwest::Client,
}

impl OpenAiProvider {
    /// Build an OpenAI-compatible provider. `name` is the static tag
    /// used in errors/logs; `default_base` is the host used when the
    /// caller didn't override `cfg.base_url`.
    pub(crate) fn new(
        cfg: ProviderConfig,
        name: &'static str,
        default_base: &'static str,
    ) -> Result<Self, Error> {
        let api_key = cfg
            .api_key
            .ok_or_else(|| Error::Config(format!("{name} requires api_key")))?;
        let base_url = cfg.base_url.unwrap_or_else(|| default_base.to_string());
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| Error::Http {
                provider: name,
                source: e,
            })?;
        Ok(Self {
            name,
            api_key,
            model: cfg.model,
            base_url,
            http,
        })
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    async fn translate(
        &self,
        text: &str,
        src_lang: &str,
        tgt_lang: &str,
        opts: &TranslateOptions,
    ) -> Result<String, Error> {
        let prompt = build_translation_prompt(text, src_lang, tgt_lang, opts.prompt_style);
        let url = format!("{}/chat/completions", self.base_url);
        let body = ChatRequest {
            model: &self.model,
            messages: vec![ChatMessage {
                role: "user",
                content: &prompt,
            }],
            temperature: opts.temperature,
            max_tokens: opts.max_tokens,
        };
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: self.name,
                source: e,
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| Error::Http {
            provider: self.name,
            source: e,
        })?;
        if !status.is_success() {
            return Err(Error::Api {
                provider: self.name,
                status: status.as_u16(),
                body: truncate(String::from_utf8_lossy(&bytes).into_owned(), 400),
            });
        }
        let chat: ChatResponse = serde_json::from_slice(&bytes)?;
        let content = chat
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| Error::BadResponse {
                provider: self.name,
                reason: "missing choices[0].message.content".into(),
            })?;
        Ok(content.trim().to_string())
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, Error> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: self.name,
                source: e,
            })?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| Error::Http {
            provider: self.name,
            source: e,
        })?;
        if !status.is_success() {
            return Err(Error::Api {
                provider: self.name,
                status: status.as_u16(),
                body: truncate(body, 400),
            });
        }
        let v: Value = serde_json::from_str(&body)?;
        let arr = v
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| Error::BadResponse {
                provider: self.name,
                reason: "expected `data` array".into(),
            })?;
        let mut out = Vec::with_capacity(arr.len());
        for m in arr {
            let id = m
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let cw = m.get("context_window").and_then(|v| v.as_u64());
            let caps = match cw {
                Some(ctx) => format!("ctx: {ctx}"),
                None => "Available".into(),
            };
            out.push(ModelInfo {
                id,
                capabilities: caps,
            });
        }
        Ok(out)
    }
}

fn truncate(s: String, n: usize) -> String {
    if s.len() <= n {
        s
    } else {
        let mut t = s;
        t.truncate(n);
        t.push('…');
        t
    }
}
