//! Action policy system for controlling which commands are allowed.
//!
//! Each command is classified into a category (e.g. `navigate`, `click`, `fill`,
//! `eval`). A policy file specifies which categories are allowed or denied.
//!
//! Policy file format (JSON):
//! ```json
//! {
//!   "default": "allow",
//!   "deny": ["eval", "download"]
//! }
//! ```

use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::{Instant, SystemTime};

use serde::{Deserialize, Serialize};

/// Action categories that commands map to.
const ACTION_CATEGORIES: &[(&str, &str)] = &[
    // Navigation
    ("navigate", "navigate"),
    ("back", "navigate"),
    ("forward", "navigate"),
    ("reload", "navigate"),
    ("tab_new", "navigate"),
    ("window_new", "navigate"),
    // Click
    ("click", "click"),
    ("dbl_click", "click"),
    ("tap", "click"),
    // Fill / input
    ("fill", "fill"),
    ("type", "fill"),
    ("insert_text", "fill"),
    ("select", "fill"),
    ("check", "fill"),
    ("uncheck", "fill"),
    ("clear", "fill"),
    ("select_all", "fill"),
    ("set_value", "fill"),
    // Download
    ("download", "download"),
    ("wait_for_download", "download"),
    ("set_download_path", "download"),
    // Upload
    ("upload", "upload"),
    // Eval / script injection
    ("eval", "eval"),
    ("add_init_script", "eval"),
    ("add_script", "eval"),
    ("add_style", "eval"),
    ("expose", "eval"),
    ("dispatch", "eval"),
    ("set_content", "eval"),
    // Snapshot / observation (read-only)
    ("snapshot", "snapshot"),
    ("screenshot", "snapshot"),
    ("pdf", "snapshot"),
    ("diff_snapshot", "snapshot"),
    ("diff_screenshot", "snapshot"),
    ("diff_url", "snapshot"),
    // Scroll
    ("scroll", "scroll"),
    ("scroll_into_view", "scroll"),
    // Wait
    ("wait", "wait"),
    // Get / query (read-only)
    ("get_text", "get"),
    ("get_content", "get"),
    ("get_html", "get"),
    ("get_inner_text", "get"),
    ("get_value", "get"),
    ("get_url", "get"),
    ("get_title", "get"),
    ("get_attribute", "get"),
    ("count", "get"),
    ("bounding_box", "get"),
    ("styles", "get"),
    ("is_visible", "get"),
    ("is_enabled", "get"),
    ("is_checked", "get"),
    ("nth", "get"),
    ("find", "get"),
    ("response_body", "get"),
    // Network interception
    ("route", "network"),
    ("unroute", "network"),
    ("requests", "network"),
    // State mutation
    ("state_save", "state"),
    ("state_load", "state"),
    ("set_cookies", "state"),
    ("set_cookie", "state"),
    ("set_storage", "state"),
    ("credentials", "state"),
    // Interaction
    ("hover", "interact"),
    ("focus", "interact"),
    ("drag", "interact"),
    ("press", "interact"),
    ("key_down", "interact"),
    ("key_up", "interact"),
    ("mouse_move", "interact"),
    ("mouse_down", "interact"),
    ("mouse_up", "interact"),
    ("wheel", "interact"),
    ("highlight", "interact"),
];

/// Known user-facing categories (excludes internal).
pub const KNOWN_CATEGORIES: &[&str] = &[
    "navigate", "click", "fill", "download", "upload", "eval", "snapshot", "scroll", "wait", "get",
    "network", "state", "interact",
];

/// A loaded action policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPolicy {
    /// Default disposition: `"allow"` or `"deny"`.
    pub default: String,
    /// Categories explicitly allowed.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Categories explicitly denied.
    #[serde(default)]
    pub deny: Vec<String>,
    /// Categories requiring human confirmation before execution.
    #[serde(default)]
    pub confirm: Vec<String>,
}

/// Result of checking a command against the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
    Confirm,
}

/// Get the category for a command name (`snake_case`).
pub fn get_category(cmd: &str) -> Option<&'static str> {
    ACTION_CATEGORIES
        .iter()
        .find(|(name, _)| *name == cmd)
        .map(|(_, cat)| *cat)
}

