//! Environment emulation: viewport, media, offline, headers, geolocation,
//! credentials, user-agent, timezone, locale, permissions, download behavior.

use base64::Engine;

use crate::error::{Error, Result};
use crate::page::Page;

impl Page {
    /// Set the viewport size via CDP `Emulation.setDeviceMetricsOverride`.
    pub async fn set_viewport(&self, width: u32, height: u32) -> Result<()> {
        self.set_viewport_scaled(width, height, 1.0).await
    }

    /// Set viewport size with a custom device scale factor.
    pub async fn set_viewport_scaled(&self, width: u32, height: u32, device_scale_factor: f64) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
        self.inner.execute(SetDeviceMetricsOverrideParams::new(i64::from(width), i64::from(height), device_scale_factor, false)).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set only the device scale factor (re-applies current viewport dimensions).
    pub async fn set_device_scale_factor(&self, scale: f64) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
        let js = "JSON.stringify({w: window.innerWidth, h: window.innerHeight})";
        let val = self.eval(js).await?;
        let (w, h) = val.as_str().and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .map_or((1280, 720), |v| (v["w"].as_i64().unwrap_or(1280), v["h"].as_i64().unwrap_or(720)));
        self.inner.execute(SetDeviceMetricsOverrideParams::new(w, h, scale, false)).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Emulate media features (color scheme, print/screen, reduced motion, etc.).
    pub async fn emulate_media(&self, media: Option<&str>, color_scheme: Option<&str>, reduced_motion: Option<&str>, forced_colors: Option<&str>) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::{MediaFeature, SetEmulatedMediaParams};
        let mut params = SetEmulatedMediaParams::default();
        if let Some(m) = media { params.media = Some(m.to_owned()); }
        let mut features = Vec::new();
        if let Some(cs) = color_scheme { features.push(MediaFeature::new("prefers-color-scheme", cs)); }
        if let Some(rm) = reduced_motion { features.push(MediaFeature::new("prefers-reduced-motion", rm)); }
        if let Some(fc) = forced_colors { features.push(MediaFeature::new("forced-colors", fc)); }
        if !features.is_empty() { params.features = Some(features); }
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Toggle offline mode.
    #[allow(deprecated)]
    pub async fn set_offline(&self, offline: bool) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::EmulateNetworkConditionsParams;
        self.inner.execute(EmulateNetworkConditionsParams::new(offline, 0., 0., 0.)).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set extra HTTP headers via CDP.
    pub async fn set_extra_headers(&self, headers: serde_json::Map<String, serde_json::Value>) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{Headers, SetExtraHttpHeadersParams};
        self.inner.execute(SetExtraHttpHeadersParams::new(Headers::new(headers))).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override geolocation.
    pub async fn set_geolocation(&self, latitude: f64, longitude: f64, accuracy: f64) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetGeolocationOverrideParams;
        self.inner.execute(SetGeolocationOverrideParams { latitude: Some(latitude), longitude: Some(longitude), accuracy: Some(accuracy), ..Default::default() }).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set HTTP Basic Auth credentials via extra headers.
    pub async fn set_credentials(&self, username: &str, password: &str) -> Result<()> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
        let mut map = serde_json::Map::new();
        map.insert("Authorization".to_owned(), serde_json::Value::String(format!("Basic {encoded}")));
        self.set_extra_headers(map).await
    }

    /// Override the browser user-agent string.
    pub async fn set_user_agent(&self, user_agent: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
        self.inner.execute(SetUserAgentOverrideParams::new(user_agent.to_owned())).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override the timezone.
    pub async fn set_timezone(&self, timezone_id: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::emulation::SetTimezoneOverrideParams;
        self.inner.execute(SetTimezoneOverrideParams::new(timezone_id.to_owned())).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Override the locale (accept-language + `navigator.language`).
    pub async fn set_locale(&self, locale: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
        let ua: String = self.inner.evaluate("navigator.userAgent").await.map_err(Error::Cdp)?.into_value().unwrap_or_default();
        let mut params = SetUserAgentOverrideParams::new(String::new());
        params.user_agent = ua;
        params.accept_language = Some(locale.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        let escaped = locale.replace('\'', "\\'");
        self.eval(&format!("Object.defineProperty(navigator, 'language', {{ get: () => '{escaped}' }}); Object.defineProperty(navigator, 'languages', {{ get: () => ['{escaped}'] }});")).await?;
        Ok(())
    }

    /// Grant or revoke browser permissions.
    pub async fn set_permissions(&self, permissions: &[String], grant: bool) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::browser::{PermissionDescriptor, PermissionSetting, SetPermissionParams};
        let setting = if grant { PermissionSetting::Granted } else { PermissionSetting::Denied };
        for perm in permissions {
            self.inner.execute(SetPermissionParams::new(PermissionDescriptor::new(perm.clone()), setting.clone())).await.map_err(Error::Cdp)?;
        }
        Ok(())
    }

    /// Bring the page to front.
    pub async fn bring_to_front(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::BringToFrontParams;
        self.inner.execute(BringToFrontParams::default()).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set download behavior via CDP.
    pub async fn set_download_behavior(&self, path: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::browser::{SetDownloadBehaviorBehavior, SetDownloadBehaviorParams};
        let mut params = SetDownloadBehaviorParams::new(SetDownloadBehaviorBehavior::AllowAndName);
        params.download_path = Some(path.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Add a script to evaluate on every new document (before page JS).
    pub async fn add_init_script(&self, script: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
        self.inner.execute(AddScriptToEvaluateOnNewDocumentParams::new(script.to_owned())).await.map_err(Error::Cdp)?;
        Ok(())
    }
}
