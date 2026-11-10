//! 更新流程的错误类型

use std::path::PathBuf;

/// 更新流程可能返回的错误
///
/// `check_for_update`、`download_update`、`install_update` 的同步错误直接返回
/// 本类型；检查与下载过程中的异步错误通过 [`AutoUpdateStatus::Failed`] 通知
///
/// [`AutoUpdateStatus::Failed`]: crate::AutoUpdateStatus::Failed
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AutoUpdateError {
    /// HTTP 请求失败，包含连接、TLS 与超时
    ///
    /// `ureq::Error` 内联了完整响应结构，装箱以缩小 `Result` 的体积
    #[error("network error: {0}")]
    Network(Box<ureq::Error>),

    /// 服务端返回非 2xx 状态码
    #[error("http {status} from {url}")]
    Http { status: u16, url: String },

    /// 清单内容无法反序列化
    #[error("manifest parse error: {0}")]
    Manifest(#[from] serde_json::Error),

    /// 清单中的版本号不是合法 semver
    #[error("invalid version `{value}`: {source}")]
    Version {
        value: String,
        #[source]
        source: semver::Error,
    },

    /// 清单中没有当前平台的产物
    #[error("no release for platform `{platform}` (available: {available})")]
    PlatformUnavailable { platform: String, available: String },

    /// 启用了签名校验但清单没有提供签名
    #[error("manifest has no signature for `{0}`")]
    MissingSignature(String),

    /// 公钥解析或签名校验失败
    #[error("signature verification failed: {0}")]
    Signature(#[from] minisign_verify::Error),

    /// 文件读写失败
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// 下载被取消
    #[error("download cancelled")]
    Cancelled,

    /// 当前没有可下载的更新
    #[error("no update available")]
    NoUpdateAvailable,

    /// 已有检查或下载任务在执行
    #[error("update operation already running")]
    Busy,

    /// 尚未下载安装包
    #[error("no update package downloaded yet")]
    NoPackage,

    /// 安装包文件不存在
    #[error("update package not found at {0}")]
    PackageNotFound(PathBuf),

    /// 未配置安装方式，需要调用方自行处理安装包
    #[error("no installer configured; call AutoUpdater::package_path and handle the package")]
    NoInstallerConfigured,
}

impl From<ureq::Error> for AutoUpdateError {
    fn from(error: ureq::Error) -> Self {
        Self::Network(Box::new(error))
    }
}

impl AutoUpdateError {
    /// 是否为可重试的临时错误
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Network(_) | Self::Http { .. } | Self::Io(_))
    }
}
