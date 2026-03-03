//! Cookie and web storage (localStorage/sessionStorage) methods.

use crate::error::{Error, Result};

use super::{CookieInput, Page};

impl Page {
    /// Get all cookies for the current page.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn get_cookies(&self) -> Result<serde_json::Value> {
        use chromiumoxide::cdp::browser_protocol::network::GetCookiesParams;
        let result = self
            .inner
            .execute(GetCookiesParams::default())
            .await
            .map_err(Error::Cdp)?;
        serde_json::to_value(&result.result.cookies)
            .map_err(|e| Error::Snapshot(format!("cookie serialize: {e}")))
    }

    /// Set a cookie via JS `document.cookie` (simple string format).
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn set_cookie(&self, cookie_str: &str) -> Result<()> {
        let escaped = cookie_str.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!("document.cookie = '{escaped}'")).await?;
        Ok(())
    }

    /// Set cookies via CDP `Network.setCookies` with full control over
    /// all cookie attributes (domain, path, httpOnly, secure, sameSite, expires).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn set_cookies(&self, cookies: &[CookieInput]) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{
            CookieParam, CookieSameSite, SetCookiesParams, TimeSinceEpoch,
        };

        let url = self.url().await.unwrap_or_default();
        let params: Vec<CookieParam> = cookies
            .iter()
            .map(|c| {
                let mut p = CookieParam::new(c.name.clone(), c.value.clone());
                p.url = Some(c.url.clone().unwrap_or_else(|| url.clone()));
                p.domain.clone_from(&c.domain);
                p.path.clone_from(&c.path);
                if let Some(exp) = c.expires {
                    p.expires = Some(TimeSinceEpoch::new(exp));
                }
                p.http_only = c.http_only;
                p.secure = c.secure;
                p.same_site = c.same_site.as_deref().and_then(|s| match s {
                    "Strict" | "strict" => Some(CookieSameSite::Strict),
                    "Lax" | "lax" => Some(CookieSameSite::Lax),
                    "None" | "none" => Some(CookieSameSite::None),
                    _ => None,
                });
                p
            })
            .collect();

        self.inner
            .execute(SetCookiesParams::new(params))
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Clear all cookies for the current page.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn clear_cookies(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{
            DeleteCookiesParams, GetCookiesParams,
        };
        let result = self
            .inner
            .execute(GetCookiesParams::default())
            .await
            .map_err(Error::Cdp)?;
        for cookie in &result.result.cookies {
            self.inner
                .execute(DeleteCookiesParams::new(cookie.name.clone()))
                .await
                .map_err(Error::Cdp)?;
        }
        Ok(())
    }

    /// Get a `localStorage` or `sessionStorage` item.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn get_storage(&self, key: &str, session: bool) -> Result<String> {
        let storage = if session {
            "sessionStorage"
        } else {
            "localStorage"
        };
        let escaped = key.replace('\\', "\\\\").replace('\'', "\\'");
        let val = self
            .eval(&format!("{storage}.getItem('{escaped}')"))
            .await?;
        Ok(val.as_str().unwrap_or("").to_owned())
    }

    /// Set a `localStorage` or `sessionStorage` item.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn set_storage(&self, key: &str, value: &str, session: bool) -> Result<()> {
        let storage = if session {
            "sessionStorage"
        } else {
            "localStorage"
        };
        let ek = key.replace('\\', "\\\\").replace('\'', "\\'");
        let ev = value.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!("{storage}.setItem('{ek}', '{ev}')"))
            .await?;
        Ok(())
    }

    /// Clear `localStorage` or `sessionStorage`.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn clear_storage(&self, session: bool) -> Result<()> {
        let storage = if session {
            "sessionStorage"
        } else {
            "localStorage"
        };
        self.eval(&format!("{storage}.clear()")).await?;
        Ok(())
    }
}
