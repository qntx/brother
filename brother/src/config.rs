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
