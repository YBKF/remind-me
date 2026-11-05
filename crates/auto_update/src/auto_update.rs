pub mod version;

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result};
use gpui_kit::{App, AppContext, Context, Entity, Global};
use minisign_verify::{PublicKey, Signature};
use semver::Version;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use utils::command::new_command;

use version::VersionJson;

#[derive(Debug, Clone)]
pub enum AutoUpdateStatus {
    Idle,
    Checking,
    Downloading {
        version: Version,
        /// Download progress as a fraction in the range `0.0..=1.0`, or `None`
        /// when the total download size is not yet known.
        progress: Option<f32>,
    },
    Installing {
        version: Version,
    },
    Updated {
        version: Version,
    },
    Errored {
        error: Arc<anyhow::Error>,
    },
}

impl PartialEq for AutoUpdateStatus {
    // `progress` is deliberately not compared: two `Downloading` statuses for
    // the same version are equal regardless of how far the download is.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AutoUpdateStatus::Idle, AutoUpdateStatus::Idle) => true,
            (AutoUpdateStatus::Checking, AutoUpdateStatus::Checking) => true,
            (
                AutoUpdateStatus::Downloading { version: v1, .. },
                AutoUpdateStatus::Downloading { version: v2, .. },
            ) => v1 == v2,
            (
                AutoUpdateStatus::Installing { version: v1 },
                AutoUpdateStatus::Installing { version: v2 },
            ) => v1 == v2,
            (
                AutoUpdateStatus::Updated { version: v1 },
                AutoUpdateStatus::Updated { version: v2 },
            ) => v1 == v2,
            (AutoUpdateStatus::Errored { error: e1 }, AutoUpdateStatus::Errored { error: e2 }) => {
                e1.to_string() == e2.to_string()
            }
            _ => false,
        }
    }
}

impl AutoUpdateStatus {
    pub fn is_updated(&self) -> bool {
        matches!(self, Self::Updated { .. })
    }
}

#[allow(dead_code)]
pub struct AutoUpdater {
    status: AutoUpdateStatus,
    current_version: Version,
    update_public_key: &'static str,
    version_json_url: &'static str,
    client: Arc<ureq::Agent>,
    /// Remote version manifest fetched during `check_for_update`.
    version_json: Option<VersionJson>,
    /// Path to a downloaded installer awaiting installation on quit.
    ///
    /// Stored behind an `Arc` so that the `on_app_quit` handler can access it
    /// even after the `AutoUpdater` entity itself has been dropped (the entity
    /// is released when the window closes, before `on_app_quit` runs).
    need_install: Arc<std::sync::Mutex<Option<String>>>,
}

/// Creates the `AutoUpdater` entity and registers an app-level `on_app_quit`
/// handler that installs any pending update when the application quits.
///
/// The `on_app_quit` handler MUST be registered on the `App` (not on the
/// `AutoUpdater` entity) for two reasons:
///
/// 1. The `AutoUpdater` entity is dropped when the last window closes — before
///    `App::shutdown` runs. A subscription stored on the entity (and not
///    `.detach()`-ed) is cancelled on drop, removing the handler from the
///    `quit_observers` set.
/// 2. `Context::on_app_quit` internally upgrades a `WeakEntity`; once the entity
///    is gone the upgrade fails and the callback is silently skipped.
///
/// Registering on `App` with `.detach()` keeps the handler alive for the whole
/// application lifetime, independent of the entity.
pub fn register_auto_updater(
    current_version: Version,
    update_public_key: &'static str,
    version_json_url: &'static str,
    client: Arc<ureq::Agent>,
    cx: &mut App,
) -> Entity<AutoUpdater> {
    let need_install = Arc::new(std::sync::Mutex::new(None::<String>));

    // Windows 平台无法覆盖正在运行的程序，只能在应用退出或重启时覆盖。
    #[cfg(target_os = "windows")]
    {
        let need_install = need_install.clone();
        cx.on_app_quit(move |_cx| {
            let need_install = need_install.lock().unwrap().clone();
            info!("quit, need_install={:?}", need_install);
            // Run synchronously here: the synchronous part of the
            // `on_app_quit` callback executes directly inside `App::shutdown`
            // and is NOT subject to the 200ms `SHUTDOWN_TIMEOUT` that applies
            // to the returned async future. `finalize_auto_update_on_quit`
            // only spawns the helper (it no longer waits on it), so this
            // blocks for just a few milliseconds.
            finalize_auto_update_on_quit(need_install);
            async move {}
        })
        .detach();
    }

    cx.new(move |cx| {
        let mut updater = AutoUpdater::new_with_need_install(
            current_version,
            update_public_key,
            version_json_url,
            client,
            need_install,
            cx,
        );
        updater.check_for_update(cx);
        updater
    })
}

