//! Encrypted credential storage for browser authentication.
//!
//! Profiles are stored as AES-256-GCM encrypted JSON files under
//! `~/.brother/auth/`. The encryption key is derived from the
//! `BROTHER_ENCRYPTION_KEY` environment variable or auto-generated
//! and stored in `~/.brother/auth/.key`.
#![allow(dead_code)]

use std::fs;
use std::path::PathBuf;

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use base64::Engine;
use serde::{Deserialize, Serialize};

/// An authentication profile (stored encrypted on disk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfile {
    /// Profile name (alphanumeric + hyphens/underscores).
    pub name: String,
    /// Login page URL.
    pub url: String,
    /// Username / email.
    pub username: String,
    /// Password (encrypted at rest).
    pub password: String,
    /// CSS selector for the username input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username_selector: Option<String>,
    /// CSS selector for the password input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_selector: Option<String>,
    /// CSS selector for the submit button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit_selector: Option<String>,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 last login timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<String>,
}

/// Public metadata (no password).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfileMeta {
    /// Profile name.
    pub name: String,
    /// Login page URL.
    pub url: String,
    /// Username.
    pub username: String,
    /// Creation timestamp.
    pub created_at: String,
    /// Last login timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_login_at: Option<String>,
}

impl From<&AuthProfile> for AuthProfileMeta {
    fn from(p: &AuthProfile) -> Self {
        Self {
            name: p.name.clone(),
            url: p.url.clone(),
            username: p.username.clone(),
            created_at: p.created_at.clone(),
            last_login_at: p.last_login_at.clone(),
        }
    }
}

/// Encrypted payload format stored on disk.
#[derive(Debug, Serialize, Deserialize)]
struct EncryptedPayload {
    /// Base64-encoded 12-byte nonce.
    nonce: String,
    /// Base64-encoded ciphertext.
    ciphertext: String,
}

// ---------------------------------------------------------------------------
// Directory & validation
// ---------------------------------------------------------------------------

fn auth_dir() -> PathBuf {
    let base = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".brother")
        .join("auth");
    if !base.exists() {
        let _ = fs::create_dir_all(&base);
    }
    base
}

fn key_file_path() -> PathBuf {
    auth_dir().join(".key")
}

/// Validate profile name: `[a-zA-Z0-9_-]+`.
fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("profile name cannot be empty".to_owned());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "invalid profile name '{name}': only alphanumeric, hyphens, and underscores allowed"
        ));
    }
    Ok(())
}

fn profile_path(name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    Ok(auth_dir().join(format!("{name}.json")))
}

// ---------------------------------------------------------------------------
// Encryption key management
// ---------------------------------------------------------------------------

fn get_encryption_key() -> Result<[u8; 32], String> {
    // Try environment variable first
    if let Ok(key_str) = std::env::var("BROTHER_ENCRYPTION_KEY") {
        let bytes = key_str.as_bytes();
        if bytes.len() < 32 {
            // Pad/hash to 32 bytes
            let mut key = [0u8; 32];
            for (i, b) in bytes.iter().enumerate() {
                key[i % 32] ^= b;
            }
            return Ok(key);
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes[..32]);
        return Ok(key);
    }

    // Try reading from key file
    let kf = key_file_path();
    if kf.exists() {
        let b64 = fs::read_to_string(&kf).map_err(|e| format!("cannot read key file: {e}"))?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("invalid key file encoding: {e}"))?;
        if decoded.len() != 32 {
            return Err("key file must contain 32 bytes (base64-encoded)".to_owned());
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&decoded);
        return Ok(key);
    }

    // Auto-generate key
    let key: [u8; 32] = rand::random();
    let b64 = base64::engine::general_purpose::STANDARD.encode(key);
    let _ = fs::create_dir_all(auth_dir());
    fs::write(&kf, b64).map_err(|e| format!("cannot write key file: {e}"))?;
    tracing::info!("generated encryption key at {}", kf.display());
    Ok(key)
}

fn encrypt(plaintext: &[u8], raw_key: &[u8; 32]) -> Result<EncryptedPayload, String> {
    let key = Key::<Aes256Gcm>::from_slice(raw_key);
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| format!("encryption failed: {e}"))?;
    let b64 = base64::engine::general_purpose::STANDARD;
    Ok(EncryptedPayload {
        nonce: b64.encode(nonce),
        ciphertext: b64.encode(ciphertext),
    })
}

fn decrypt(payload: &EncryptedPayload, raw_key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let nonce_bytes = b64
        .decode(&payload.nonce)
        .map_err(|e| format!("invalid nonce: {e}"))?;
    let ct_bytes = b64
        .decode(&payload.ciphertext)
        .map_err(|e| format!("invalid ciphertext: {e}"))?;
    if nonce_bytes.len() != 12 {
        return Err("nonce must be 12 bytes".to_owned());
    }
    let nonce = Nonce::from_slice(&nonce_bytes);
    let key = Key::<Aes256Gcm>::from_slice(raw_key);
    let cipher = Aes256Gcm::new(key);
    cipher
        .decrypt(nonce, ct_bytes.as_ref())
        .map_err(|_| "decryption failed — wrong key or corrupted data".to_owned())
}

// ---------------------------------------------------------------------------
// CRUD operations
// ---------------------------------------------------------------------------

fn read_profile(name: &str) -> Result<Option<AuthProfile>, String> {
    let path = profile_path(name)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("cannot read profile: {e}"))?;
    let payload: EncryptedPayload =
        serde_json::from_str(&raw).map_err(|e| format!("invalid profile format: {e}"))?;
    let key = get_encryption_key()?;
    let plaintext = decrypt(&payload, &key)?;
    let profile: AuthProfile = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("invalid decrypted profile: {e}"))?;
    Ok(Some(profile))
}

