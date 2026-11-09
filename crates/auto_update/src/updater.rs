//! 更新器实体：状态机 + 事件 + 异步任务编排。

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global};
use semver::Version;
use tracing::{info, warn};

use crate::{
    config::AutoUpdateConfig,
    download::{self, DownloadProgress, DownloadRequest},
    error::AutoUpdateError,
    install::{self, InstallTiming},
    manifest::ReleaseManifest,
};

/// 可用的更新。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    /// 远端版本。
    pub version: Version,
    /// 更新说明（通常是 Markdown）。
    pub notes: Option<String>,
    /// 发布时间（原样字符串）。
    pub pub_date: Option<String>,
    /// 安装包下载地址。
    pub download_url: String,
    /// minisign 签名。
    pub signature: Option<String>,
}

/// 更新器状态。
///
/// 这是一个「粗粒度」的生命周期状态；下载进度通过
/// [`AutoUpdater::download_progress`] 与 [`AutoUpdateEvent::Progress`] 单独获取，
/// 因此渲染层不需要为了进度而去比较状态。
#[derive(Clone, Debug)]
pub enum AutoUpdateStatus {
    /// 尚未检查，或已被重置。
    Idle,
    /// 正在获取版本清单。
    Checking,
    /// 已是最新版本。
    UpToDate { current: Version },
    /// 发现新版本，等待下载。
    UpdateAvailable { version: Version },
    /// 正在下载安装包。
    Downloading { version: Version },
    /// 安装包已下载并通过校验，等待安装。
    Downloaded { version: Version },
    /// 安装已启动或已安排在退出时执行。
    Installing { version: Version },
    /// 出错。
    Failed { error: Arc<AutoUpdateError> },
}

impl PartialEq for AutoUpdateStatus {
    /// `Failed` 按错误文本比较，其余按变体 + 版本比较。
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Idle, Self::Idle) | (Self::Checking, Self::Checking) => true,
            (Self::UpToDate { current: a }, Self::UpToDate { current: b }) => a == b,
            (Self::UpdateAvailable { version: a }, Self::UpdateAvailable { version: b }) => a == b,
            (Self::Downloading { version: a }, Self::Downloading { version: b }) => a == b,
            (Self::Downloaded { version: a }, Self::Downloaded { version: b }) => a == b,
            (Self::Installing { version: a }, Self::Installing { version: b }) => a == b,
            (Self::Failed { error: a }, Self::Failed { error: b }) => a.to_string() == b.to_string(),
            _ => false,
        }
    }
}

impl AutoUpdateStatus {
    /// 是否已经是最新版本。
    pub fn is_up_to_date(&self) -> bool {
        matches!(self, Self::UpToDate { .. })
    }

    /// 是否有可用更新（尚未开始下载）。
    pub fn is_update_available(&self) -> bool {
        matches!(self, Self::UpdateAvailable { .. })
    }

    /// 是否正在检查、下载，或安装已安排。
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing { .. }
        )
    }
}

/// 更新器发出的事件。
///
/// ```ignore
/// cx.subscribe(&updater, |this, _updater, event: &AutoUpdateEvent, cx| {
///     if let AutoUpdateEvent::StatusChanged(status) = event {
///         // ...
///     }
///     cx.notify();
/// })
/// .detach();
/// ```
#[derive(Clone, Debug, PartialEq)]
pub enum AutoUpdateEvent {
    /// 状态发生变化。
    StatusChanged(AutoUpdateStatus),
    /// 下载进度更新（已节流到 1% 或 100ms）。
    Progress(DownloadProgress),
}

/// 让更新器在全应用范围内可见。
///
/// [`AutoUpdater::init`] 会自动注册；也可以通过 [`AutoUpdater::global`] 取回。
pub struct GlobalAutoUpdater(pub Entity<AutoUpdater>);

impl Global for GlobalAutoUpdater {}

#[derive(Clone, Debug)]
struct DownloadedInstaller {
    path: PathBuf,
    version: Version,
}

/// 更新器实体。
///
/// 状态变化通过 [`AutoUpdateEvent`] 广播，也可以用 [`AutoUpdater::status`]
/// 直接读取（并 `cx.observe` 更新器以在变化时重绘）。
pub struct AutoUpdater {
    config: AutoUpdateConfig,
    http: ureq::Agent,
    status: AutoUpdateStatus,
    available: Option<AvailableUpdate>,
    progress: Option<DownloadProgress>,
    installer: Option<DownloadedInstaller>,
    cancel: Option<Arc<AtomicBool>>,
    /// 等待退出时执行的安装。放在 `Arc<Mutex<_>>` 里，这样 `on_app_quit`
    /// 回调在实体被丢弃之后仍然能拿到数据。
    pending_install: Arc<Mutex<Option<DownloadedInstaller>>>,
}

