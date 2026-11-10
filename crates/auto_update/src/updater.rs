//! 更新器实体：状态、事件与异步任务编排

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use semver::Version;
use tracing::{info, warn};

use crate::{
    config::AutoUpdateConfig,
    download::{self, DownloadJob, DownloadProgress},
    error::AutoUpdateError,
    installer::InstallTiming,
    manifest::ReleaseManifest,
};

/// 可用的更新
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    /// 远端版本
    pub version: Version,
    /// 更新说明，通常是 Markdown
    pub notes: Option<String>,
    /// 发布时间，原样保留
    pub pub_date: Option<String>,
    /// 安装包下载地址
    pub download_url: String,
    /// minisign 签名
    pub signature: Option<String>,
}

/// 更新器状态
///
/// 只表达生命周期；下载进度用 [`AutoUpdater::download_progress`] 或
/// [`AutoUpdateEvent::Progress`] 读取
#[derive(Clone, Debug)]
pub enum AutoUpdateStatus {
    /// 尚未检查，或已被重置
    Idle,
    /// 正在获取版本清单
    Checking,
    /// 已是最新版本
    UpToDate { current_version: Version },
    /// 发现新版本，等待下载
    UpdateAvailable { version: Version },
    /// 正在下载安装包
    Downloading { version: Version },
    /// 安装包已下载并通过校验，等待安装
    Downloaded { version: Version },
    /// 安装已启动或已安排在退出时执行
    Installing { version: Version },
    /// 出错
    Failed { error: Arc<AutoUpdateError> },
}

impl PartialEq for AutoUpdateStatus {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Idle, Self::Idle) | (Self::Checking, Self::Checking) => true,
            (Self::UpToDate { current_version: a }, Self::UpToDate { current_version: b }) => {
                a == b
            }
            (Self::UpdateAvailable { version: a }, Self::UpdateAvailable { version: b }) => a == b,
            (Self::Downloading { version: a }, Self::Downloading { version: b }) => a == b,
            (Self::Downloaded { version: a }, Self::Downloaded { version: b }) => a == b,
            (Self::Installing { version: a }, Self::Installing { version: b }) => a == b,
            (Self::Failed { error: a }, Self::Failed { error: b }) => {
                a.to_string() == b.to_string()
            }
            _ => false,
        }
    }
}

impl AutoUpdateStatus {
    /// 是否正在检查、下载，或准备安装
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing { .. }
        )
    }
}

/// 更新器事件
#[derive(Clone, Debug, PartialEq)]
pub enum AutoUpdateEvent {
    /// 状态变化
    StatusChanged(AutoUpdateStatus),
    /// 下载进度，已按 1% 或 100ms 节流
    Progress(DownloadProgress),
}

/// 已下载的安装包
#[derive(Clone, Debug)]
struct Package {
    path: PathBuf,
    version: Version,
}

struct GlobalAutoUpdater(Entity<AutoUpdater>);

impl Global for GlobalAutoUpdater {}

/// 更新器实体
///
/// 状态变化通过 [`AutoUpdateEvent`] 广播，也可以用 [`AutoUpdater::status`]
/// 直接读取
pub struct AutoUpdater {
    config: AutoUpdateConfig,
    agent: ureq::Agent,
    status: AutoUpdateStatus,
    available_update: Option<AvailableUpdate>,
    download_progress: Option<DownloadProgress>,
    package: Option<Package>,
    cancel: Option<Arc<AtomicBool>>,
    /// 待退出时安装的包，`on_app_quit` 回调在实体释放后仍需读取
    pending: Arc<Mutex<Option<Package>>>,
}

