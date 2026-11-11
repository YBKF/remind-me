//! GPUI 应用的更新检查与自动安装
//!
//! 主程序只需要三步：
//!
//! 1. 用 `AutoUpdateConfig::builder()` 描述清单地址、公钥与安装方式
//! 2. 调用 [`AutoUpdater::init`] 创建更新器并注册为全局单例
//! 3. 订阅 [`AutoUpdateEvent`]，用 [`AutoUpdater::status`] 与
//!    [`AutoUpdater::download_progress`] 渲染
//!
//! ```no_run
//! use auto_update::{AutoUpdateConfig, AutoUpdater, InstallerConfig};
//! use gpui_kit::App;
//! use semver::Version;
//!
//! pub fn init_updater(cx: &mut App) {
//!     let config = AutoUpdateConfig::builder()
//!         .current_version(Version::new(0, 1, 0))
//!         .manifest_url("https://example.com/version.json")
//!         .public_key("RWQf...minisign 公钥...")
//!         .installer_config(InstallerConfig::inno_setup())
//!         .build();
//!
//!     AutoUpdater::init(cx, config);
//! }
//! ```
//!
//! # 状态流转
//!
//! ```text
//! Idle ──check_for_update──▶ Checking ──┬─▶ UpdateAvailable ──download_update──▶ Downloading ──▶ Downloaded
//!                                       └─▶ UpToDate                                                    │
//!                                                                                          install_update
//!                                                                                                       ▼
//!                                                                                                  Installing
//! ```
//!
//! 任一步失败进入 `Failed { error }`，可用 [`AutoUpdater::reset`] 重来
//!
//! # 错误约定
//!
//! 同步前置错误（没有可用更新、已有任务执行、未配置安装方式）由
//! `check_for_update`、`download_update`、`install_update` 直接返回
//! [`AutoUpdateError`]；网络、解析、签名等异步错误通过
//! [`AutoUpdateStatus::Failed`] 与 [`AutoUpdateEvent::StatusChanged`] 通知，
//! 同时写入 `tracing` 日志
//!
//! # 版本清单
//!
//! 格式与 Zed、Tauri 兼容，平台键可任意扩展，见 [`ReleaseManifest`]
//!
//! # 全局安装与用户安装
//!
//! 同一个安装包既能装到 `Program Files` 也能装到 `%LOCALAPPDATA%\Programs`，
//! 更新时必须沿用原有范围，否则用户级安装会要求提权、全局安装会装进用户目录
//!
//! [`InstallerConfig::inno_setup`]、[`InstallerConfig::nsis`]、
//! [`InstallerConfig::msi`] 已内置判断：安装前用 [`InstallScope::detect`]
//! 推断当前 exe 的位置，再追加对应的参数
//!
//! # 安装后清理与重启
//!
//! `InstallerConfig::launch` 只负责启动并立刻返回，安装结束后删除安装包、重新
//! 启动程序需要另一个进程完成。配合 `auto_update_helper` 时用 `--` 分隔参数：
//! `--` 之前是助手的参数，之后原样传给安装包，助手会等待主程序退出、执行安装
//! 包、成功后删除安装包，再启动 `{app}`
//!
//! ```
//! use auto_update::InstallerConfig;
//!
//! let installer_config = InstallerConfig::builder()
//!     .program("tools/auto_update_helper.exe")
//!     .args(bon::vec![
//!         "--installer-path", "{installer}",
//!         "--relaunch", "{app}",
//!         "--wait-pid", "{pid}",
//!         "--auto-remove",
//!         "--",
//!         "/SILENT",
//!     ])
//!     .build();
//! ```
//!
//! # 平台支持
//!
//! 检查与下载与平台无关；安装由 [`InstallerConfig`] 描述，库不假设安装包格式
//! 与安装器类型。Windows 上覆盖正在运行的 exe 需要先退出，因此默认在
//! [`InstallTiming::OnQuit`] 时安装

mod config;
mod download;
mod error;
mod installer;
mod manifest;
mod updater;

pub use config::AutoUpdateConfig;
pub use download::DownloadProgress;
pub use error::AutoUpdateError;
pub use installer::{InstallScope, InstallTiming, InstallerConfig};
pub use manifest::{ReleaseAsset, ReleaseManifest, platform_key};
pub use updater::{AutoUpdateEvent, AutoUpdateStatus, AutoUpdater, AvailableUpdate};

/// 版本号类型，避免调用方额外依赖 `semver`
pub use semver;
/// HTTP 客户端类型，用于 [`AutoUpdateConfig::builder`] 的自定义 agent
pub use ureq;
