//! Configuration file and environment variable support.
//!
//! Configuration is loaded from three sources (in ascending priority):
//! 1. Default values
//! 2. Config file (`~/.brother/config.toml`)
//! 3. Environment variables (`BROTHER_*`)
//! 4. CLI flags (handled separately in `main.rs`)

use serde::Deserialize;

/// Persistent configuration loaded from file and environment.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Run in headed mode.
    pub headed: Option<bool>,
    /// Output as JSON.
    pub json: Option<bool>,
    /// Proxy server URL.
    pub proxy: Option<String>,
    /// Path to Chrome/Chromium executable.
    pub executable_path: Option<String>,
    /// User data directory for persistent profiles.
    pub user_data_dir: Option<String>,
    /// Additional Chrome arguments (space-separated string).
    pub args: Option<String>,
    /// Custom user-agent string.
    pub user_agent: Option<String>,
    /// Ignore HTTPS errors.
    pub ignore_https_errors: Option<bool>,
    /// Default download directory.
    pub download_path: Option<String>,
    /// Named session for daemon isolation.
    pub session: Option<String>,
    /// Auto-discover and connect to a running Chrome instance.
    pub auto_connect: Option<bool>,
    /// Path to action policy JSON file.
    pub policy_file: Option<String>,
}

/// Load configuration from file + environment variables.
///
/// Priority: file < env vars (env wins).
#[must_use]
pub fn load() -> Config {
    let mut config = load_from_file();
    apply_env(&mut config);
    config
}

/// Read `~/.brother/config.toml` if it exists.
fn load_from_file() -> Config {
    let Some(dir) = dirs::data_local_dir() else {
        return Config::default();
    };
    let path = dir.join("brother").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    match toml::from_str::<Config>(&content) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("warning: invalid config file {}: {e}", path.display());
            Config::default()
        }
    }
}

/// Override config values with `BROTHER_*` environment variables.
fn apply_env(config: &mut Config) {
    if let Ok(val) = std::env::var("BROTHER_HEADED") {
        config.headed = Some(is_truthy(&val));
    }
    if let Ok(val) = std::env::var("BROTHER_JSON") {
        config.json = Some(is_truthy(&val));
    }
    if let Ok(val) = std::env::var("BROTHER_PROXY") {
        config.proxy = Some(val);
    }
    if let Ok(val) = std::env::var("BROTHER_EXECUTABLE_PATH") {
        config.executable_path = Some(val);
    }
    if let Ok(val) = std::env::var("BROTHER_USER_DATA_DIR") {
        config.user_data_dir = Some(val);
    }
    if let Ok(val) = std::env::var("BROTHER_ARGS") {
        config.args = Some(val);
    }
    if let Ok(val) = std::env::var("BROTHER_USER_AGENT") {
        config.user_agent = Some(val);
    }
    if let Ok(val) = std::env::var("BROTHER_IGNORE_HTTPS_ERRORS") {
        config.ignore_https_errors = Some(is_truthy(&val));
    }
    if let Ok(val) = std::env::var("BROTHER_DOWNLOAD_PATH") {
        config.download_path = Some(val);
    }
    if let Ok(val) = std::env::var("BROTHER_SESSION") {
        config.session = Some(val);
    }
    if let Ok(val) = std::env::var("BROTHER_AUTO_CONNECT") {
        config.auto_connect = Some(is_truthy(&val));
    }
    if let Ok(val) = std::env::var("BROTHER_POLICY_FILE") {
        config.policy_file = Some(val);
    }
}

/// Treat `"1"`, `"true"`, `"yes"` (case-insensitive) as truthy.
fn is_truthy(val: &str) -> bool {
    matches!(val.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}
