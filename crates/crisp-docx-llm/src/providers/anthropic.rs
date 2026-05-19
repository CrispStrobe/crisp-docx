//! Anthropic (`/v1/messages`).
//!
//! Wire format diverges from OpenAI: uses `x-api-key` header instead of
//! `Authorization: Bearer`, requires the `anthropic-version` header, and
//! the response field is `content[0].text` rather than `choices[0].message.content`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ModelInfo, Provider, ProviderConfig, TranslateOptions};
use crate::Error;

const DEFAULT_BASE: &str = "https://api.anthropic.com/v1";
const API_VERSION: &str = "2023-06-01";

pub(crate) struct AnthropicProvider {
    api_key: String,
    model: String,
    base_url: String,
    http: reqwest::Client,
}

impl AnthropicProvider {
    pub(crate) fn new(cfg: ProviderConfig) -> Result<Self, Error> {
        let api_key = cfg
            .api_key
            .ok_or_else(|| Error::Config("anthropic requires api_key".into()))?;
        let base_url = cfg.base_url.unwrap_or_else(|| DEFAULT_BASE.to_string());
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| Error::Http {
                provider: "anthropic",
                source: e,
            })?;
        Ok(Self {
            api_key,
            model: cfg.model,
            base_url,
            http,
        })
    }
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    messages: Vec<Message<'a>>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Serialize)]
struct Message<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: Option<String>,
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn translate(&self, prompt: &str, opts: &TranslateOptions) -> Result<String, Error> {
        let url = format!("{}/messages", self.base_url);
        let body = MessagesRequest {
            model: &self.model,
            messages: vec![Message {
                role: "user",
                content: prompt,
            }],
            temperature: opts.temperature,
            max_tokens: opts.max_tokens,
        };
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "anthropic",
                source: e,
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| Error::Http {
            provider: "anthropic",
            source: e,
        })?;
        if !status.is_success() {
            return Err(Error::Api {
                provider: "anthropic",
                status: status.as_u16(),
                body: truncate(String::from_utf8_lossy(&bytes).into_owned(), 400),
            });
        }
        let parsed: MessagesResponse = serde_json::from_slice(&bytes)?;
        let text = parsed
            .content
            .into_iter()
            .find(|b| b.block_type == "text")
            .and_then(|b| b.text)
            .ok_or_else(|| Error::BadResponse {
                provider: "anthropic",
                reason: "no text block in response.content".into(),
            })?;
        Ok(text.trim().to_string())
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, Error> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "anthropic",
                source: e,
            })?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| Error::Http {
            provider: "anthropic",
            source: e,
        })?;
        if !status.is_success() {
            return Err(Error::Api {
                provider: "anthropic",
                status: status.as_u16(),
                body: truncate(text, 400),
            });
        }
        let v: Value = serde_json::from_str(&text)?;
        let arr = v
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| Error::BadResponse {
                provider: "anthropic",
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
            let display = m
                .get("display_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let caps = if display.is_empty() {
                "Available".to_string()
            } else {
                format!("Display: {display}")
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
