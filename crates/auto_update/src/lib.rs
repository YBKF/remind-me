//! 面向 [GPUI](https://gpui.rs) 应用的更新检查与自动安装。
//!
//! 设计目标与 Zed 的 `auto_update` 类似：应用只负责「展示状态」和「触发动作」，
//! 检查清单、比较版本、下载、校验签名、退出时安装这些琐事全部交给本库。
//!
//! # 快速开始
//!
//! ```no_run
//! use auto_update::{AutoUpdateConfig, AutoUpdater, InstallerConfig};
//! use gpui_kit::App;
//!
//! pub fn init_updater(cx: &mut App) {
//!     let config = AutoUpdateConfig::parse(
//!             env!("CARGO_PKG_VERSION"),
//!             "https://example.com/releases/version.json",
//!         )
//!         .expect("CARGO_PKG_VERSION 必然是合法 semver")
//!         .with_public_key("RWQf...minisign 公钥...")
//!         .with_installer(InstallerConfig::inno_setup());
//!
//!     // 创建并注册为全局单例，默认会立即检查一次更新。
//!     let _updater = AutoUpdater::init(cx, config);
//! }
//! ```
//!
//! 在视图里订阅事件即可驱动 UI（`AutoUpdater` 是实体，`cx.subscribe` 可用）：
//!
//! ```ignore
//! cx.subscribe(&updater, |this, updater, event: &AutoUpdateEvent, cx| {
//!     match event {
//!         AutoUpdateEvent::StatusChanged(status) => { /* 切换按钮文案 */ }
//!         AutoUpdateEvent::Progress(progress) => {
//!             // progress.fraction() 可能为 None（服务端没给 Content-Length）
//!             let _ = progress.fraction();
//!         }
//!     }
//!     cx.notify();
//! })
//! .detach();
//! ```
//!
//! # 生命周期
//!
//! ```text
//! Idle ──check_for_update──▶ Checking ──┬─▶ UpdateAvailable ──download_update──▶ Downloading ──▶ Downloaded
//!                                       └─▶ UpToDate                                                    │
//!                                                                                          install_update
//!                                                                                                       ▼
//!                                                                                                  Installing
//! ```
//!
//! 任何一步失败都会进入 `Failed { error }`，可通过 [`AutoUpdater::reset`] 重来。
//!
//! # 错误处理约定
//!
//! * **同步前置错误**（没有可用更新、已有任务在跑、未配置安装策略）由
//!   `check_for_update` / `download_update` / `install_update` 直接返回
//!   [`AutoUpdateError`]。
//! * **异步错误**（网络、解析、签名校验）通过
//!   [`AutoUpdateStatus::Failed`] 与 [`AutoUpdateEvent::StatusChanged`] 通知，
//!   同时写入 `tracing` 日志。
//!
//! # 版本清单
//!
//! 格式与 Zed / Tauri 兼容，平台键可以任意扩展，详见 [`ReleaseManifest`]：
//!
//! ```json
//! {
//!   "version": "v1.2.3",
//!   "notes": "修复了一些问题",
//!   "pub_date": "2026-11-06T17:02:20+08:00",
//!   "platforms": {
//!     "windows-x86_64": { "url": "https://.../Setup.exe", "signature": "..." },
//!     "darwin-aarch64": { "url": "https://.../App.dmg",   "signature": "..." }
//!   }
//! }
//! ```
//!
//! # 平台支持
//!
//! 检查与下载与平台无关；「自动安装」由 [`InstallerConfig`] 描述，库不假设
//! 安装包格式。Windows 上覆盖正在运行的程序需要先退出，因此默认在
//! [`InstallTiming::OnQuit`] 时安装。

mod config;
mod download;
mod error;
mod install;
mod manifest;
mod updater;
mod verify;

pub use config::AutoUpdateConfig;
pub use download::DownloadProgress;
pub use error::AutoUpdateError;
pub use install::{
    InstallHandler, InstallRequest, InstallTiming, InstallerConfig, InstallerProgram,
};
pub use manifest::{platform_key, parse_version, ReleaseAsset, ReleaseManifest};
pub use updater::{
    AutoUpdateEvent, AutoUpdateStatus, AutoUpdater, AvailableUpdate, GlobalAutoUpdater,
};

/// 本 crate 的 `Result` 别名。
pub type Result<T, E = AutoUpdateError> = std::result::Result<T, E>;

/// 重新导出 [`semver`]，方便调用方不多引入一个依赖。
pub use semver;
/// 重新导出 [`ureq`]，方便自定义 `Agent`（代理、证书等）。
pub use ureq;
