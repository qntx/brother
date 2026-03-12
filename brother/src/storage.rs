//! Cookie and web storage (localStorage / sessionStorage) methods.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::page::Page;

// ---------------------------------------------------------------------------
// CookieInput type
// ---------------------------------------------------------------------------

/// Structured cookie input for [`Page::set_cookies`].
///
/// All fields except `name` and `value` are optional. When `url` is omitted,
/// the current page URL is used automatically.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CookieInput {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// URL to associate the cookie with (defaults to current page URL).
    #[serde(default)]
    pub url: Option<String>,
    /// Cookie domain.
    #[serde(default)]
    pub domain: Option<String>,
    /// Cookie path.
    #[serde(default)]
    pub path: Option<String>,
    /// Expiration as Unix timestamp in seconds. `None` = session cookie.
    #[serde(default)]
    pub expires: Option<f64>,
    /// Mark as HTTP-only.
    #[serde(default)]
    pub http_only: Option<bool>,
    /// Mark as secure (HTTPS only).
    #[serde(default)]
    pub secure: Option<bool>,
    /// `SameSite` policy: `"Strict"`, `"Lax"`, or `"None"`.
    #[serde(default)]
    pub same_site: Option<String>,
}

// ---------------------------------------------------------------------------
// Cookie methods
// ---------------------------------------------------------------------------

impl Page {
    /// Get all cookies for the current page.
    pub async fn get_cookies(&self) -> Result<serde_json::Value> {
        use chromiumoxide::cdp::browser_protocol::network::GetCookiesParams;
        let result = self.inner.execute(GetCookiesParams::default()).await.map_err(Error::Cdp)?;
        serde_json::to_value(&result.result.cookies).map_err(|e| Error::Snapshot(format!("cookie serialize: {e}")))
    }

    /// Set a cookie via JS `document.cookie` (simple string format).
    pub async fn set_cookie(&self, cookie_str: &str) -> Result<()> {
        let escaped = cookie_str.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!("document.cookie = '{escaped}'")).await?;
        Ok(())
    }

    /// Set cookies via CDP `Network.setCookies` with full control.
    pub async fn set_cookies(&self, cookies: &[CookieInput]) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{CookieParam, CookieSameSite, SetCookiesParams, TimeSinceEpoch};
        let url = self.url().await.unwrap_or_default();
        let params: Vec<CookieParam> = cookies.iter().map(|c| {
            let mut p = CookieParam::new(c.name.clone(), c.value.clone());
            p.url = Some(c.url.clone().unwrap_or_else(|| url.clone()));
            p.domain.clone_from(&c.domain);
            p.path.clone_from(&c.path);
            if let Some(exp) = c.expires { p.expires = Some(TimeSinceEpoch::new(exp)); }
            p.http_only = c.http_only;
            p.secure = c.secure;
            p.same_site = c.same_site.as_deref().and_then(|s| match s {
                "Strict" | "strict" => Some(CookieSameSite::Strict),
                "Lax" | "lax" => Some(CookieSameSite::Lax),
                "None" | "none" => Some(CookieSameSite::None),
                _ => None,
            });
            p
        }).collect();
        self.inner.execute(SetCookiesParams::new(params)).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Clear all cookies for the current page.
    pub async fn clear_cookies(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{DeleteCookiesParams, GetCookiesParams};
        let result = self.inner.execute(GetCookiesParams::default()).await.map_err(Error::Cdp)?;
        for cookie in &result.result.cookies {
            self.inner.execute(DeleteCookiesParams::new(cookie.name.clone())).await.map_err(Error::Cdp)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Web storage methods
// ---------------------------------------------------------------------------

impl Page {
    /// Get a localStorage or sessionStorage item.
    pub async fn get_storage(&self, key: &str, session: bool) -> Result<String> {
        let storage = if session { "sessionStorage" } else { "localStorage" };
        let escaped = key.replace('\\', "\\\\").replace('\'', "\\'");
        let val = self.eval(&format!("{storage}.getItem('{escaped}')")).await?;
        Ok(val.as_str().unwrap_or("").to_owned())
    }

    /// Set a localStorage or sessionStorage item.
    pub async fn set_storage(&self, key: &str, value: &str, session: bool) -> Result<()> {
        let storage = if session { "sessionStorage" } else { "localStorage" };
        let ek = key.replace('\\', "\\\\").replace('\'', "\\'");
        let ev = value.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!("{storage}.setItem('{ek}', '{ev}')")).await?;
        Ok(())
    }

    /// Clear localStorage or sessionStorage.
    pub async fn clear_storage(&self, session: bool) -> Result<()> {
        let storage = if session { "sessionStorage" } else { "localStorage" };
        self.eval(&format!("{storage}.clear()")).await?;
        Ok(())
    }
}