impl AutoUpdater {
    /// 用 `auto_update_config` 创建更新器
    ///
    /// 构造后 [`AutoUpdater::status`] 为 [`AutoUpdateStatus::Idle`]，
    /// [`AutoUpdater::available_update`]、[`AutoUpdater::download_progress`] 与
    /// [`AutoUpdater::package_path`] 返回 `None`；`installer_config` 为 `Some` 时
    /// 额外注册 App 级退出安装回调
    ///
    /// `auto_update_config.check_on_start` 为 `true` 时立即发起检查，`status`
    /// 随即变为 [`AutoUpdateStatus::Checking`]；返回的 `Entity` 需要调用方持有，
    /// 用 [`AutoUpdater::init`] 可以一并注册为全局单例
    pub fn new(cx: &mut App, auto_update_config: AutoUpdateConfig) -> Entity<Self> {
        let agent = auto_update_config.build_agent();
        let installer_config = auto_update_config.installer_config.clone();
        let pending: Arc<Mutex<Option<Package>>> = Arc::new(Mutex::new(None));

        if auto_update_config.public_key.is_none() {
            warn!("no minisign public key configured, update packages will not be verified");
        }

        // 注册在 App 而不是实体上：关窗时实体会先于 App::shutdown 释放
        if let Some(installer_config) = installer_config {
            let pending = pending.clone();
            cx.on_app_quit(move |_cx| {
                if let Some(package) = pending.lock().unwrap().take()
                    && let Err(error) = installer_config.launch(&package.path, &package.version)
                {
                    warn!(%error, "failed to start update installer on quit");
                }
                async {}
            })
            .detach();
        }

        let check_on_start = auto_update_config.check_on_start;
        cx.new(move |cx| {
            let mut updater = Self {
                config: auto_update_config,
                agent,
                status: AutoUpdateStatus::Idle,
                available_update: None,
                download_progress: None,
                package: None,
                cancel: None,
                pending,
            };
            if check_on_start && let Err(error) = updater.check_for_update(cx) {
                warn!(%error, "initial update check failed");
            }
            updater
        })
    }

    /// 用 `auto_update_config` 创建更新器并注册为全局单例
    ///
    /// 构造结果与 [`AutoUpdater::new`] 相同，另可通过 [`AutoUpdater::global`] 取回
    pub fn init(cx: &mut App, auto_update_config: AutoUpdateConfig) -> Entity<Self> {
        let updater = Self::new(cx, auto_update_config);
        cx.set_global(GlobalAutoUpdater(updater.clone()));
        updater
    }