#[allow(dead_code)]
#[derive(Default)]
struct GlobalAutoUpdate(Option<Entity<AutoUpdater>>);

impl Global for GlobalAutoUpdate {}

impl AutoUpdater {
    #[allow(dead_code)]
    fn new_with_need_install(
        current_version: Version,
        update_public_key: &'static str,
        version_json_url: &'static str,
        client: Arc<ureq::Agent>,
        need_install: Arc<std::sync::Mutex<Option<String>>>,
        _cx: &mut Context<Self>,
    ) -> Self {
        cleanup_old_folder();

        Self {
            status: AutoUpdateStatus::Idle,
            current_version,
            update_public_key,
            version_json_url,
            client,
            version_json: None,
            need_install,
        }
    }

    pub fn status(&self) -> AutoUpdateStatus {
        self.status.clone()
    }

    pub fn current_version(&self) -> Version {
        self.current_version.clone()
    }

    /// Whether a new version is available (manifest fetched and newer than current).
    pub fn has_update_available(&self) -> bool {
        self.version_json.is_some()
    }

    /// Spawn a background task that fetches the version manifest, compares
    /// versions, and stores the manifest if an update is available.
    pub fn check_for_update(&mut self, cx: &mut Context<Self>) {
        self.status = AutoUpdateStatus::Checking;
        cx.notify();

        let client = self.client.clone();
        let version_json_url = self.version_json_url;
        cx.spawn(async move |this, cx| {
            // `ureq` is a blocking HTTP client, so run the request on smol's
            // blocking thread pool and await its completion here.
            let result = smol::unblock(move || -> Result<VersionJson> {
                let response = client
                    .get(version_json_url)
                    .call()
                    .context("Failed to send version check request")?;
                let status = response.status();
                anyhow::ensure!(
                    (200..300).contains(&status),
                    "version check HTTP error: {}",
                    status
                );
                let version_json: VersionJson = response
                    .into_json()
                    .context("Failed to deserialize VersionJson")?;
                Ok(version_json)
            })
            .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(version_json) => match version_json.get_semver() {
                        Ok(remote_version) => {
                            if remote_version > this.current_version {
                                info!(
                                    "Update available: {} (current: {})",
                                    remote_version, this.current_version
                                );
                                this.version_json = Some(version_json);
                                this.status = AutoUpdateStatus::Idle;
                            } else {
                                info!(
                                    "Already up to date: {} (current: {})",
                                    remote_version, this.current_version
                                );
                                this.version_json = None;
                                this.status = AutoUpdateStatus::Idle;
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse remote semver: {}", e);
                            this.version_json = None;
                            this.status = AutoUpdateStatus::Errored {
                                error: Arc::new(e.into()),
                            };
                        }
                    },
                    Err(e) => {
                        warn!("Update check failed: {:#}", e);
                        this.version_json = None;
                        this.status = AutoUpdateStatus::Errored { error: Arc::new(e) };
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Start downloading the installer for the available update.
    ///
    /// Downloads the installer to the `Updates` folder next to the current
    /// executable, verifies its minisign signature, and stores the path in
    /// `need_install` for installation on quit.
    pub fn start_download(&mut self, cx: &mut Context<Self>) {
        let Some(version_json) = &self.version_json else {
            return;
        };

        let download_url = version_json.download_url().to_string();
        let signature = version_json.signature().to_string();
        let remote_version = match version_json.get_semver() {
            Ok(v) => v,
            Err(e) => {
                warn!("Cannot start download, bad semver: {}", e);
                return;
            }
        };

        // Determine the target directory and file name.
        let Some(exe_parent) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
        else {
            warn!("Cannot determine executable parent directory");
            return;
        };

        let Some(filename) = extract_filename_from_url(&download_url) else {
            warn!(
                "Cannot extract a valid filename (with extension) from URL: {}",
                download_url
            );
            return;
        };

        let updates_dir = exe_parent.join("Updates");
        let target_path = updates_dir.join(&filename);

        // Switch to Downloading status.
        self.status = AutoUpdateStatus::Downloading {
            version: remote_version.clone(),
            progress: None,
        };
        cx.notify();

        let client = self.client.clone();

        let update_public_key = self.update_public_key;
        cx.spawn(async move |this, cx| {
            // `AsyncApp` is not `Send`, so we can't move `cx` into the blocking
            // closure. Instead, the blocking download runs on smol's thread pool
            // and reports progress/results back to GPUI's foreground executor
            // over a channel, where `this.update` runs with `cx`.
            let (event_tx, event_rx) = smol::channel::unbounded::<DownloadEvent>();

            let download_task = smol::unblock(move || {
                // Ensure the Updates folder exists.
                if let Err(e) = std::fs::create_dir_all(&updates_dir) {
                    let _ = event_tx.try_send(DownloadEvent::Failed(anyhow::anyhow!(
                        "Failed to create Updates folder: {}",
                        e
                    )));
                    return;
                }

                info!("Downloading from {}", download_url);

                // Stream-download the installer, forwarding progress events.
                let download_result = download_release(&target_path, &download_url, &client, {
                    let event_tx = event_tx.clone();
                    move |progress| {
                        let _ = event_tx.try_send(DownloadEvent::Progress(progress));
                    }
                });

                if let Err(e) = download_result {
                    // Clean up partial download.
                    let _ = std::fs::remove_file(&target_path);
                    let _ = event_tx.try_send(DownloadEvent::Failed(anyhow::anyhow!(
                        "Download failed: {}",
                        e
                    )));
                    return;
                }

                // Read the downloaded file for signature verification.
                let file_bytes = match std::fs::read(&target_path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        let _ = event_tx.try_send(DownloadEvent::Failed(anyhow::anyhow!(
                            "Failed to read downloaded file: {}",
                            e
                        )));
                        return;
                    }
                };

                // Verify the minisign signature.
                if let Err(e) = verify_signature(&file_bytes, update_public_key, &signature) {
                    let _ = std::fs::remove_file(&target_path);
                    let _ = event_tx.try_send(DownloadEvent::Failed(anyhow::anyhow!(
                        "Signature verification failed: {}",
                        e
                    )));
                    return;
                }

                info!(
                    "Update verified and ready to install. path: {:?}",
                    target_path
                );
                let _ = event_tx.try_send(DownloadEvent::Finished(
                    target_path.to_string_lossy().to_string(),
                ));
            });

            // Consume events on GPUI's foreground executor and update state.
            while let Ok(event) = event_rx.recv().await {
                match event {
                    DownloadEvent::Progress(progress) => {
                        this.update(cx, |this, cx| {
                            this.status = AutoUpdateStatus::Downloading {
                                version: remote_version.clone(),
                                progress,
                            };
                            cx.notify();
                        })
                        .ok();
                    }
                    DownloadEvent::Failed(error) => {
                        this.update(cx, |this, cx| {
                            this.status = AutoUpdateStatus::Errored {
                                error: Arc::new(error),
                            };
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                    DownloadEvent::Finished(path) => {
                        this.update(cx, |this, cx| {
                            *this.need_install.lock().unwrap() = Some(path);
                            this.status = AutoUpdateStatus::Updated {
                                version: remote_version.clone(),
                            };
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                }
            }

            // Wait for the blocking task to finish (channel closes when all
            // senders drop).
            download_task.await;
        })
        .detach();
    }
}

/// Events produced by the blocking download task and consumed on GPUI's
/// foreground executor to drive UI state updates.
enum DownloadEvent {
    /// Download progress as a fraction `0.0..=1.0`.
    Progress(Option<f32>),
    /// A fatal error occurred during download/verification.
    Failed(anyhow::Error),
    /// Download finished and the installer was verified; carries the path.
    Finished(String),
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ReleaseAsset {
    pub version: String,
    pub url: String,
}

/// Extract the file name (last path segment) from a download URL.
/// Returns `None` if no usable file name with an extension can be extracted.
fn extract_filename_from_url(url: &str) -> Option<String> {
    // Strip query string and fragment
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last_segment = path.rsplit('/').next()?;
    if last_segment.is_empty() {
        return None;
    }
    // Must contain at least one dot (i.e. have an extension)
    if !last_segment.contains('.') {
        return None;
    }
    Some(last_segment.to_string())
}

/// Verify a minisign signature over the given data using the bundled public key.
fn verify_signature(data: &[u8], update_public_key: &str, signature_str: &str) -> Result<()> {
    let public_key =
        PublicKey::from_base64(update_public_key).context("Failed to parse public key")?;
    let signature =
        Signature::decode(signature_str).context("Failed to decode minisign signature")?;
    public_key
        .verify(data, &signature, true)
        .context("Signature verification failed")
}

/// Stream-download `url` to `target_path`, invoking `on_progress` with the
/// download fraction (0.0..=1.0) as data arrives.
///
/// Blocking: uses `ureq` (which reads the body via `std::io::Read`) and writes
/// to a `std::fs::File`. Intended to be called on a background thread.
fn download_release(
    target_path: &Path,
    url: &str,
    client: &ureq::Agent,
    mut on_progress: impl FnMut(Option<f32>),
) -> anyhow::Result<()> {
    let response = client
        .get(url)
        .call()
        .context("Failed to send download request")?;
    let status = response.status();
    anyhow::ensure!(
        (200..300).contains(&status),
        "failed to download update: {}",
        status
    );

    let total_bytes = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|total_bytes| *total_bytes > 0);

    let mut target_file = std::fs::File::create(target_path)?;
    let mut reader = response.into_reader();

    let mut downloaded_bytes = 0u64;
    let mut last_reported_percent: Option<u8> = None;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buf)
            .context("Error reading download stream")?;
        if read == 0 {
            break;
        }
        target_file.write_all(&buf[..read])?;
        downloaded_bytes += read as u64;

        if let Some(total_bytes) = total_bytes {
            let fraction = (downloaded_bytes as f32 / total_bytes as f32).clamp(0.0, 1.0);
            let percent = (fraction * 100.0) as u8;
            if Some(percent) != last_reported_percent {
                last_reported_percent = Some(percent);
                on_progress(Some(fraction));
            }
        }
    }
    target_file.flush()?;
    if total_bytes.is_some() && last_reported_percent != Some(100) {
        on_progress(Some(1.0));
    }
    info!("downloaded update. path:{:?}", target_path);

    Ok(())
}

/// Best-effort removal of a leftover `old` folder next to the current executable.
/// Failures are logged and ignored.
fn cleanup_old_folder() {
    let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        return;
    };

    let old_dir = parent.join("old");
    if old_dir.exists() {
        match std::fs::remove_dir_all(&old_dir) {
            Ok(()) => info!("Cleaned up old folder: {:?}", old_dir),
            Err(e) => {
                warn!("Failed to remove old folder (ignored): {}", e);
            }
        }
    }
}

/// Locate the `auto_update_helper` executable.
/// Checks `tools/auto_update_helper.exe` first, then falls back to the same
/// directory as the current executable.
fn find_helper_exe() -> Option<PathBuf> {
    let parent = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))?;

    let candidate = parent.join("tools").join("auto_update_helper.exe");
    if candidate.exists() {
        return Some(candidate);
    }

    let fallback = parent.join("auto_update_helper.exe");
    if fallback.exists() {
        return Some(fallback);
    }

    None
}

/// Called on app quit. If `need_install` points to a downloaded installer,
/// copies the helper into an `old` folder and launches it to install the
/// update. The helper deletes the installer after a successful install.
///
/// Blocking: performs synchronous filesystem and process work. Intended to be
/// called on a background thread (see `AutoUpdater::new`).
pub fn finalize_auto_update_on_quit(need_install: Option<String>) {
    info!(
        "finalize_auto_update_on_quit called, need_install={:?}",
        need_install
    );
    let Some(installer_path_str) = need_install else {
        info!("finalize: no installer to install, returning");
        return;
    };

    let installer_path = PathBuf::from(&installer_path_str);
    if !installer_path.exists() {
        warn!(
            "Skipping update on quit: installer not found at {:?}",
            installer_path
        );
        return;
    }

    let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        warn!("Cannot determine executable parent directory");
        return;
    };

    let old_dir = parent.join("old");
    if let Err(e) = std::fs::create_dir_all(&old_dir) {
        warn!("Failed to create old folder: {}", e);
        return;
    }

    let Some(helper_src) = find_helper_exe() else {
        warn!("auto_update_helper not found; skipping update");
        return;
    };
    info!("finalize: helper found at {:?}", helper_src);

    let helper_dst = old_dir.join("auto_update_helper.exe");
    if let Err(e) = std::fs::copy(&helper_src, &helper_dst) {
        warn!("Failed to copy helper to old folder: {}", e);
        return;
    }
    info!("finalize: helper copied to {:?}", helper_dst);

    let mut command = new_command(&helper_dst);
    command
        .arg("--installer-path")
        .arg(&installer_path_str)
        .arg("--auto-remove");

    match command.spawn() {
        // Do NOT wait for the helper: it runs the installer (which may take
        // seconds) and cleans up afterwards. Blocking here would exceed GPUI's
        // `SHUTDOWN_TIMEOUT` and the process would exit before the helper is
        // even spawned. Dropping the child detaches it so it keeps running.
        Ok(_child) => {
            info!("Launched helper from old folder for update installation");
        }
        Err(e) => {
            warn!("Failed to spawn helper: {}", e);
        }
    }
}