/// Check whether a command is allowed by the policy.
///
/// Internal/meta commands (those not in the category map) are always allowed.
pub fn check_policy(cmd: &str, policy: &ActionPolicy) -> PolicyDecision {
    let Some(category) = get_category(cmd) else {
        return PolicyDecision::Allow;
    };

    if policy.deny.iter().any(|c| c == category) {
        return PolicyDecision::Deny;
    }

    if policy.confirm.iter().any(|c| c == category) {
        return PolicyDecision::Confirm;
    }

    if policy.allow.iter().any(|c| c == category) {
        return PolicyDecision::Allow;
    }

    if policy.default == "deny" {
        PolicyDecision::Deny
    } else {
        PolicyDecision::Allow
    }
}

/// Load a policy from a JSON file.
///
/// # Errors
///
/// Returns an error string if the file cannot be read or parsed.
pub fn load_policy_file(path: &str) -> Result<ActionPolicy, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("cannot read policy file: {e}"))?;
    let policy: ActionPolicy =
        serde_json::from_str(&content).map_err(|e| format!("invalid policy JSON: {e}"))?;

    if policy.default != "allow" && policy.default != "deny" {
        return Err(format!(
            "invalid policy: \"default\" must be \"allow\" or \"deny\", got \"{}\"",
            policy.default
        ));
    }

    let known: HashSet<&str> = KNOWN_CATEGORIES.iter().copied().collect();
    for cat in policy.allow.iter().chain(policy.deny.iter()) {
        if !known.contains(cat.as_str()) {
            tracing::warn!("unknown action category \"{cat}\" in policy file");
        }
    }

    Ok(policy)
}

/// Build a short human-readable description of a command for confirmation prompts.
pub fn describe_action(cmd: &str, req_json: &serde_json::Value) -> String {
    match cmd {
        "navigate" => {
            let url = req_json.get("url").and_then(|v| v.as_str()).unwrap_or("?");
            format!("navigate to {url}")
        }
        "eval" => {
            let expr = req_json
                .get("expression")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let truncated: String = expr.chars().take(80).collect();
            format!("evaluate JS: {truncated}")
        }
        "click" | "dbl_click" => {
            let target = req_json.get("target").and_then(|v| v.as_str()).unwrap_or("?");
            format!("{cmd} on {target}")
        }
        "fill" | "type" => {
            let target = req_json
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("{cmd} into {target}")
        }
        "download" | "upload" => {
            let target = req_json.get("target").and_then(|v| v.as_str()).unwrap_or("?");
            format!("{cmd} via {target}")
        }
        _ => {
            let category = get_category(cmd).unwrap_or("unknown");
            format!("{cmd} (category: {category})")
        }
    }
}

/// A pending confirmation entry, waiting for user to confirm or deny.
#[allow(dead_code)]
pub struct PendingConfirmation {
    pub cmd_name: String,
    pub category: String,
    pub description: String,
    pub request_json: String,
    pub created: Instant,
}

const AUTO_DENY_TIMEOUT_SECS: u64 = 60;

/// Queue of pending confirmations, keyed by confirmation ID.
pub struct ConfirmationQueue {
    pending: HashMap<String, PendingConfirmation>,
}

impl ConfirmationQueue {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    /// Insert a new pending confirmation. Returns the generated ID.
    pub fn request(&mut self, cmd_name: String, category: String, description: String, request_json: String) -> String {
        self.gc();
        let id = format!("c_{:016x}", rand::random::<u64>());
        self.pending.insert(
            id.clone(),
            PendingConfirmation {
                cmd_name,
                category,
                description,
                request_json,
                created: Instant::now(),
            },
        );
        id
    }

    /// Remove and return a pending confirmation by ID.
    /// Returns `None` if not found or expired.
    pub fn take(&mut self, id: &str) -> Option<PendingConfirmation> {
        let entry = self.pending.remove(id)?;
        if entry.created.elapsed().as_secs() > AUTO_DENY_TIMEOUT_SECS {
            return None;
        }
        Some(entry)
    }

    /// Garbage-collect expired entries.
    fn gc(&mut self) {
        self.pending
            .retain(|_, v| v.created.elapsed().as_secs() <= AUTO_DENY_TIMEOUT_SECS);
    }
}

/// Cached policy with hot-reload support.
pub struct PolicyCache {
    path: String,
    policy: ActionPolicy,
    mtime: SystemTime,
    last_check: Instant,
}

const RELOAD_INTERVAL_SECS: u64 = 5;

impl PolicyCache {
    /// Create a new cache from a loaded policy.
    pub fn new(path: String, policy: ActionPolicy) -> Self {
        let mtime = fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        Self {
            path,
            policy,
            mtime,
            last_check: Instant::now(),
        }
    }