    /// 取回 [`AutoUpdater::init`] 注册的全局更新器，未注册时返回 `None`
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAutoUpdater>()
            .map(|global| global.0.clone())
    }

    /// 当前状态
    pub fn status(&self) -> &AutoUpdateStatus {
        &self.status
    }

    /// 当前应用版本
    pub fn current_version(&self) -> &Version {
        &self.config.current_version
    }

    /// 可用更新，检查到新版本后到安装前都有效
    pub fn available_update(&self) -> Option<&AvailableUpdate> {
        self.available_update.as_ref()
    }

    /// 下载进度，非下载状态返回 `None`
    pub fn download_progress(&self) -> Option<DownloadProgress> {
        self.download_progress
    }

    /// 已下载并通过校验的安装包路径，未下载时返回 `None`
    pub fn package_path(&self) -> Option<&Path> {
        self.package.as_ref().map(|package| package.path.as_path())
    }

    /// 是否正在检查、下载，或准备安装
    pub fn is_busy(&self) -> bool {
        self.status.is_busy()
    }

    /// 请求 `auto_update_config.manifest_url` 并与当前版本比较
    ///
    /// 立即返回，结果通过 [`AutoUpdateStatus`] 与 [`AutoUpdateEvent`] 通知；
    /// 仅在已有任务执行时返回 [`AutoUpdateError::Busy`]
    pub fn check_for_update(&mut self, cx: &mut Context<Self>) -> Result<(), AutoUpdateError> {
        if self.is_busy() {
            return Err(AutoUpdateError::Busy);
        }

        self.available_update = None;
        self.download_progress = None;
        self.set_status(AutoUpdateStatus::Checking, cx);

        let agent = self.agent.clone();
        let manifest_url = self.config.manifest_url.clone();
        let platform = self.config.platform.clone();
        let retries = self.config.retries;
        let current_version = self.config.current_version.clone();

        cx.spawn(async move |this, cx| {
            let current_version_for_task = current_version.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let release_manifest = fetch_manifest(&agent, &manifest_url, retries)?;
                    resolve_update(&release_manifest, &platform, &current_version_for_task)
                })
                .await;

            this.update(cx, |updater, cx| {
                updater.apply_check_result(result, current_version, cx)
            })
            .ok();
        })
        .detach();

        Ok(())
    }

    /// 下载 [`AutoUpdater::available_update`] 指向的安装包并校验签名
    ///
    /// 同步前置错误（没有可用更新、已有任务执行）直接返回；网络与校验错误
    /// 通过 [`AutoUpdateStatus::Failed`] 通知
    pub fn download_update(&mut self, cx: &mut Context<Self>) -> Result<(), AutoUpdateError> {
        let Some(available_update) = self.available_update.clone() else {
            return Err(AutoUpdateError::NoUpdateAvailable);
        };
        if self.is_busy() {
            return Err(AutoUpdateError::Busy);
        }

        let download_dir = self.config.download_dir();
        std::fs::create_dir_all(&download_dir)?;
        let file_name =
            download::file_name_for(&available_update.download_url, &available_update.version);

        let download_job = DownloadJob {
            client: self.agent.clone(),
            url: available_update.download_url.clone(),
            part_path: download_dir.join(format!("{file_name}.part")),
            final_path: download_dir.join(&file_name),
            public_key: self.config.public_key.clone(),
            signature: available_update.signature.clone(),
        };

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.download_progress = Some(DownloadProgress::default());
        let version = available_update.version.clone();
        self.set_status(
            AutoUpdateStatus::Downloading {
                version: version.clone(),
            },
            cx,
        );

        cx.spawn(async move |this, cx| {
            let (progress_sender, progress_receiver) =
                smol::channel::unbounded::<DownloadProgress>();
            let task = cx.background_executor().spawn(async move {
                download::download_and_verify(&download_job, &progress_sender, &cancel)
            });

            while let Ok(progress) = progress_receiver.recv().await {
                this.update(cx, |updater, cx| {
                    updater.download_progress = Some(progress);
                    cx.emit(AutoUpdateEvent::Progress(progress));
                    cx.notify();
                })
                .ok();
            }

            let result = task.await;
            this.update(cx, |updater, cx| {
                updater.apply_download_result(result, version, cx)
            })
            .ok();
        })
        .detach();

        Ok(())
    }

    /// 安装 [`AutoUpdater::package_path`] 指向的安装包
    ///
    /// [`InstallTiming::Immediately`] 时立即启动安装程序；[`InstallTiming::OnQuit`]
    /// 时登记到退出回调，`status` 均变为 [`AutoUpdateStatus::Installing`]
    ///
    /// 未下载安装包返回 [`AutoUpdateError::NoPackage`]，未配置
    /// `installer_config` 返回 [`AutoUpdateError::NoInstallerConfigured`]
    pub fn install_update(&mut self, cx: &mut Context<Self>) -> Result<(), AutoUpdateError> {
        let Some(package) = self.package.clone() else {
            return Err(AutoUpdateError::NoPackage);
        };
        let Some(installer_config) = self.config.installer_config.clone() else {
            return Err(AutoUpdateError::NoInstallerConfigured);
        };
        if !package.path.exists() {
            self.package = None;
            return Err(AutoUpdateError::PackageNotFound(package.path));
        }

        match installer_config.timing() {
            InstallTiming::Immediately => {
                installer_config.launch(&package.path, &package.version)?
            }
            InstallTiming::OnQuit => *self.pending.lock().unwrap() = Some(package.clone()),
        }

        self.set_status(
            AutoUpdateStatus::Installing {
                version: package.version,
            },
            cx,
        );
        Ok(())
    }

    /// 取消下载，`status` 回到 [`AutoUpdateStatus::UpdateAvailable`]
    ///
    /// 后台任务在下一个检查点退出并删除半成品
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        if let AutoUpdateStatus::Downloading { version } = self.status.clone() {
            self.download_progress = None;
            self.set_status(AutoUpdateStatus::UpdateAvailable { version }, cx);
        }
    }

    /// 清空状态与已下载的安装包，`status` 回到 [`AutoUpdateStatus::Idle`]
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.available_update = None;
        self.download_progress = None;
        self.cancel = None;
        *self.pending.lock().unwrap() = None;
        self.discard_package();
        self.set_status(AutoUpdateStatus::Idle, cx);
    }

    /// 应用检查结果，`current_version` 用于 [`AutoUpdateStatus::UpToDate`]
    fn apply_check_result(
        &mut self,
        result: Result<Option<AvailableUpdate>, AutoUpdateError>,
        current_version: Version,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(Some(available_update)) => {
                let version = available_update.version.clone();
                let reusable = self
                    .package
                    .as_ref()
                    .is_some_and(|package| package.version == version && package.path.exists());
                if !reusable {
                    self.discard_package();
                }

                info!(%version, current = %self.config.current_version, "update available");
                self.available_update = Some(available_update);
                let status = if self.package.is_some() {
                    AutoUpdateStatus::Downloaded { version }
                } else {
                    AutoUpdateStatus::UpdateAvailable { version }
                };
                self.set_status(status, cx);
            }
            Ok(None) => {
                info!(%current_version, "already up to date");
                self.set_status(AutoUpdateStatus::UpToDate { current_version }, cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    /// 应用下载结果，`version` 用于状态与安装包记录
    fn apply_download_result(
        &mut self,
        result: Result<PathBuf, AutoUpdateError>,
        version: Version,
        cx: &mut Context<Self>,
    ) {
        self.cancel = None;
        self.download_progress = None;

        match result {
            Ok(path) => {
                info!(path = %path.display(), %version, "update package downloaded");
                self.package = Some(Package {
                    path,
                    version: version.clone(),
                });
                self.set_status(AutoUpdateStatus::Downloaded { version }, cx);

                let install_now =
                    self.config
                        .installer_config
                        .as_ref()
                        .is_some_and(|installer_config| {
                            installer_config.timing() == InstallTiming::Immediately
                        });
                if install_now && let Err(error) = self.install_update(cx) {
                    self.fail(error, cx);
                }
            }
            Err(AutoUpdateError::Cancelled) => {
                self.set_status(AutoUpdateStatus::UpdateAvailable { version }, cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    /// 删除已下载的安装包及其文件
    fn discard_package(&mut self) {
        if let Some(package) = self.package.take() {
            let _ = std::fs::remove_file(&package.path);
        }
    }

    fn fail(&mut self, error: AutoUpdateError, cx: &mut Context<Self>) {
        warn!(%error, "update failed");
        self.set_status(
            AutoUpdateStatus::Failed {
                error: Arc::new(error),
            },
            cx,
        );
    }

    fn set_status(&mut self, status: AutoUpdateStatus, cx: &mut Context<Self>) {
        if self.status == status {
            return;
        }
        self.status = status.clone();
        cx.emit(AutoUpdateEvent::StatusChanged(status));
        cx.notify();
    }
}

impl EventEmitter<AutoUpdateEvent> for AutoUpdater {}

/// 请求清单，网络类错误按 `retries` 重试
fn fetch_manifest(
    agent: &ureq::Agent,
    url: &str,
    retries: u32,
) -> Result<ReleaseManifest, AutoUpdateError> {
    let mut attempt = 0;
    loop {
        match request_manifest(agent, url) {
            Ok(release_manifest) => return Ok(release_manifest),
            Err(error) if error.is_retryable() && attempt < retries => {
                attempt += 1;
                warn!(%error, attempt, retries, "failed to fetch update manifest, retrying");
                std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
            }
            Err(error) => return Err(error),
        }
    }
}

/// 请求一次清单
fn request_manifest(agent: &ureq::Agent, url: &str) -> Result<ReleaseManifest, AutoUpdateError> {
    let response = agent.get(url).call()?;
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(AutoUpdateError::Http {
            status,
            url: url.to_owned(),
        });
    }
    Ok(serde_json::from_reader(response.into_reader())?)
}

/// 从 `release_manifest` 中取 `platform` 的产物
///
/// 清单版本不高于 `current_version` 时返回 `None`
fn resolve_update(
    release_manifest: &ReleaseManifest,
    platform: &str,
    current_version: &Version,
) -> Result<Option<AvailableUpdate>, AutoUpdateError> {
    let version = release_manifest.semver()?;
    if &version <= current_version {
        return Ok(None);
    }

    let release_asset = release_manifest.asset_for(platform).ok_or_else(|| {
        AutoUpdateError::PlatformUnavailable {
            platform: platform.to_owned(),
            available: release_manifest.available_platforms(),
        }
    })?;

    Ok(Some(AvailableUpdate {
        version,
        notes: release_manifest.notes.clone(),
        pub_date: release_manifest.pub_date.clone(),
        download_url: release_asset.url.clone(),
        signature: release_asset.signature.clone(),
    }))
}
