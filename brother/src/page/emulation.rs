//! Environment emulation: viewport, media, offline, headers, geolocation, credentials,
//! user-agent, timezone, locale, permissions, bring-to-front, download behavior.

use base64::Engine;

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Set the viewport size via CDP `Emulation.setDeviceMetricsOverride`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn set_viewport(&self, width: u32, height: u32) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
        let params =
            SetDeviceMetricsOverrideParams::new(i64::from(width), i64::from(height), 1.0, false);
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Emulate media features (color scheme, print/screen, reduced motion, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
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

    /// Toggle offline mode via JS fetch/XHR monkey-patch.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn set_offline(&self, offline: bool) -> Result<()> {
        let js = if offline {
            r"(() => {
                if (!window.__brother_offline) {
                    window.__brother_offline = true;
                    const F = window.fetch;
                    window.__brother_orig_fetch = F;
                    window.fetch = function() {
                        if (window.__brother_offline) return Promise.reject(new TypeError('Network request failed (offline mode)'));
                        return F.apply(this, arguments);
                    };
                } else {
                    window.__brother_offline = true;
                }
            })()"
        } else {
            r"(() => { window.__brother_offline = false; })()"
        };
        self.eval(js).await?;
        Ok(())
    }

    /// Set extra HTTP headers via CDP `Network.setExtraHTTPHeaders`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn set_extra_headers(
        &self,
        headers: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{Headers, SetExtraHttpHeadersParams};
        let params = SetExtraHttpHeadersParams::new(Headers::new(headers));
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override geolocation via CDP `Emulation.setGeolocationOverride`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
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

    /// Set HTTP Basic Auth credentials via CDP extra headers.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
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

    /// Override the browser user-agent string via CDP `Network.setUserAgentOverride`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn set_user_agent(&self, user_agent: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
        let params = SetUserAgentOverrideParams::new(user_agent.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override the timezone via CDP `Emulation.setTimezoneOverride`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn set_timezone(&self, timezone_id: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetTimezoneOverrideParams;
        let params = SetTimezoneOverrideParams::new(timezone_id.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override the locale via JS `navigator.language/languages` override.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn set_locale(&self, locale: &str) -> Result<()> {
        let escaped = locale.replace('\'', "\\'");
        let js = format!(
            "Object.defineProperty(navigator, 'language', {{ get: () => '{escaped}' }}); \
             Object.defineProperty(navigator, 'languages', {{ get: () => ['{escaped}'] }});"
        );
        self.eval(&js).await?;
        Ok(())
    }

    /// Grant or revoke browser permissions via CDP `Browser.setPermission`.
    ///
    /// # Errors
    ///
    /// Returns an error if any permission grant/deny fails.
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

    /// Bring the page to front via CDP `Page.bringToFront`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn bring_to_front(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::BringToFrontParams;
        self.inner
            .execute(BringToFrontParams::default())
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set download behavior via CDP `Browser.setDownloadBehavior`.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn set_download_behavior(&self, path: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::browser::{
            SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
        };
        let mut params = SetDownloadBehaviorParams::new(SetDownloadBehaviorBehavior::AllowAndName);
        params.download_path = Some(path.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }
}
