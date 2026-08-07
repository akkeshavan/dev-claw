use anyhow::{bail, Result};
use std::path::PathBuf;
use std::sync::Mutex;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;

// ── Known providers ───────────────────────────────────────────────────────────

/// LLM providers available for `config set-key` and `auto_detect_provider`.
pub const LLM_PROVIDERS: &[&str] = &["deepseek", "openai", "claude", "sarvam", "mistral"];

/// All valid key names (LLM + cloud tokens).
pub const ALL_PROVIDERS: &[&str] = &[
    "deepseek", "openai", "claude", "sarvam", "mistral",
    "do-token", "hetzner-token",
];

// ── Session key cache ─────────────────────────────────────────────────────────

// Holds the AES-256 key derived from the master passphrase for the lifetime of
// the process. Avoids re-prompting and re-deriving (Argon2 is intentionally slow).
static KEY_CACHE: Mutex<Option<[u8; 32]>> = Mutex::new(None);

// ── Public API ────────────────────────────────────────────────────────────────

/// Encrypt `api_key` with the master passphrase and write to `~/dev-claw/creds/{provider}.enc`.
pub fn store(provider: &str, api_key: &str) -> Result<()> {
    let is_new = !salt_path()?.exists();
    let key    = get_or_derive_key(is_new)?;
    let ciphertext = encrypt(&key, api_key)?;
    std::fs::write(cred_path(provider)?, &ciphertext)
        .map_err(|e| anyhow::anyhow!("Cannot write credential file: {e}"))
}

/// Read and decrypt the stored key for `provider`.
pub fn load(provider: &str) -> Result<String> {
    let path = cred_path(provider)?;
    if !path.exists() {
        bail!(
            "No API key stored for '{provider}'.\n\
             Run: dev-claw config set-key --provider {provider}"
        );
    }
    let data = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("Cannot read credential file: {e}"))?;
    let key  = get_or_derive_key(false)?;
    decrypt(&key, &data)
        .map_err(|_| anyhow::anyhow!(
            "Failed to decrypt '{provider}' — wrong master passphrase?\n\
             Check DEV_CLAW_MASTER env var or re-run with correct passphrase."
        ))
}

/// Returns which providers have credentials stored, in priority order.
/// Checks DEV_CLAW_PROVIDER env var first, then scans the creds directory.
pub fn auto_detect_provider() -> Option<String> {
    if let Ok(p) = std::env::var("DEV_CLAW_PROVIDER") { return Some(p); }
    LLM_PROVIDERS.iter()
        .find(|&&p| cred_path(p).map(|path| path.exists()).unwrap_or(false))
        .map(|s| s.to_string())
}

/// Returns the names of all providers with stored credentials.
pub fn stored_providers() -> Result<Vec<String>> {
    let dir = creds_dir()?;
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') { continue; }
        if let Some(provider) = name.strip_suffix(".enc") {
            found.push(provider.to_string());
        }
    }
    found.sort();
    Ok(found)
}

// ── Paths ─────────────────────────────────────────────────────────────────────

