//! Browser launch configuration.

use std::path::PathBuf;

/// Configuration for launching a browser instance.
#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// Run browser in headless mode. Defaults to `true`.
    pub headless: bool,

    /// Browser viewport width in pixels.
    pub viewport_width: u32,

    /// Browser viewport height in pixels.
    pub viewport_height: u32,

    /// Path to a Chrome/Chromium executable. Auto-detected if `None`.
    pub executable: Option<PathBuf>,

    /// User data directory for persistent profiles. Temporary if `None`.
    pub user_data_dir: Option<PathBuf>,

    /// Additional Chrome launch arguments.
    pub args: Vec<String>,

    /// Proxy server URL (e.g. `http://localhost:8080`).
    pub proxy: Option<String>,

    /// Custom user-agent string.
    pub user_agent: Option<String>,

    /// Disable GPU acceleration. Defaults to `true`.
    pub disable_gpu: bool,

    /// Sandbox mode. Disabled by default for container compatibility.
    pub sandbox: bool,

    /// Ignore HTTPS/TLS certificate errors. Defaults to `false`.
    pub ignore_https_errors: bool,

    /// Default download directory for browser downloads.
    pub download_path: Option<PathBuf>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            headless: true,
            viewport_width: 1280,
            viewport_height: 720,
            executable: None,
            user_data_dir: None,
            args: Vec::new(),
            proxy: None,
            user_agent: None,
            disable_gpu: true,
            sandbox: false,
            ignore_https_errors: false,
            download_path: None,
        }
    }
}

impl BrowserConfig {
    /// Set headless mode.
    #[must_use]
    pub const fn headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    /// Set viewport dimensions.
    #[must_use]
    pub const fn viewport(mut self, width: u32, height: u32) -> Self {
        self.viewport_width = width;
        self.viewport_height = height;
        self
    }

    /// Set Chrome executable path.
    #[must_use]
    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    /// Set user data directory for persistent profiles.
    #[must_use]
    pub fn user_data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.user_data_dir = Some(path.into());
        self
    }

    /// Set proxy server URL.
    #[must_use]
    pub fn proxy(mut self, url: impl Into<String>) -> Self {
        self.proxy = Some(url.into());
        self
    }

    /// Set custom user-agent string.
    #[must_use]
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    /// Ignore HTTPS/TLS certificate errors (useful for self-signed certs).
    #[must_use]
    pub const fn ignore_https_errors(mut self, ignore: bool) -> Self {
        self.ignore_https_errors = ignore;
        self
    }

    /// Set default download directory.
    #[must_use]
    pub fn download_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.download_path = Some(path.into());
        self
    }

    /// Apply a device preset (viewport + user-agent).
    #[must_use]
    pub fn device(mut self, name: &str) -> Self {
        if let Some(preset) = DevicePreset::lookup(name) {
            self.viewport_width = preset.width;
            self.viewport_height = preset.height;
            self.user_agent = Some(preset.user_agent.to_owned());
        }
        self
    }

    /// Convert to `chromiumoxide::BrowserConfig`.
    pub(crate) fn into_chromium_config(self) -> crate::Result<chromiumoxide::BrowserConfig> {
        let mut builder = chromiumoxide::BrowserConfig::builder();

        if self.headless {
            builder = builder.arg("--headless=new");
        }

        builder = builder.window_size(self.viewport_width, self.viewport_height);

        if let Some(ref exec) = self.executable {
            builder = builder.chrome_executable(exec);
        }

        if let Some(ref dir) = self.user_data_dir {
            builder = builder.user_data_dir(dir);
        }

        if self.disable_gpu {
            builder = builder.arg("--disable-gpu");
        }

        if !self.sandbox {
            builder = builder.arg("--no-sandbox");
        }

        if self.ignore_https_errors {
            builder = builder.arg("--ignore-certificate-errors");
        }

        // Standard agent-friendly flags
        builder = builder
            .arg("--disable-background-networking")
            .arg("--disable-default-apps")
            .arg("--disable-extensions")
            .arg("--disable-sync")
            .arg("--disable-translate")
            .arg("--no-first-run")
            .arg("--disable-component-update");

        if let Some(ref proxy_url) = self.proxy {
            builder = builder.arg(format!("--proxy-server={proxy_url}"));
        }

        for arg in &self.args {
            builder = builder.arg(arg.as_str());
        }

        builder.build().map_err(crate::Error::Browser)
    }
}

/// A named device preset with viewport dimensions, user-agent, and scale factor.
#[derive(Debug, Clone, Copy)]
pub struct DevicePreset {
    /// Device name.
    pub name: &'static str,
    /// Viewport width in pixels.
    pub width: u32,
    /// Viewport height in pixels.
    pub height: u32,
    /// User-agent string.
    pub user_agent: &'static str,
    /// Device pixel ratio (1.0 for standard, 2.0–3.0 for HiDPI/Retina).
    pub device_scale_factor: f64,
}

/// All available device presets.
pub const DEVICE_PRESETS: &[DevicePreset] = &[
    DevicePreset {
        name: "iphone-14",
        width: 390,
        height: 844,
        user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
        device_scale_factor: 3.0,
    },
    DevicePreset {
        name: "iphone-14-pro-max",
        width: 430,
        height: 932,
        user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1",
        device_scale_factor: 3.0,
    },
    DevicePreset {
        name: "pixel-7",
        width: 412,
        height: 915,
        user_agent: "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36",
        device_scale_factor: 2.625,
    },
    DevicePreset {
        name: "ipad-pro",
        width: 1024,
        height: 1366,
        user_agent: "Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Safari/604.1",
        device_scale_factor: 2.0,
    },
    DevicePreset {
        name: "ipad-mini",
        width: 768,
        height: 1024,
        user_agent: "Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Safari/604.1",
        device_scale_factor: 2.0,
    },
    DevicePreset {
        name: "galaxy-s23",
        width: 360,
        height: 780,
        user_agent: "Mozilla/5.0 (Linux; Android 13; SM-S911B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36",
        device_scale_factor: 3.0,
    },
    DevicePreset {
        name: "desktop-hd",
        width: 1920,
        height: 1080,
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        device_scale_factor: 1.0,
    },
    DevicePreset {
        name: "desktop",
        width: 1280,
        height: 720,
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        device_scale_factor: 1.0,
    },
    DevicePreset {
        name: "laptop",
        width: 1366,
        height: 768,
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        device_scale_factor: 2.0,
    },
];

impl DevicePreset {
    /// Look up a device preset by name (case-insensitive).
    #[must_use]
    pub fn lookup(name: &str) -> Option<&'static Self> {
        let lower = name.to_ascii_lowercase();
        DEVICE_PRESETS.iter().find(|d| d.name == lower)
    }

    /// List all available device preset names.
    #[must_use]
    pub fn list_names() -> Vec<&'static str> {
        DEVICE_PRESETS.iter().map(|d| d.name).collect()
    }
}
