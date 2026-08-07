use anyhow::{Context, Result};
use keyring::Entry;

const SERVICE: &str = "dev-claw";

pub fn store(provider: &str, key: &str) -> Result<()> {
    Entry::new(SERVICE, provider)
        .context("Cannot open keychain entry")?
        .set_password(key)
        .context("Cannot write to OS keychain")
}

pub fn load(provider: &str) -> Result<String> {
    Entry::new(SERVICE, provider)
        .context("Cannot open keychain entry")?
        .get_password()
        .with_context(|| format!(
            "No API key stored for '{provider}'.\n\
             Run: dev-claw config set-key --provider {provider}"
        ))
}

/// Returns the first provider whose key exists in the keychain.
pub fn auto_detect_provider() -> Option<String> {
    for p in ["deepseek", "openai", "claude", "ollama"] {
        if load(p).is_ok() {
            return Some(p.to_string());
        }
    }
    None
}
