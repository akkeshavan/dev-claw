use anyhow::Result;

use crate::keychain;

const VALID_PROVIDERS: &[&str] = &["deepseek", "openai", "claude", "ollama"];

pub async fn set_key(provider: &str) -> Result<()> {
    validate_provider(provider)?;

    let prompt = format!("Enter API key for {provider} (hidden): ");
    let key    = rpassword::prompt_password(&prompt)?;
    let key    = key.trim();

    if key.is_empty() {
        anyhow::bail!("API key cannot be empty.");
    }

    keychain::store(provider, key)?;
    println!("✓ Key for '{provider}' stored in OS keychain.");
    println!("  Test it:  echo 'SyntaxError: Unexpected token' | dev-claw doctor");
    Ok(())
}

fn validate_provider(provider: &str) -> Result<()> {
    if VALID_PROVIDERS.contains(&provider) {
        return Ok(());
    }
    anyhow::bail!(
        "Unknown provider '{provider}'. Valid options: {}",
        VALID_PROVIDERS.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_all_valid_providers() {
        for p in VALID_PROVIDERS {
            assert!(validate_provider(p).is_ok(), "should accept '{p}'");
        }
    }

    #[test]
    fn rejects_unknown_provider() {
        let err = validate_provider("anthropic").unwrap_err();
        assert!(err.to_string().contains("Unknown provider"));
        assert!(err.to_string().contains("anthropic"));
    }

    #[test]
    fn error_message_lists_valid_options() {
        let err = validate_provider("bad").unwrap_err();
        for p in VALID_PROVIDERS {
            assert!(err.to_string().contains(p), "error should mention '{p}'");
        }
    }
}
