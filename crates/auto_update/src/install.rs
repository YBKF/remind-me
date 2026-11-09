//! 安装策略：下载完成之后如何把更新装到系统上。
//!
//! 库本身不假设安装包格式。常见形态通过 [`InstallerConfig`] 的构造函数
//! 覆盖（Inno Setup / NSIS / MSI / 自带 helper），特殊流程用
//! [`InstallerConfig::custom`] 自行实现。

use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use semver::Version;
use tracing::{info, warn};

use crate::AutoUpdateError;

/// 什么时候执行安装。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallTiming {
    /// 应用退出时执行（默认）。
    ///
    /// Windows 上安装包无法覆盖正在运行的可执行文件，因此这是最稳妥的时机。
    #[default]
    OnQuit,
    /// 下载完成（或调用 `install_update`）后立刻执行。
    Immediately,
}

/// 要运行的程序。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallerProgram {
    /// 运行刚刚下载的安装包本身。
    Downloaded,
    /// 运行指定的外部程序，例如随应用分发的更新助手。
    ///
    /// 相对路径会以当前可执行文件所在目录为基准解析。
    Path(PathBuf),
}

/// 自定义安装钩子。
///
/// 适用于库没有内置支持的场景（例如直接替换二进制、调用系统包管理器、
/// 弹出自定义 UI 等）。
pub trait InstallHandler: Send + Sync + 'static {
    /// 执行安装。返回 `Ok(())` 表示已成功交给安装程序（不必等待其结束）。
    fn install(&self, request: &InstallRequest) -> Result<(), AutoUpdateError>;
}

/// 传给 [`InstallHandler::install`] 的上下文。
#[derive(Clone, Debug)]
pub struct InstallRequest {
    /// 已下载并通过校验的安装包路径。
    pub installer_path: PathBuf,
    /// 该安装包对应的版本。
    pub version: Version,
}

/// 下载完成后的安装策略。
#[derive(Clone, Default)]
pub enum InstallerConfig {
    /// 只下载，不自动安装（默认）。
    ///
    /// 下载完成后 [`AutoUpdater::installer_path`] 可拿到路径，由应用自行处理。
    ///
    /// [`AutoUpdater::installer_path`]: crate::AutoUpdater::installer_path
    #[default]
    Manual,
    /// 运行一个外部程序完成安装。
    Command {
        /// 运行哪个程序。
        program: InstallerProgram,
        /// 命令行参数，`{installer}` 会替换为安装包路径。
        args: Vec<String>,
        /// 执行时机。
        timing: InstallTiming,
        /// 先把程序复制到该目录再运行。
        ///
        /// 用于「安装过程会清空应用目录，但助手程序还需要继续运行」的场景。
        relocate_to: Option<PathBuf>,
        /// 是否隐藏控制台窗口（仅 Windows 生效）。
        hide_console: bool,
    },
    /// 完全由应用自己实现安装逻辑。
    Custom(Arc<dyn InstallHandler>),
}

impl fmt::Debug for InstallerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manual => f.write_str("Manual"),
            Self::Command {
                program,
                args,
                timing,
                relocate_to,
                hide_console,
            } => f
                .debug_struct("Command")
                .field("program", program)
                .field("args", args)
                .field("timing", timing)
                .field("relocate_to", relocate_to)
                .field("hide_console", hide_console)
                .finish(),
            Self::Custom(_) => f.write_str("Custom(..)"),
        }
    }
}