impl AutoUpdater {
    /// 创建更新器。
    ///
    /// 会根据 [`AutoUpdateConfig::check_on_start`] 决定是否立即检查更新，
    /// 并注册「退出时安装」的 App 级回调。
    ///
    /// 拿到的 `Entity` 需要应用自己持有；想省事可以直接用
    /// [`AutoUpdater::init`]。
    pub fn new(cx: &mut App, config: AutoUpdateConfig) -> Entity<Self> {
        let http = config.build_agent();
        let installer_config = config.installer.clone();
        let pending_install: Arc<Mutex<Option<DownloadedInstaller>>> = Arc::new(Mutex::new(None));

        if config.public_key.is_none() {
            warn!(
                "auto_update: no minisign public key configured; downloaded installers will not be verified"
            );
        }

        // 注册在 App 上（而不是实体上）并在实体被丢弃后仍然有效：
        // 关闭最后一个窗口时 AutoUpdater 实体会先于 App::shutdown 被释放。
        {
            let pending_install = pending_install.clone();
            cx.on_app_quit(move |_cx| {
                let pending = pending_install.lock().unwrap().take();
                if let Some(pending) = pending {
                    info!(
                        "installing update {} from {:?}",
                        pending.version, pending.path
                    );
                    if let Err(error) =
                        install::execute(&installer_config, &pending.path, &pending.version)
                    {
                        warn!("failed to start update installer: {error}");
                    }
                }
                // 同步部分已经执行完，异步部分留空。
                async {}
            })
            .detach();
        }

        let check_on_start = config.check_on_start;
        cx.new(move |cx| {
            let mut this = Self {
                config,
                http,
                status: AutoUpdateStatus::Idle,
                available: None,
                progress: None,
                installer: None,
                cancel: None,
                pending_install,
            };
            if check_on_start && let Err(error) = this.check_for_update(cx) {
                warn!("auto_update: initial check failed: {error}");
            }
            this
        })
    }

    /// 创建更新器并注册为全局单例。
    ///
    /// 之后任意位置都可以用 [`AutoUpdater::global`] 取回它。
    pub fn init(cx: &mut App, config: AutoUpdateConfig) -> Entity<Self> {
        let updater = Self::new(cx, config);
        cx.set_global(GlobalAutoUpdater(updater.clone()));
        updater
    }

