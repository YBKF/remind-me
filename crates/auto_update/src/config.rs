//! 更新器配置

use std::{fmt, path::PathBuf, time::Duration};

use semver::Version;

use crate::{installer::InstallerConfig, manifest::platform_key};

/// 更新器配置
///
/// 用 `AutoUpdateConfig::builder()` 构造，`current_version` 与 `manifest_url`
/// 必填，其余字段不设置时取默认值：`public_key` 为 `None`（不校验签名），
/// `platform` 为 [`platform_key`]，`user_agent` 为 `auto-update/<本 crate 版本>`，
/// `timeout` 为 30 秒，`retries` 为 2，`download_dir` 为 `None`（运行时取系统
/// 临时目录下的 `<应用名>-updates`），`installer_config` 为 `None`（只下载不安装），
/// `check_on_start` 为 `true`，`agent` 为 `None`
///
/// ```
/// use auto_update::AutoUpdateConfig;
/// use semver::Version;
///
/// let config = AutoUpdateConfig::builder()
///     .current_version(Version::new(1, 0, 0))
///     .manifest_url("https://example.com/version.json")
///     .build();
/// # let _ = config;
/// ```
#[derive(Clone, bon::Builder)]
#[builder(on(String, into))]
pub struct AutoUpdateConfig {
    /// 当前应用版本，用于与清单版本比较
    pub(crate) current_version: Version,

    /// 版本清单地址
    pub(crate) manifest_url: String,

    /// minisign 公钥，`None` 表示不校验安装包签名
    pub(crate) public_key: Option<String>,

    /// 清单中使用的平台键，默认取 [`platform_key`]
    #[builder(default = platform_key().to_owned())]
    pub(crate) platform: String,

    /// HTTP `User-Agent`
    #[builder(default = default_user_agent())]
    pub(crate) user_agent: String,

    /// 网络读写超时
    #[builder(default = Duration::from_secs(30))]
    pub(crate) timeout: Duration,

    /// 清单请求的最大重试次数
    #[builder(default = 2)]
    pub(crate) retries: u32,

    /// 安装包存放目录，默认是系统临时目录下的 `<应用名>-updates`
    #[builder(into)]
    pub(crate) download_dir: Option<PathBuf>,

    /// 下载完成后的安装方式，`None` 表示只下载不安装
    pub(crate) installer_config: Option<InstallerConfig>,

    /// 创建更新器时立即检查一次更新
    #[builder(default = true)]
    pub(crate) check_on_start: bool,

    /// 自定义 HTTP 客户端，用于代理、证书等场景
    pub(crate) agent: Option<ureq::Agent>,
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
            .field("installer_config", &self.installer_config)
            .field("check_on_start", &self.check_on_start)
            .field("agent", &self.agent.as_ref().map(|_| "<custom>"))
            .finish()
    }
}

impl AutoUpdateConfig {
    /// 安装包存放目录，`download_dir` 未设置时返回系统临时目录下的默认目录
    pub(crate) fn download_dir(&self) -> PathBuf {
        self.download_dir
            .clone()
            .unwrap_or_else(default_download_dir)
    }

    /// 按 `agent`、`user_agent`、`timeout` 构造 HTTP 客户端
    ///
    /// `agent` 已设置时直接返回它，忽略其余字段
    pub(crate) fn build_agent(&self) -> ureq::Agent {
        if let Some(agent) = &self.agent {
            return agent.clone();
        }
        ureq::AgentBuilder::new()
            .user_agent(&self.user_agent)
            .timeout_connect(self.timeout.min(Duration::from_secs(15)))
            .timeout_read(self.timeout)
            .build()
    }
}

fn default_user_agent() -> String {
    format!("auto-update/{}", env!("CARGO_PKG_VERSION"))
}

fn default_download_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{}-updates", app_name()))
}

fn app_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "app".to_owned())
}
