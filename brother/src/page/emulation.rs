//! Environment emulation: viewport, media, offline, headers, geolocation, credentials,
//! user-agent, timezone, locale, permissions, bring-to-front, download behavior.

use base64::Engine;

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Set the viewport size via CDP `Emulation.setDeviceMetricsOverride`.
    pub async fn set_viewport(&self, width: u32, height: u32) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
        let params =
            SetDeviceMetricsOverrideParams::new(i64::from(width), i64::from(height), 1.0, false);
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Emulate media features (color scheme, print/screen, reduced motion, etc.).
    pub async fn emulate_media(
        &self,
        media: Option<&str>,
        color_scheme: Option<&str>,
        reduced_motion: Option<&str>,
        forced_colors: Option<&str>,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::{
            MediaFeature, SetEmulatedMediaParams,
        };
        let mut params = SetEmulatedMediaParams::default();
        if let Some(m) = media {
            params.media = Some(m.to_owned());
        }
        let mut features = Vec::new();
        if let Some(cs) = color_scheme {
            features.push(MediaFeature::new("prefers-color-scheme", cs));
        }
        if let Some(rm) = reduced_motion {
            features.push(MediaFeature::new("prefers-reduced-motion", rm));
        }
        if let Some(fc) = forced_colors {
            features.push(MediaFeature::new("forced-colors", fc));
        }
        if !features.is_empty() {
            params.features = Some(features);
        }
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Toggle offline mode (intercepts all network at protocol level).
    #[allow(deprecated)] // chromiumoxide marks it experimental, but the CDP method works fine
    pub async fn set_offline(&self, offline: bool) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::EmulateNetworkConditionsParams;
        let params = EmulateNetworkConditionsParams::new(offline, 0., 0., 0.);
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set extra HTTP headers via CDP `Network.setExtraHTTPHeaders`.
    pub async fn set_extra_headers(
        &self,
        headers: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{Headers, SetExtraHttpHeadersParams};
        let params = SetExtraHttpHeadersParams::new(Headers::new(headers));
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override geolocation.
    pub async fn set_geolocation(
        &self,
        latitude: f64,
        longitude: f64,
        accuracy: f64,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetGeolocationOverrideParams;
        let params = SetGeolocationOverrideParams {
            latitude: Some(latitude),
            longitude: Some(longitude),
            accuracy: Some(accuracy),
            ..Default::default()
        };
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set HTTP Basic Auth credentials via extra headers.
    pub async fn set_credentials(&self, username: &str, password: &str) -> Result<()> {
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
        let mut map = serde_json::Map::new();
        map.insert(
            "Authorization".to_owned(),
            serde_json::Value::String(format!("Basic {encoded}")),
        );
        self.set_extra_headers(map).await
    }

    /// Override the browser user-agent string.
    pub async fn set_user_agent(&self, user_agent: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
        let params = SetUserAgentOverrideParams::new(user_agent.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override the timezone.
    pub async fn set_timezone(&self, timezone_id: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetTimezoneOverrideParams;
        let params = SetTimezoneOverrideParams::new(timezone_id.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override the locale (accept-language + `navigator.language`).
    pub async fn set_locale(&self, locale: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
        // Set accept-language at the protocol level (affects HTTP headers).
        let mut params = SetUserAgentOverrideParams::new(String::new());
        // Keep the current user-agent by reading it from navigator first.
        let ua: String = self
            .inner
            .evaluate("navigator.userAgent")
            .await
            .map_err(Error::Cdp)?
            .into_value()
            .unwrap_or_default();
        params.user_agent = ua;
        params.accept_language = Some(locale.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;

        // Also patch navigator.language / languages for JS-side access.
        let escaped = locale.replace('\'', "\\'");
        let js = format!(
            "Object.defineProperty(navigator, 'language', {{ get: () => '{escaped}' }}); \
             Object.defineProperty(navigator, 'languages', {{ get: () => ['{escaped}'] }});"
        );
        self.eval(&js).await?;
        Ok(())
    }

    /// Grant or revoke browser permissions.
    pub async fn set_permissions(&self, permissions: &[String], grant: bool) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::browser::{
            PermissionDescriptor, PermissionSetting, SetPermissionParams,
        };
        let setting = if grant {
            PermissionSetting::Granted
        } else {
            PermissionSetting::Denied
        };
        for perm in permissions {
            let descriptor = PermissionDescriptor::new(perm.clone());
            let params = SetPermissionParams::new(descriptor, setting.clone());
            self.inner.execute(params).await.map_err(Error::Cdp)?;
        }
        Ok(())
    }

    /// Bring the page to front.
    pub async fn bring_to_front(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::BringToFrontParams;
        self.inner
            .execute(BringToFrontParams::default())
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set download behavior via CDP `Browser.setDownloadBehavior`.
    pub async fn set_download_behavior(&self, path: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::browser::{
            SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
        };
        let mut params = SetDownloadBehaviorParams::new(SetDownloadBehaviorBehavior::AllowAndName);
        params.download_path = Some(path.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    // -- Script & style injection, DOM event dispatch --

    /// Add a script to evaluate on every new document (before page JS).
    pub async fn add_init_script(&self, script: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
        let params = AddScriptToEvaluateOnNewDocumentParams::new(script.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Inject a `<script>` tag into the current page.
    pub async fn add_script(&self, content: Option<&str>, url: Option<&str>) -> Result<()> {
        let js = match (content, url) {
            (Some(c), _) => {
                let escaped = c
                    .replace('\\', "\\\\")
                    .replace('`', "\\`")
                    .replace('$', "\\$");
                format!(
                    r"(() => {{ const s = document.createElement('script'); s.textContent = `{escaped}`; document.head.appendChild(s); }})()"
                )
            }
            (_, Some(u)) => {
                let escaped = u.replace('\\', "\\\\").replace('\'', "\\'");
                format!(
                    r"(() => {{ const s = document.createElement('script'); s.src = '{escaped}'; document.head.appendChild(s); }})()"
                )
            }
            _ => return Err(Error::Browser("either content or url is required".into())),
        };
        self.eval(&js).await?;
        Ok(())
    }

    /// Inject a `<style>` or `<link>` tag into the current page.
    pub async fn add_style(&self, content: Option<&str>, url: Option<&str>) -> Result<()> {
        let js = match (content, url) {
            (Some(c), _) => {
                let escaped = c
                    .replace('\\', "\\\\")
                    .replace('`', "\\`")
                    .replace('$', "\\$");
                format!(
                    r"(() => {{ const s = document.createElement('style'); s.textContent = `{escaped}`; document.head.appendChild(s); }})()"
                )
            }
            (_, Some(u)) => {
                let escaped = u.replace('\\', "\\\\").replace('\'', "\\'");
                format!(
                    r"(() => {{ const l = document.createElement('link'); l.rel = 'stylesheet'; l.href = '{escaped}'; document.head.appendChild(l); }})()"
                )
            }
            _ => return Err(Error::Browser("either content or url is required".into())),
        };
        self.eval(&js).await?;
        Ok(())
    }

    /// Dispatch a DOM event on an element.
    pub async fn dispatch_event(
        &self,
        target: &str,
        event: &str,
        event_init: Option<&str>,
    ) -> Result<()> {
        let escaped_event = event.replace('\\', "\\\\").replace('\'', "\\'");
        let init_arg = event_init.map_or_else(|| "{}".to_owned(), ToOwned::to_owned);
        let escaped_sel = target.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r"(() => {{
                const el = document.querySelector('{escaped_sel}');
                if (!el) throw new Error('element not found: {escaped_sel}');
                el.dispatchEvent(new Event('{escaped_event}', {init_arg}));
            }})()"
        );
        self.eval(&js).await?;
        Ok(())
    }
}