    /// 取回通过 [`AutoUpdater::init`] 注册的全局更新器。
    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAutoUpdater>()
            .map(|global| global.0.clone())
    }

    /// 当前状态。
    pub fn status(&self) -> &AutoUpdateStatus {
        &self.status
    }

    /// 当前应用版本。
    pub fn current_version(&self) -> &Version {
        &self.config.current_version
    }

    /// 可用更新（发现新版本后到下载完成前都有效）。
    pub fn available_update(&self) -> Option<&AvailableUpdate> {
        self.available.as_ref()
    }

    /// 下载进度；非下载状态为 `None`。
    pub fn download_progress(&self) -> Option<DownloadProgress> {
        self.progress
    }

    /// 已经下载并通过校验的安装包路径。
    pub fn installer_path(&self) -> Option<&Path> {
        self.installer.as_ref().map(|installer| installer.path.as_path())
    }

    /// 配置。
    pub fn config(&self) -> &AutoUpdateConfig {
        &self.config
    }

    /// 是否正在检查或下载。
    pub fn is_busy(&self) -> bool {
        self.status.is_busy()
    }

    /// 检查更新。
    ///
    /// 立即返回；结果通过 [`AutoUpdateStatus`] 与 [`AutoUpdateEvent`] 通知。
    /// 只有在「已有任务在跑」这类同步前置错误时才返回 `Err`。
    pub fn check_for_update(&mut self, cx: &mut Context<Self>) -> Result<(), AutoUpdateError> {
        if self.is_busy() {
            return Err(AutoUpdateError::Busy);
        }

        self.available = None;
        self.progress = None;
        self.set_status(AutoUpdateStatus::Checking, cx);

        let client = self.http.clone();
        let url = self.config.manifest_url.clone();
        let platform = self.config.platform.clone();
        let current = self.config.current_version.clone();
        let retries = self.config.retries;

        cx.spawn(async move |this, cx| {
            let task_current = current.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    fetch_available_update(&client, &url, &platform, &task_current, retries)
                })
                .await;

            this.update(cx, |this, cx| match result {
                Ok(Some(update)) => {
                    info!(
                        "update available: {} (current: {})",
                        update.version, this.config.current_version
                    );
                    let version = update.version.clone();
                    // 同一版本如果已经下载好了，就不要让用户再下一遍。
                    let reusable = this
                        .installer
                        .as_ref()
                        .is_some_and(|installer| installer.version == version && installer.path.exists());
                    if !reusable {
                        this.discard_installer();
                    }
                    this.available = Some(update);
                    let status = if this.installer.is_some() {
                        AutoUpdateStatus::Downloaded { version }
                    } else {
                        AutoUpdateStatus::UpdateAvailable { version }
                    };
                    this.set_status(status, cx);
                }
                Ok(None) => {
                    info!("already up to date: {}", this.config.current_version);
                    this.set_status(AutoUpdateStatus::UpToDate { current }, cx);
                }
                Err(error) => this.fail(error, cx),
            })
            .ok();
        })
        .detach();

        Ok(())
    }

    /// 下载可用更新的安装包，并在完成后校验签名。
    ///
    /// 同步前置错误（没有可用更新、已有任务在跑）直接返回 `Err`；
    /// 网络与校验错误通过 `Failed` 状态通知。
    pub fn download_update(&mut self, cx: &mut Context<Self>) -> Result<(), AutoUpdateError> {
        let Some(update) = self.available.clone() else {
            return Err(AutoUpdateError::NoUpdateAvailable);
        };
        if self.is_busy() {
            return Err(AutoUpdateError::Busy);
        }

        let file_name = download::file_name_for(&update.download_url, &update.version);
        let dir = self.config.download_dir();
        std::fs::create_dir_all(&dir)?;

        let request = DownloadRequest {
            client: self.http.clone(),
            url: update.download_url.clone(),
            part_path: dir.join(format!("{file_name}.part")),
            final_path: dir.join(&file_name),
            public_key: self.config.public_key.clone(),
            signature: update.signature.clone(),
        };

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.progress = Some(DownloadProgress::default());
        let version = update.version.clone();
        self.set_status(
            AutoUpdateStatus::Downloading {
                version: version.clone(),
            },
            cx,
        );

        cx.spawn(async move |this, cx| {
            let (sender, receiver) = smol::channel::unbounded::<DownloadProgress>();
            let executor = cx.background_executor().clone();
            let task = {
                let cancel = cancel.clone();
                let sender = sender.clone();
                executor
                    .spawn(async move { download::download_and_verify(&request, &sender, &cancel) })
            };
            // 只保留后台任务里的那一个 sender，任务结束后通道自动关闭。
            drop(sender);

            while let Ok(progress) = receiver.recv().await {
                this.update(cx, |this, cx| {
                    this.progress = Some(progress);
                    cx.emit(AutoUpdateEvent::Progress(progress));
                    cx.notify();
                })
                .ok();
            }

            let result = task.await;
            this.update(cx, |this, cx| this.finish_download(result, version, cx))
                .ok();
        })
        .detach();

        Ok(())
    }

    /// 安装已下载的更新。
    ///
    /// 具体行为取决于 [`InstallerConfig`]：
    ///
    /// * [`InstallerConfig::Manual`]：返回 [`AutoUpdateError::ManualInstallRequired`]，
    ///   请用 [`AutoUpdater::installer_path`] 自行处理。
    /// * 立即执行：启动安装程序后返回。
    /// * 退出时执行（默认）：登记安装，应用退出时自动运行安装程序。
    ///
    /// [`InstallerConfig`]: crate::InstallerConfig
    /// [`InstallerConfig::Manual`]: crate::InstallerConfig::Manual
    pub fn install_update(&mut self, cx: &mut Context<Self>) -> Result<(), AutoUpdateError> {
        let Some(installer) = self.installer.clone() else {
            return Err(AutoUpdateError::NoInstaller);
        };
        if !installer.path.exists() {
            self.installer = None;
            return Err(AutoUpdateError::InstallerNotFound(installer.path));
        }

        match self.config.installer.timing() {
            None => Err(AutoUpdateError::ManualInstallRequired),
            Some(InstallTiming::Immediately) => {
                install::execute(&self.config.installer, &installer.path, &installer.version)?;
                self.set_status(
                    AutoUpdateStatus::Installing {
                        version: installer.version,
                    },
                    cx,
                );
                Ok(())
            }
            Some(InstallTiming::OnQuit) => {
                let version = installer.version.clone();
                *self.pending_install.lock().unwrap() = Some(installer);
                self.set_status(AutoUpdateStatus::Installing { version }, cx);
                Ok(())
            }
        }
    }

    /// 取消正在进行的下载。
    ///
    /// 状态会立刻回到 `UpdateAvailable`，后台任务在下一个检查点退出并删除
    /// 半成品文件。
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(flag) = &self.cancel {
            flag.store(true, Ordering::Relaxed);
        }
        if let AutoUpdateStatus::Downloading { version } = self.status.clone() {
            self.progress = None;
            self.set_status(AutoUpdateStatus::UpdateAvailable { version }, cx);
        }
    }

    /// 清空所有状态回到 `Idle`，并删除已下载的安装包。
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.available = None;
        *self.pending_install.lock().unwrap() = None;
        self.discard_installer();
        self.progress = None;
        self.cancel = None;
        self.set_status(AutoUpdateStatus::Idle, cx);
    }

    /// 丢掉落盘的安装包并尽量删除对应文件。
    fn discard_installer(&mut self) {
        if let Some(installer) = self.installer.take() {
            let _ = std::fs::remove_file(&installer.path);
        }
    }

    fn finish_download(
        &mut self,
        result: Result<PathBuf, AutoUpdateError>,
        version: Version,
        cx: &mut Context<Self>,
    ) {
        self.cancel = None;
        self.progress = None;

        match result {
            Ok(path) => {
                info!("update {} downloaded to {:?}", version, path);
                self.installer = Some(DownloadedInstaller {
                    path,
                    version: version.clone(),
                });
                self.set_status(
                    AutoUpdateStatus::Downloaded {
                        version: version.clone(),
                    },
                    cx,
                );

                if self.config.installer.timing() == Some(InstallTiming::Immediately)
                    && let Err(error) = self.install_update(cx)
                {
                    self.fail(error, cx);
                }
            }
            Err(AutoUpdateError::Cancelled) => {
                self.set_status(AutoUpdateStatus::UpdateAvailable { version }, cx);
            }
            Err(error) => self.fail(error, cx),
        }
    }

    fn fail(&mut self, error: AutoUpdateError, cx: &mut Context<Self>) {
        warn!("update failed: {error}");
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

fn fetch_available_update(
    client: &ureq::Agent,
    url: &str,
    platform: &str,
    current: &Version,
    retries: u32,
) -> Result<Option<AvailableUpdate>, AutoUpdateError> {
    let manifest = fetch_manifest(client, url, retries)?;
    let version = manifest.semver()?;
    if &version <= current {
        return Ok(None);
    }

    let asset = manifest
        .asset_for(platform)
        .ok_or_else(|| AutoUpdateError::PlatformUnavailable {
            platform: platform.to_string(),
            available: manifest.available_platforms(),
        })?;

    Ok(Some(AvailableUpdate {
        version,
        notes: manifest.notes.clone(),
        pub_date: manifest.pub_date.clone(),
        download_url: asset.url.clone(),
        signature: asset.signature.clone(),
    }))
}

fn fetch_manifest(
    client: &ureq::Agent,
    url: &str,
    retries: u32,
) -> Result<ReleaseManifest, AutoUpdateError> {
    let mut attempt = 0;
    loop {
        match try_fetch_manifest(client, url) {
            Ok(manifest) => return Ok(manifest),
            Err(error) if error.is_retryable() && attempt < retries => {
                attempt += 1;
                warn!(
                    "fetching update manifest failed (attempt {attempt}/{retries}): {error}"
                );
                std::thread::sleep(Duration::from_millis(500 * u64::from(attempt)));
            }
            Err(error) => return Err(error),
        }
    }
}

fn try_fetch_manifest(client: &ureq::Agent, url: &str) -> Result<ReleaseManifest, AutoUpdateError> {
    let response = client.get(url).call()?;
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(AutoUpdateError::Http {
            status,
            url: url.to_string(),
        });
    }
    Ok(serde_json::from_reader(response.into_reader())?)
}