    /// Get the current policy, reloading from disk if the file has changed.
    pub fn get(&mut self) -> &ActionPolicy {
        if self.last_check.elapsed().as_secs() >= RELOAD_INTERVAL_SECS {
            self.last_check = Instant::now();
            if let Ok(meta) = fs::metadata(&self.path)
                && let Ok(new_mtime) = meta.modified()
                && new_mtime != self.mtime
                && let Ok(new_policy) = load_policy_file(&self.path)
            {
                tracing::info!("reloaded action policy from {}", self.path);
                self.policy = new_policy;
                self.mtime = new_mtime;
            }
        }
        &self.policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(default: &str, allow: Vec<&str>, deny: Vec<&str>, confirm: Vec<&str>) -> ActionPolicy {
        ActionPolicy {
            default: default.to_owned(),
            allow: allow.into_iter().map(String::from).collect(),
            deny: deny.into_iter().map(String::from).collect(),
            confirm: confirm.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn default_allow() {
        let policy = policy("allow", vec![], vec![], vec![]);
        assert_eq!(check_policy("navigate", &policy), PolicyDecision::Allow);
        assert_eq!(check_policy("eval", &policy), PolicyDecision::Allow);
    }

    #[test]
    fn default_deny() {
        let policy = policy("deny", vec![], vec![], vec![]);
        assert_eq!(check_policy("navigate", &policy), PolicyDecision::Deny);
        assert_eq!(check_policy("eval", &policy), PolicyDecision::Deny);
    }

    #[test]
    fn explicit_deny_overrides() {
        let policy = policy("allow", vec![], vec!["eval"], vec![]);
        assert_eq!(check_policy("navigate", &policy), PolicyDecision::Allow);
        assert_eq!(check_policy("eval", &policy), PolicyDecision::Deny);
        assert_eq!(
            check_policy("add_init_script", &policy),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn explicit_allow_in_deny_default() {
        let policy = policy("deny", vec!["navigate", "get"], vec![], vec![]);
        assert_eq!(check_policy("navigate", &policy), PolicyDecision::Allow);
        assert_eq!(check_policy("get_text", &policy), PolicyDecision::Allow);
        assert_eq!(check_policy("eval", &policy), PolicyDecision::Deny);
    }

    #[test]
    fn unknown_commands_always_allowed() {
        let policy = policy("deny", vec![], vec![], vec![]);
        // Internal commands like "status", "close" are not in the category map
        assert_eq!(check_policy("status", &policy), PolicyDecision::Allow);
        assert_eq!(check_policy("close", &policy), PolicyDecision::Allow);
    }

    #[test]
    fn category_mapping() {
        assert_eq!(get_category("navigate"), Some("navigate"));
        assert_eq!(get_category("click"), Some("click"));
        assert_eq!(get_category("fill"), Some("fill"));
        assert_eq!(get_category("eval"), Some("eval"));
        assert_eq!(get_category("snapshot"), Some("snapshot"));
        assert_eq!(get_category("status"), None);
    }

    #[test]
    fn confirm_decision() {
        let p = policy("allow", vec![], vec![], vec!["eval", "download"]);
        assert_eq!(check_policy("navigate", &p), PolicyDecision::Allow);
        assert_eq!(check_policy("eval", &p), PolicyDecision::Confirm);
        assert_eq!(check_policy("add_init_script", &p), PolicyDecision::Confirm);
        assert_eq!(check_policy("download", &p), PolicyDecision::Confirm);
    }

    #[test]
    fn deny_overrides_confirm() {
        let p = policy("allow", vec![], vec!["eval"], vec!["eval"]);
        assert_eq!(check_policy("eval", &p), PolicyDecision::Deny);
    }

    #[test]
    fn confirmation_queue_lifecycle() {
        let mut q = ConfirmationQueue::new();
        let id = q.request(
            "eval".into(),
            "eval".into(),
            "evaluate JS".into(),
            "{}".into(),
        );
        assert!(id.starts_with("c_"));
        let entry = q.take(&id).unwrap();
        assert_eq!(entry.cmd_name, "eval");
        assert!(q.take(&id).is_none());
    }

    #[test]
    fn confirmation_queue_unknown_id() {
        let mut q = ConfirmationQueue::new();
        assert!(q.take("nonexistent").is_none());
    }
}
