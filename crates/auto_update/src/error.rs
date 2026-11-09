use std::path::PathBuf;

/// `auto_update` 中所有可能出现的错误。
///
/// 异步流程（检查、下载）里的错误会通过 [`AutoUpdateStatus::Failed`] 和
/// [`AutoUpdateEvent::StatusChanged`] 通知；同步入口（例如
/// [`AutoUpdater::download_update`]）则直接返回 `Result`，方便调用方立即
/// 分支处理。
///
/// [`AutoUpdateStatus::Failed`]: crate::AutoUpdateStatus::Failed
/// [`AutoUpdateEvent::StatusChanged`]: crate::AutoUpdateEvent::StatusChanged
/// [`AutoUpdater::download_update`]: crate::AutoUpdater::download_update
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AutoUpdateError {
    /// HTTP 请求失败（连接、TLS、超时等）。
    ///
    /// 装箱是为了让 `Result<_, AutoUpdateError>` 保持小尺寸（`ureq::Error`
    /// 内联了完整的响应结构）。
    #[error("network error: {0}")]
    Network(Box<ureq::Error>),

    /// 服务端返回了非 2xx 状态码。
    #[error("unexpected HTTP status {status} while requesting {url}")]
    Http { status: u16, url: String },

    /// 版本清单 JSON 无法反序列化。
    #[error("failed to parse release manifest: {0}")]
    Manifest(#[from] serde_json::Error),

    /// 清单中的版本号不是合法 semver。
    #[error("invalid version `{value}`: {source}")]
    Version {
        value: String,
        #[source]
        source: semver::Error,
    },

    /// 清单中没有当前平台的产物。
    #[error("manifest has no release for platform `{platform}` (available: {available})")]
    PlatformUnavailable { platform: String, available: String },

    /// 启用了签名校验，但清单中该平台没有签名。
    #[error("manifest entry for `{0}` has no signature")]
    MissingSignature(String),

    /// 签名校验失败（密钥不匹配、内容被篡改等）。
    #[error("signature verification failed: {0}")]
    Signature(#[from] minisign_verify::Error),

    /// 公钥格式非法。
    #[error("invalid minisign public key: {0}")]
    PublicKey(#[source] minisign_verify::Error),

    /// 文件系统错误。
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 用户取消了下载。
    #[error("update cancelled")]
    Cancelled,

    /// 当前没有可用更新（尚未检查、已是最新或已安装）。
    #[error("no update available")]
    NoUpdateAvailable,

    /// 已有检查或下载任务在进行中。
    #[error("another update operation is already running")]
    Busy,

    /// 还没有下载好的安装包。
    #[error("no installer has been downloaded yet")]
    NoInstaller,

    /// 安装包文件不存在（可能被清理了）。
    #[error("installer not found at {0}")]
    InstallerNotFound(PathBuf),

    /// 安装策略为 [`InstallerConfig::Manual`]，库不会自动安装。
    ///
    /// [`InstallerConfig::Manual`]: crate::InstallerConfig::Manual
    #[error("no install strategy configured; handle the downloaded installer yourself")]
    ManualInstallRequired,
}

impl From<ureq::Error> for AutoUpdateError {
    fn from(error: ureq::Error) -> Self {
        Self::Network(Box::new(error))
    }
}

impl AutoUpdateError {
    /// 是否值得重试（网络类错误）。
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Http { .. } | Self::Io(_)
        )
    }
}
