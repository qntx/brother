//! Domain allowlist enforcement.
//!
//! Matches hostnames against patterns like `"example.com"` (exact) or
//! `"*.example.com"` (wildcard suffix). When active, navigation to
//! non-allowed domains is rejected and an init script patches
//! `WebSocket`, `EventSource`, and `navigator.sendBeacon` in every page.

/// Check whether `hostname` matches at least one allowed pattern.
///
/// - `"example.com"` → exact match
/// - `"*.example.com"` → matches `sub.example.com` and `example.com` itself
pub fn is_allowed(hostname: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return true; // no filter = everything allowed
    }
    let h = hostname.to_ascii_lowercase();
    patterns.iter().any(|pat| {
        let p = pat.to_ascii_lowercase();
        p.strip_prefix("*.").map_or_else(
            || h == p,
            |suffix| h == suffix || h.ends_with(&format!(".{suffix}")),
        )
    })
}

/// Extract hostname from a URL string. Returns `None` for non-http(s) or
/// unparseable URLs.
///
/// Uses simple string parsing to avoid pulling in the `url` crate.
pub fn extract_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split('/').next()?;
    // strip optional userinfo (user:pass@)
    let host_port = authority.rsplit('@').next()?;
    // strip optional port
    let host = if host_port.starts_with('[') {
        // IPv6: [::1]:8080
        host_port.split(']').next().map(|s| &s[1..])
    } else {
        host_port.split(':').next()
    }?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Build a JS init script that monkey-patches `WebSocket`, `EventSource`,
/// and `navigator.sendBeacon` to block connections to non-allowed domains.
pub fn build_init_script(patterns: &[String]) -> String {
    let serialized = serde_json::to_string(patterns).unwrap_or_else(|_| "[]".into());
    format!(
        r#"(function(){{
var _ad={serialized};
function _ok(h){{h=h.toLowerCase();for(var i=0;i<_ad.length;i++){{var p=_ad[i];if(p.indexOf("*.")===0){{var s=p.slice(1);if(h===p.slice(2)||h.slice(-s.length)===s)return true}}else if(h===p)return true}}return false}}
function _chk(u){{try{{return _ok(new URL(u).hostname)}}catch(_){{return false}}}}
if(typeof WebSocket!=="undefined"){{var _WS=WebSocket;WebSocket=function(u,p){{if(!_chk(u))throw new DOMException("blocked by domain allowlist","SecurityError");return p!==void 0?new _WS(u,p):new _WS(u)}};WebSocket.prototype=_WS.prototype;WebSocket.CONNECTING=_WS.CONNECTING;WebSocket.OPEN=_WS.OPEN;WebSocket.CLOSING=_WS.CLOSING;WebSocket.CLOSED=_WS.CLOSED}}
if(typeof EventSource!=="undefined"){{var _ES=EventSource;EventSource=function(u,o){{if(!_chk(u))throw new DOMException("blocked by domain allowlist","SecurityError");return new _ES(u,o)}};EventSource.prototype=_ES.prototype;EventSource.CONNECTING=_ES.CONNECTING;EventSource.OPEN=_ES.OPEN;EventSource.CLOSED=_ES.CLOSED}}
if(typeof navigator!=="undefined"&&typeof navigator.sendBeacon==="function"){{var _sb=navigator.sendBeacon.bind(navigator);navigator.sendBeacon=function(u,d){{return _chk(u)?_sb(u,d):false}}}}
}})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        let p = vec!["example.com".into()];
        assert!(is_allowed("example.com", &p));
        assert!(!is_allowed("other.com", &p));
    }

    #[test]
    fn wildcard_match() {
        let p = vec!["*.example.com".into()];
        assert!(is_allowed("sub.example.com", &p));
        assert!(is_allowed("example.com", &p));
        assert!(!is_allowed("evil.com", &p));
    }

    #[test]
    fn empty_allows_all() {
        assert!(is_allowed("anything.com", &[]));
    }

    #[test]
    fn case_insensitive() {
        let p = vec!["Example.COM".into()];
        assert!(is_allowed("example.com", &p));
        assert!(is_allowed("EXAMPLE.COM", &p));
    }

    #[test]
    fn extract_host_http() {
        assert_eq!(extract_host("https://Foo.COM/bar"), Some("foo.com".into()));
        assert_eq!(extract_host("data:text/html,hi"), None);
    }
}