pub fn creds_dir() -> Result<PathBuf> {
    let base = if let Ok(p) = std::env::var("DEV_CLAW_CREDS_DIR") {
        PathBuf::from(p)
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
            .join("dev-claw")
            .join("creds")
    };
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

fn cred_path(provider: &str) -> Result<PathBuf> {
    Ok(creds_dir()?.join(format!("{provider}.enc")))
}

fn salt_path() -> Result<PathBuf> {
    Ok(creds_dir()?.join(".salt"))
}

// ── Salt management ───────────────────────────────────────────────────────────

fn load_or_create_salt() -> Result<Vec<u8>> {
    let path = salt_path()?;
    if path.exists() {
        return std::fs::read(&path)
            .map_err(|e| anyhow::anyhow!("Cannot read salt file: {e}"));
    }
    let mut salt = vec![0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    std::fs::write(&path, &salt)
        .map_err(|e| anyhow::anyhow!("Cannot write salt file: {e}"))?;
    Ok(salt)
}

// ── Key derivation ────────────────────────────────────────────────────────────

pub fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let params = Params::new(65536, 3, 1, Some(32))
        .map_err(|e| anyhow::anyhow!("Argon2 parameter error: {e}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("Key derivation failed: {e}"))?;
    Ok(key)
}

fn get_passphrase(is_new_vault: bool) -> Result<String> {
    if let Ok(p) = std::env::var("DEV_CLAW_MASTER") { return Ok(p); }
    if is_new_vault {
        let p1 = rpassword::prompt_password("Create master passphrase: ")?;
        if p1.is_empty() { bail!("Master passphrase cannot be empty."); }
        let p2 = rpassword::prompt_password("Confirm master passphrase: ")?;
        if p1 != p2 { bail!("Passphrases do not match. No changes written."); }
        return Ok(p1);
    }
    rpassword::prompt_password("Master passphrase: ")
        .map_err(|e| anyhow::anyhow!("Failed to read passphrase: {e}"))
}

fn get_or_derive_key(is_new_vault: bool) -> Result<[u8; 32]> {
    let mut guard = KEY_CACHE.lock().unwrap();
    if let Some(key) = *guard { return Ok(key); }
    let salt       = load_or_create_salt()?;
    let passphrase = get_passphrase(is_new_vault)?;
    let key        = derive_key(&passphrase, &salt)?;
    *guard = Some(key);
    Ok(key)
}

// ── AES-256-GCM encrypt / decrypt ─────────────────────────────────────────────

/// File format: [12-byte random nonce][ciphertext + 16-byte GCM tag]
pub fn encrypt(key: &[u8; 32], plaintext: &str) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce      = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| anyhow::anyhow!("Encryption failed"))?;
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<String> {
    if data.len() < 29 {
        bail!("Credential file is too short to be valid (expected ≥29 bytes).");
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);
    let cipher    = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce     = Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("Decryption failed — wrong passphrase or corrupted file."))?;
    String::from_utf8(plaintext)
        .map_err(|e| anyhow::anyhow!("Decrypted content is not valid UTF-8: {e}"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] { [42u8; 32] }

    // --- encrypt / decrypt round-trip ---

    #[test]
    fn encrypt_decrypt_round_trips() {
        let key   = test_key();
        let plain = "sk-test-api-key-12345";
        let enc   = encrypt(&key, plain).unwrap();
        let dec   = decrypt(&key, &enc).unwrap();
        assert_eq!(dec, plain);
    }

    #[test]
    fn encrypt_produces_different_ciphertext_each_time() {
        let key   = test_key();
        let plain = "same-plaintext";
        let enc1  = encrypt(&key, plain).unwrap();
        let enc2  = encrypt(&key, plain).unwrap();
        assert_ne!(enc1, enc2, "random nonce should produce unique ciphertexts");
    }

    #[test]
    fn decrypt_fails_with_wrong_key() {
        let key1 = test_key();
        let key2 = [99u8; 32];
        let enc  = encrypt(&key1, "secret").unwrap();
        assert!(decrypt(&key2, &enc).is_err(), "wrong key should fail");
    }

    #[test]
    fn decrypt_fails_on_truncated_data() {
        assert!(decrypt(&test_key(), &[0u8; 10]).is_err());
    }

    #[test]
    fn decrypt_fails_on_empty_data() {
        assert!(decrypt(&test_key(), &[]).is_err());
    }

    #[test]
    fn decrypt_fails_on_tampered_ciphertext() {
        let key      = test_key();
        let mut enc  = encrypt(&key, "secret-value").unwrap();
        let last     = enc.len() - 1;
        enc[last]   ^= 0xFF;
        assert!(decrypt(&key, &enc).is_err(), "tampered data should fail authentication");
    }

    // --- derive_key ---

    #[test]
    fn derive_key_is_deterministic() {
        let salt = b"test-salt-32-bytes-exactly!!!!!!";
        let k1   = derive_key("passphrase", salt).unwrap();
        let k2   = derive_key("passphrase", salt).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn derive_key_differs_for_different_passphrases() {
        let salt = b"test-salt-32-bytes-exactly!!!!!!";
        let k1   = derive_key("pass1", salt).unwrap();
        let k2   = derive_key("pass2", salt).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn derive_key_differs_for_different_salts() {
        let k1 = derive_key("same", b"salt-a-32-bytes-exactly!!!!!!!!!").unwrap();
        let k2 = derive_key("same", b"salt-b-32-bytes-exactly!!!!!!!!!").unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn derive_key_output_is_32_bytes() {
        let key = derive_key("x", b"12345678901234567890123456789012").unwrap();
        assert_eq!(key.len(), 32);
    }

    // --- store / load round-trip (uses isolated temp dir via DEV_CLAW_CREDS_DIR) ---

    #[test]
    fn store_and_load_round_trip() {
        let d = tempfile::TempDir::new().unwrap();
        std::env::set_var("DEV_CLAW_CREDS_DIR", d.path());
        std::env::set_var("DEV_CLAW_MASTER", "test-master-passphrase");
        // Clear cache so the test env vars take effect
        *KEY_CACHE.lock().unwrap() = None;

        store("openai", "sk-test-key-abc123").unwrap();
        let loaded = load("openai").unwrap();

        std::env::remove_var("DEV_CLAW_CREDS_DIR");
        std::env::remove_var("DEV_CLAW_MASTER");
        *KEY_CACHE.lock().unwrap() = None;

        assert_eq!(loaded, "sk-test-key-abc123");
    }

    #[test]
    fn load_missing_provider_returns_error() {
        let d = tempfile::TempDir::new().unwrap();
        std::env::set_var("DEV_CLAW_CREDS_DIR", d.path());
        let result = load("nonexistent");
        std::env::remove_var("DEV_CLAW_CREDS_DIR");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nonexistent"));
    }

    // --- stored_providers ---

    #[test]
    fn stored_providers_empty_when_no_creds() {
        let d = tempfile::TempDir::new().unwrap();
        std::env::set_var("DEV_CLAW_CREDS_DIR", d.path());
        let providers = stored_providers().unwrap();
        std::env::remove_var("DEV_CLAW_CREDS_DIR");
        assert!(providers.is_empty());
    }

    // --- auto_detect_provider ---

    #[test]
    fn auto_detect_uses_env_var_first() {
        std::env::set_var("DEV_CLAW_PROVIDER", "mistral");
        let p = auto_detect_provider();
        std::env::remove_var("DEV_CLAW_PROVIDER");
        assert_eq!(p, Some("mistral".to_string()));
    }

    // --- provider lists ---

    #[test]
    fn llm_providers_contains_expected_entries() {
        assert!(LLM_PROVIDERS.contains(&"deepseek"));
        assert!(LLM_PROVIDERS.contains(&"openai"));
        assert!(LLM_PROVIDERS.contains(&"claude"));
        assert!(LLM_PROVIDERS.contains(&"sarvam"));
        assert!(LLM_PROVIDERS.contains(&"mistral"));
    }

    #[test]
    fn all_providers_is_superset_of_llm_providers() {
        for p in LLM_PROVIDERS {
            assert!(ALL_PROVIDERS.contains(p), "{p} missing from ALL_PROVIDERS");
        }
    }
}
