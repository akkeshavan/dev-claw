use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};

use super::LlmClient;

pub struct Client {
    base_url: String,
    api_key:  String,
    model:    String,
    http:     HttpClient,
}

impl Client {
    pub fn new(base_url: &str, api_key: &str, model: &str) -> Self {
        let http = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            base_url: base_url.to_string(),
            api_key:  api_key.to_string(),
            model:    model.to_string(),
            http,
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model:       &'a str,
    messages:    Vec<Msg<'a>>,
    max_tokens:  u32,
    temperature: f32,
}

#[derive(Serialize)]
struct Msg<'a> {
    role:    &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: MsgOut,
}

#[derive(Deserialize)]
struct MsgOut {
    content: String,
}

fn build_request<'a>(model: &'a str, system: &'a str, user: &'a str) -> ChatRequest<'a> {
    ChatRequest {
        model,
        messages: vec![
            Msg { role: "system", content: system },
            Msg { role: "user",   content: user },
        ],
        max_tokens:  512,
        temperature: 0.1, // Low temperature for deterministic diagnostic output
    }
}

fn extract_text(resp: ChatResponse) -> Result<String> {
    resp.choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| anyhow::anyhow!("LLM returned no choices"))
}

#[async_trait]
impl LlmClient for Client {
    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let body = build_request(&self.model, system, user);

        let resp = self.http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("Failed to reach LLM API — check your network connection")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("LLM API returned {status}: {text}");
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .context("Failed to parse LLM response JSON")?;

        extract_text(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serializes_two_messages() {
        let req = build_request("deepseek-chat", "You are helpful.", "Hello");
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "deepseek-chat");
        assert_eq!(json["messages"].as_array().unwrap().len(), 2);
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][1]["role"], "user");
    }

    #[test]
    fn request_uses_low_temperature() {
        let req = build_request("gpt-4o-mini", "sys", "user");
        let json = serde_json::to_value(&req).unwrap();
        assert!(json["temperature"].as_f64().unwrap() <= 0.2);
    }

    #[test]
    fn response_extracts_content() {
        let raw = r#"{"choices":[{"message":{"content":"Root cause: missing dep\nFix: npm install"}}]}"#;
        let resp: ChatResponse = serde_json::from_str(raw).unwrap();
        assert!(extract_text(resp).unwrap().contains("Root cause"));
    }

    #[test]
    fn empty_choices_returns_error() {
        let resp = ChatResponse { choices: vec![] };
        assert!(extract_text(resp).is_err());
    }
}
