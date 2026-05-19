//! Ollama (`/api/generate`).
//!
//! Local Ollama server, default `http://localhost:11434/api`. No auth.
//! Wire format is `{model, prompt, stream: false}` → `{response: "..."}`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ModelInfo, Provider, ProviderConfig, TranslateOptions};
use crate::Error;

const DEFAULT_BASE: &str = "http://localhost:11434/api";

pub(crate) struct OllamaProvider {
    model: String,
    base_url: String,
    http: reqwest::Client,
}

impl OllamaProvider {
    pub(crate) fn new(cfg: ProviderConfig) -> Result<Self, Error> {
        let base_url = cfg.base_url.unwrap_or_else(|| DEFAULT_BASE.to_string());
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| Error::Http {
                provider: "ollama",
                source: e,
            })?;
        Ok(Self {
            model: cfg.model,
            base_url,
            http,
        })
    }
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

#[async_trait]
impl Provider for OllamaProvider {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn translate(&self, prompt: &str, _opts: &TranslateOptions) -> Result<String, Error> {
        let url = format!("{}/generate", self.base_url);
        let body = GenerateRequest {
            model: &self.model,
            prompt,
            stream: false,
        };
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Http {
                provider: "ollama",
                source: e,
            })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| Error::Http {
            provider: "ollama",
            source: e,
        })?;
        if !status.is_success() {
            return Err(Error::Api {
                provider: "ollama",
                status: status.as_u16(),
                body: truncate(String::from_utf8_lossy(&bytes).into_owned(), 400),
            });
        }
        let parsed: GenerateResponse = serde_json::from_slice(&bytes)?;
        Ok(parsed.response.trim().to_string())
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, Error> {
        let url = format!("{}/tags", self.base_url);
        let resp = self.http.get(&url).send().await.map_err(|e| Error::Http {
            provider: "ollama",
            source: e,
        })?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| Error::Http {
            provider: "ollama",
            source: e,
        })?;
        if !status.is_success() {
            return Err(Error::Api {
                provider: "ollama",
                status: status.as_u16(),
                body: truncate(text, 400),
            });
        }
        let v: Value = serde_json::from_str(&text)?;
        let arr = v
            .get("models")
            .and_then(|d| d.as_array())
            .ok_or_else(|| Error::BadResponse {
                provider: "ollama",
                reason: "expected `models` array".into(),
            })?;
        let mut out = Vec::with_capacity(arr.len());
        for m in arr {
            let id = m
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let psize = m
                .get("details")
                .and_then(|d| d.get("parameter_size"))
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            out.push(ModelInfo {
                id,
                capabilities: format!("{psize} params"),
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
