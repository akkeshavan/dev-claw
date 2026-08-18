use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client as HttpClient;
use serde::{Deserialize, Serialize};

use super::LlmClient;

pub struct Client {
    base_url: String,
    api_key: String,
    model: String,
    http: HttpClient,
}

impl Client {
    pub fn new(api_key: &str, model: &str) -> Self {
        Self::with_base_url("https://api.anthropic.com/v1", api_key, model)
    }

    pub fn with_base_url(base_url: &str, api_key: &str, model: &str) -> Self {
        let http = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to build HTTP client");
        Self {
            base_url: base_url.to_string(),
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
            .post(format!("{}/messages", self.base_url))
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

    // ── HTTP integration tests (mock server) ─────────────────────────────────

    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn sends_correct_headers_and_parses_response() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/messages"))
            .and(header("x-api-key", "sk-ant-test"))
            .and(header("anthropic-version", "2023-06-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"text": "Root cause: missing semicolon\nFix: add ';' on line 5"}]
            })))
            .mount(&server)
            .await;

        let client = Client::with_base_url(&server.uri(), "sk-ant-test", "claude-haiku-4-5-20251001");
        let result = client.complete("You are helpful.", "error log here").await.unwrap();
        assert!(result.contains("Root cause"));
    }

    #[tokio::test]
    async fn propagates_api_error_status() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .mount(&server)
            .await;

        let client = Client::with_base_url(&server.uri(), "bad-key", "claude-haiku-4-5-20251001");
        let err = client.complete("sys", "user").await.unwrap_err();
        assert!(err.to_string().contains("401"));
    }

    #[tokio::test]
    async fn request_body_contains_model_and_messages() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{"text": "ok"}]
            })))
            .mount(&server)
            .await;

        let client = Client::with_base_url(&server.uri(), "sk-ant-test", "claude-haiku-4-5-20251001");
        client.complete("system prompt", "user message").await.unwrap();

        let req = &server.received_requests().await.unwrap()[0];
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
        assert_eq!(body["model"], "claude-haiku-4-5-20251001");
        assert_eq!(body["system"], "system prompt");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "user message");
        assert_eq!(body["max_tokens"], 512);
    }
}
