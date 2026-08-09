use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};

use super::LlmClient;

pub struct Client {
    api_key: String,
    model: String,
    http: HttpClient,
}

impl Client {
    pub fn new(api_key: &str, model: &str) -> Self {
        let http = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            http,
        }
    }
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    system: &'a str,
    messages: Vec<Msg<'a>>,
    max_tokens: u32,
}

#[derive(Serialize)]
struct Msg<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct Response {
    content: Vec<Block>,
}

#[derive(Deserialize)]
struct Block {
    text: String,
}

fn build_request<'a>(model: &'a str, system: &'a str, user: &'a str) -> Request<'a> {
    Request {
        model,
        system,
        messages: vec![Msg {
            role: "user",
            content: user,
        }],
        max_tokens: 512,
    }
}

fn extract_text(resp: Response) -> Result<String> {
    resp.content
        .into_iter()
        .next()
        .map(|b| b.text)
        .ok_or_else(|| anyhow::anyhow!("Anthropic returned empty content"))
}

#[async_trait]
impl LlmClient for Client {
    async fn complete(&self, system: &str, user: &str) -> Result<String> {
        let body = build_request(&self.model, system, user);

        let resp = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("Failed to reach Anthropic API")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic API returned {status}: {text}");
        }

        let parsed: Response = resp
            .json()
            .await
            .context("Failed to parse Anthropic response")?;

        extract_text(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_has_single_user_message() {
        let req = build_request("claude-haiku-4-5-20251001", "You are helpful.", "Hello");
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-haiku-4-5-20251001");
        assert_eq!(json["system"], "You are helpful.");
        let msgs = json["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    #[test]
    fn request_max_tokens_set() {
        let req = build_request("claude-haiku-4-5-20251001", "sys", "user");
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["max_tokens"], 512);
    }

    #[test]
    fn response_extracts_first_block() {
        let raw = r#"{"content":[{"text":"Root cause: type mismatch\nFix: change i32 to u32"}]}"#;
        let resp: Response = serde_json::from_str(raw).unwrap();
        assert!(extract_text(resp).unwrap().contains("Root cause"));
    }

    #[test]
    fn empty_content_returns_error() {
        let resp = Response { content: vec![] };
        assert!(extract_text(resp).is_err());
    }
}
