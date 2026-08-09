use anyhow::Result;

use crate::creds;

// ── set-key ───────────────────────────────────────────────────────────────────

pub async fn set_key(provider: &str) -> Result<()> {
    validate_provider(provider)?;
    let prompt = format!("{provider} API key: ");
    let api_key = rpassword::prompt_password(&prompt)?;
    let api_key = api_key.trim();
    if api_key.is_empty() {
        anyhow::bail!("API key cannot be empty.");
    }
    creds::store(provider, api_key)?;
    println!("✓ Stored key for '{provider}' in ~/dev-claw/creds/{provider}.enc");
    Ok(())
}

// ── list-keys ─────────────────────────────────────────────────────────────────

pub async fn list_keys() -> Result<()> {
    let dir = creds::creds_dir()?;
    let providers = creds::stored_providers()?;
    println!("Credentials directory: {}", dir.display());
    println!();
    if providers.is_empty() {
        println!("No keys stored yet.");
        println!("Run: dev-claw config set-key --provider <provider>");
        return Ok(());
    }
    for p in &providers {
        let label = if creds::LLM_PROVIDERS.contains(&p.as_str()) {
            "LLM"
        } else {
            "token"
        };
        println!("  ✓  {p}  [{label}]");
    }
    Ok(())
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate_provider(provider: &str) -> Result<()> {
    if creds::ALL_PROVIDERS.contains(&provider) {
        return Ok(());
    }
    anyhow::bail!(
        "Unknown provider '{provider}'. Valid options: {}",
        creds::ALL_PROVIDERS.join(", ")
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_all_llm_providers() {
        for p in creds::LLM_PROVIDERS {
            assert!(validate_provider(p).is_ok(), "should accept '{p}'");
        }
    }

    #[test]
    fn accepts_cloud_tokens() {
        assert!(validate_provider("do-token").is_ok());
        assert!(validate_provider("hetzner-token").is_ok());
    }

    #[test]
    fn rejects_unknown_provider() {
        let err = validate_provider("unknown-xyz").unwrap_err();
        assert!(err.to_string().contains("Unknown provider"));
    }

    #[test]
    fn error_message_lists_valid_options() {
        let err = validate_provider("bad").unwrap_err();
        for p in creds::ALL_PROVIDERS {
            assert!(err.to_string().contains(p), "error should mention '{p}'");
        }
    }
}