fn write_profile(profile: &AuthProfile) -> Result<(), String> {
    let key = get_encryption_key()?;
    let json = serde_json::to_string_pretty(profile)
        .map_err(|e| format!("serialization failed: {e}"))?;
    let payload = encrypt(json.as_bytes(), &key)?;
    let encrypted_json =
        serde_json::to_string_pretty(&payload).map_err(|e| format!("serialization failed: {e}"))?;
    let path = profile_path(&profile.name)?;
    fs::write(&path, encrypted_json).map_err(|e| format!("cannot write profile: {e}"))?;
    Ok(())
}

/// Save or update an auth profile.
///
/// # Errors
///
/// Returns an error if encryption or I/O fails.
pub fn save_profile(
    name: &str,
    url: &str,
    username: &str,
    password: &str,
    username_selector: Option<&str>,
    password_selector: Option<&str>,
    submit_selector: Option<&str>,
) -> Result<(AuthProfileMeta, bool), String> {
    let existing = read_profile(name)?;
    let now = chrono_now();

    let profile = AuthProfile {
        name: name.to_owned(),
        url: url.to_owned(),
        username: username.to_owned(),
        password: password.to_owned(),
        username_selector: username_selector.map(ToOwned::to_owned),
        password_selector: password_selector.map(ToOwned::to_owned),
        submit_selector: submit_selector.map(ToOwned::to_owned),
        created_at: existing
            .as_ref()
            .map_or_else(|| now.clone(), |p| p.created_at.clone()),
        last_login_at: existing.as_ref().and_then(|p| p.last_login_at.clone()),
    };

    let updated = existing.is_some();
    write_profile(&profile)?;

    Ok((AuthProfileMeta::from(&profile), updated))
}

/// Get an auth profile by name.
///
/// # Errors
///
/// Returns an error if decryption or I/O fails.
pub fn get_profile(name: &str) -> Result<Option<AuthProfile>, String> {
    read_profile(name)
}

/// Get profile metadata (no password) by name.
///
/// # Errors
///
/// Returns an error if decryption or I/O fails.
pub fn get_profile_meta(name: &str) -> Result<Option<AuthProfileMeta>, String> {
    Ok(read_profile(name)?.map(|p| AuthProfileMeta::from(&p)))
}

/// List all auth profiles (metadata only).
///
/// # Errors
///
/// Returns an error if reading the auth directory fails.
pub fn list_profiles() -> Result<Vec<AuthProfileMeta>, String> {
    let dir = auth_dir();
    let entries =
        fs::read_dir(&dir).map_err(|e| format!("cannot read auth directory: {e}"))?;

    let mut profiles = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.ends_with(".json") {
            continue;
        }
        let profile_name = name_str.trim_end_matches(".json");
        match get_profile_meta(profile_name) {
            Ok(Some(meta)) => profiles.push(meta),
            Ok(None) => {}
            Err(_) => {
                // Can't decrypt — show placeholder
                profiles.push(AuthProfileMeta {
                    name: profile_name.to_owned(),
                    url: "(encrypted)".to_owned(),
                    username: "(encrypted)".to_owned(),
                    created_at: "(unknown)".to_owned(),
                    last_login_at: None,
                });
            }
        }
    }
    Ok(profiles)
}

/// Delete an auth profile by name.
///
/// # Errors
///
/// Returns an error if the name is invalid.
pub fn delete_profile(name: &str) -> Result<bool, String> {
    let path = profile_path(name)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).map_err(|e| format!("cannot delete profile: {e}"))?;
    Ok(true)
}

/// Update the `last_login_at` timestamp for a profile.
///
/// # Errors
///
/// Returns an error if the profile doesn't exist or I/O fails.
pub fn update_last_login(name: &str) -> Result<(), String> {
    let Some(mut profile) = read_profile(name)? else {
        return Err(format!("profile '{name}' not found"));
    };
    profile.last_login_at = Some(chrono_now());
    write_profile(&profile)
}

fn chrono_now() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // Simple ISO-like timestamp without chrono dependency
    let secs = now.as_secs();
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let s = time_secs % 60;
    // Rough date calc (not perfect but good enough for timestamps)
    let mut y = 1970u64;
    let mut remaining_days = days;
    loop {
        let days_in_year = if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0u64;
    for &md in &month_days {
        if remaining_days < md {
            break;
        }
        remaining_days -= md;
        m += 1;
    }
    format!(
        "{y:04}-{:02}-{:02}T{hours:02}:{mins:02}:{s:02}Z",
        m + 1,
        remaining_days + 1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_ok() {
        assert!(validate_name("my-profile").is_ok());
        assert!(validate_name("test_123").is_ok());
    }

    #[test]
    fn validate_name_bad() {
        assert!(validate_name("").is_err());
        assert!(validate_name("../evil").is_err());
        assert!(validate_name("has space").is_err());
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key: [u8; 32] = [42u8; 32];
        let plaintext = b"hello secret world";
        let payload = encrypt(plaintext, &key).expect("encrypt");
        let decrypted = decrypt(&payload, &key).expect("decrypt");
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let key1: [u8; 32] = [1u8; 32];
        let key2: [u8; 32] = [2u8; 32];
        let payload = encrypt(b"secret", &key1).expect("encrypt");
        assert!(decrypt(&payload, &key2).is_err());
    }

    #[test]
    fn chrono_now_format() {
        let ts = chrono_now();
        // Should match ISO-like format
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
    }
}
