use anyhow::Result;
use async_trait::async_trait;

pub mod anthropic;
pub mod openai_compat;

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, system: &str, user: &str) -> Result<String>;
}

/// Returns the correct LLM client for a provider name.
/// DeepSeek, OpenAI, Groq, Ollama, OpenRouter, Sarvam, and Mistral all use the OpenAI-compatible endpoint.
pub fn client_for(provider: &str, api_key: &str) -> Box<dyn LlmClient> {
    match provider {
        "deepseek" => Box::new(openai_compat::Client::new(
            "https://api.deepseek.com/v1",
            api_key,
            "deepseek-chat",
        )),
        "openai" => Box::new(openai_compat::Client::new(
            "https://api.openai.com/v1",
            api_key,
            "gpt-4o-mini",
        )),
        "groq" => Box::new(openai_compat::Client::new(
            "https://api.groq.com/openai/v1",
            api_key,
            "llama-3.3-70b-versatile",
        )),
        "ollama" => Box::new(openai_compat::Client::new(
            "http://localhost:11434/v1",
            "ollama",
            "llama3",
        )),
        "openrouter" => Box::new(openai_compat::Client::new(
            "https://openrouter.ai/api/v1",
            api_key,
            "openai/gpt-4o-mini",
        )),
        "anthropic" | "claude" => {
            Box::new(anthropic::Client::new(api_key, "claude-haiku-4-5-20251001"))
        }
        "sarvam" => Box::new(openai_compat::Client::new(
            "https://api.sarvam.ai/v1",
            api_key,
            "sarvam-m",
        )),
        "mistral" => Box::new(openai_compat::Client::new(
            "https://api.mistral.ai/v1",
            api_key,
            "mistral-small-latest",
        )),
        other => {
            eprintln!("Unknown provider '{other}', falling back to deepseek");
            Box::new(openai_compat::Client::new(
                "https://api.deepseek.com/v1",
                api_key,
                "deepseek-chat",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_providers_return_clients() {
        for provider in [
            "deepseek",
            "openai",
            "groq",
            "ollama",
            "openrouter",
            "anthropic",
            "claude",
            "sarvam",
            "mistral",
        ] {
            let _ = client_for(provider, "sk-test");
        }
    }

    #[test]
    fn unknown_provider_falls_back_without_panic() {
        let _ = client_for("nonexistent", "sk-test");
    }
}
