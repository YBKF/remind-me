//! 更新器配置。

use std::{
    fmt,
    path::PathBuf,
    time::Duration,
};

use semver::Version;

use crate::{
    error::AutoUpdateError,
    install::InstallerConfig,
    manifest::{parse_version, platform_key},
};

/// [`AutoUpdater`] 的全部可配置项。
///
/// 通过链式 `with_*` 方法构造：
///
/// ```
/// use auto_update::{AutoUpdateConfig, InstallerConfig};
///
/// let config = AutoUpdateConfig::parse(env!("CARGO_PKG_VERSION"), "https://example.com/version.json")
///     .expect("valid version")
///     .with_public_key("RWQf...")
///     .with_installer(InstallerConfig::inno_setup());
/// # let _ = config;
/// ```
///
/// [`AutoUpdater`]: crate::AutoUpdater
#[derive(Clone)]
pub struct AutoUpdateConfig {
    pub(crate) current_version: Version,
    pub(crate) manifest_url: String,
    pub(crate) public_key: Option<String>,
    pub(crate) platform: String,
    pub(crate) user_agent: String,
    pub(crate) timeout: Duration,
    pub(crate) retries: u32,
    pub(crate) download_dir: Option<PathBuf>,
    pub(crate) installer: InstallerConfig,
    pub(crate) check_on_start: bool,
    pub(crate) http_agent: Option<ureq::Agent>,
}

impl fmt::Debug for AutoUpdateConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AutoUpdateConfig")
            .field("current_version", &self.current_version)
            .field("manifest_url", &self.manifest_url)
            .field(
                "public_key",
                &self.public_key.as_ref().map(|_| "<configured>"),
            )
            .field("platform", &self.platform)
            .field("user_agent", &self.user_agent)
            .field("timeout", &self.timeout)
            .field("retries", &self.retries)
            .field("download_dir", &self.download_dir)
            .field("installer", &self.installer)
            .field("check_on_start", &self.check_on_start)
            .field("http_agent", &self.http_agent.as_ref().map(|_| "<custom>"))
            .finish()
    }
}

impl AutoUpdateConfig {
    /// 创建配置。
    ///
    /// * `current_version`：当前应用版本，建议用 `env!("CARGO_PKG_VERSION")` 解析。
    /// * `manifest_url`：版本清单地址。
    ///
    /// 默认不校验签名（仅日志警告）。生产环境请务必调用
    /// [`AutoUpdateConfig::with_public_key`]。
    pub fn new(current_version: Version, manifest_url: impl Into<String>) -> Self {
        Self {
            current_version,
            manifest_url: manifest_url.into(),
            public_key: None,
            platform: platform_key().to_string(),
            user_agent: format!("auto-update/{}", env!("CARGO_PKG_VERSION")),
            timeout: Duration::from_secs(30),
            retries: 2,
            download_dir: None,
            installer: InstallerConfig::Manual,
            check_on_start: true,
            http_agent: None,
        }
    }

    /// 用字符串版本号创建配置（容忍 `v` 前缀）。
    pub fn parse(
        current_version: &str,
        manifest_url: impl Into<String>,
    ) -> Result<Self, AutoUpdateError> {
        Ok(Self::new(parse_version(current_version)?, manifest_url))
    }

    /// 设置 minisign 公钥，启用安装包签名校验。
    ///
    /// 可以传 `minisign.pub` 的完整内容，也可以只传其中的 base64 公钥。
    pub fn with_public_key(mut self, public_key: impl Into<String>) -> Self {
        self.public_key = Some(public_key.into());
        self
    }

    /// 关闭签名校验（不推荐，仅用于开发调试）。
    pub fn without_signature_verification(mut self) -> Self {
        self.public_key = None;
        self
    }

    /// 指定清单中使用的平台键，默认取 [`platform_key`]。
    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = platform.into();
        self
    }

    /// 设置 HTTP `User-Agent`。
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    /// 设置网络读写超时，默认 30 秒。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// 设置版本清单请求的最大重试次数，默认 2。
    pub fn with_retries(mut self, retries: u32) -> Self {
        self.retries = retries;
        self
    }

    /// 设置安装包下载目录。
    ///
    /// 默认是系统临时目录下的 `<应用名>-updates`。
    pub fn with_download_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.download_dir = Some(dir.into());
        self
    }

    /// 设置安装策略，默认 [`InstallerConfig::Manual`]。
    pub fn with_installer(mut self, installer: InstallerConfig) -> Self {
        self.installer = installer;
        self
    }

    /// 是否在创建更新器时立即检查（默认 `true`）。
    pub fn with_check_on_start(mut self, check: bool) -> Self {
        self.check_on_start = check;
        self
    }

    /// 使用自定义的 `ureq::Agent`（代理、证书、超时等高级场景）。
    pub fn with_http_agent(mut self, agent: ureq::Agent) -> Self {
        self.http_agent = Some(agent);
        self
    }

    /// 当前应用版本。
    pub fn current_version(&self) -> &Version {
        &self.current_version
    }

    /// 版本清单地址。
    pub fn manifest_url(&self) -> &str {
        &self.manifest_url
    }

    /// 清单中使用的平台键。
    pub fn platform(&self) -> &str {
        &self.platform
    }

    /// 安装策略。
    pub fn installer(&self) -> &InstallerConfig {
        &self.installer
    }

    /// 是否在创建时自动检查。
    pub fn check_on_start(&self) -> bool {
        self.check_on_start
    }

    pub(crate) fn download_dir(&self) -> PathBuf {
        self.download_dir
            .clone()
            .unwrap_or_else(default_download_dir)
    }

    pub(crate) fn build_agent(&self) -> ureq::Agent {
        if let Some(agent) = &self.http_agent {
            return agent.clone();
        }
        ureq::AgentBuilder::new()
            .user_agent(&self.user_agent)
            .timeout_connect(self.timeout.min(Duration::from_secs(15)))
            .timeout_read(self.timeout)
            .build()
    }
}

fn default_download_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{}-updates", app_name()))
}

fn app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "app".to_string())
}