impl InstallerConfig {
    /// 运行下载好的安装包。
    pub fn run_downloaded<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Command {
            program: InstallerProgram::Downloaded,
            args: args.into_iter().map(Into::into).collect(),
            timing: InstallTiming::OnQuit,
            relocate_to: None,
            hide_console: true,
        }
    }

    /// 运行外部程序（例如更新助手），`{installer}` 会被替换为安装包路径。
    pub fn run_helper<I, S>(program: impl Into<PathBuf>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Command {
            program: InstallerProgram::Path(program.into()),
            args: args.into_iter().map(Into::into).collect(),
            timing: InstallTiming::OnQuit,
            relocate_to: None,
            hide_console: true,
        }
    }

    /// Inno Setup 静默安装（当前用户）。
    pub fn inno_setup() -> Self {
        Self::run_downloaded(["/SILENT", "/CURRENTUSER"])
    }

    /// Inno Setup 静默安装（所有用户，需要管理员权限）。
    pub fn inno_setup_all_users() -> Self {
        Self::run_downloaded(["/SILENT", "/ALLUSERS"])
    }

    /// NSIS 静默安装。
    pub fn nsis() -> Self {
        Self::run_downloaded(["/S"])
    }

    /// MSI 静默安装。
    pub fn msi() -> Self {
        Self::run_downloaded(["/qn", "/norestart"])
    }

    /// 使用自定义安装钩子。
    pub fn custom(handler: impl InstallHandler) -> Self {
        Self::Custom(Arc::new(handler))
    }

    /// 修改执行时机。
    pub fn with_timing(mut self, timing: InstallTiming) -> Self {
        if let Self::Command { timing: slot, .. } = &mut self {
            *slot = timing;
        }
        self
    }

    /// 执行前把程序复制到指定目录。
    pub fn with_relocate_to(mut self, dir: impl Into<PathBuf>) -> Self {
        if let Self::Command { relocate_to, .. } = &mut self {
            *relocate_to = Some(dir.into());
        }
        self
    }

    /// 是否显示控制台窗口（默认隐藏）。
    pub fn with_console(mut self, show: bool) -> Self {
        if let Self::Command { hide_console, .. } = &mut self {
            *hide_console = !show;
        }
        self
    }

    /// 安装时机；[`InstallerConfig::Manual`] 返回 `None`。
    pub(crate) fn timing(&self) -> Option<InstallTiming> {
        match self {
            Self::Manual => None,
            Self::Command { timing, .. } => Some(*timing),
            Self::Custom(_) => Some(InstallTiming::Immediately),
        }
    }
}

/// 按配置执行安装。不会等待安装程序结束。
pub(crate) fn execute(
    config: &InstallerConfig,
    installer_path: &Path,
    version: &Version,
) -> Result<(), AutoUpdateError> {
    match config {
        InstallerConfig::Manual => Err(AutoUpdateError::ManualInstallRequired),
        InstallerConfig::Custom(handler) => handler.install(&InstallRequest {
            installer_path: installer_path.to_path_buf(),
            version: version.clone(),
        }),
        InstallerConfig::Command {
            program,
            args,
            relocate_to,
            hide_console,
            ..
        } => {
            if !installer_path.exists() {
                return Err(AutoUpdateError::InstallerNotFound(
                    installer_path.to_path_buf(),
                ));
            }

            let installer_arg = installer_path.display().to_string();
            let mut program_path = match program {
                InstallerProgram::Downloaded => installer_path.to_path_buf(),
                InstallerProgram::Path(path) => resolve_program(path),
            };

            if let Some(dir) = relocate_to {
                std::fs::create_dir_all(dir)?;
                let file_name = program_path
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("installer"));
                let destination = dir.join(file_name);
                std::fs::copy(&program_path, &destination)?;
                program_path = destination;
            }

            let mut command = Command::new(&program_path);
            for arg in args {
                command.arg(if arg.contains("{installer}") {
                    arg.replace("{installer}", &installer_arg)
                } else {
                    arg.clone()
                });
            }
            if *hide_console {
                hide_console_window(&mut command);
            }

            info!(
                "starting update installer {:?} for version {}",
                program_path, version
            );
            match command.spawn() {
                Ok(_child) => Ok(()),
                Err(error) => {
                    warn!("failed to spawn update installer: {error}");
                    Err(AutoUpdateError::Io(error))
                }
            }
        }
    }
}

/// 相对路径以当前可执行文件所在目录为基准解析。
fn resolve_program(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        Some(dir) => dir.join(path),
        None => path.to_path_buf(),
    }
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}
