//! 安装策略：下载完成之后如何把更新装到系统上。
//!
//! 库本身不假设安装包格式。常见形态通过 [`InstallerConfig`] 的构造函数
//! 覆盖（Inno Setup / NSIS / MSI / 自带 helper），特殊流程用
//! [`InstallerConfig::custom`] 自行实现。
//!
//! # 全局安装 / 用户安装
//!
//! 同一个安装包通常既能装到 `Program Files`（全局，需要管理员权限），也能
//! 装到 `%LOCALAPPDATA%\Programs`（当前用户）。更新时必须沿用原来的安装范围，
//! 否则会出现「用户级安装被要求提权」或者「全局安装被装进用户目录」。
//!
//! [`InstallerConfig::inno_setup`] 等构造函数默认带上 [`ScopeArgs`]，执行安装前
//! 用 [`InstallScope::detect`] 判断当前程序的位置，再追加对应参数。

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

/// 安装范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallScope {
    /// 全局安装（所有用户），通常位于 `Program Files`，安装需要管理员权限。
    PerMachine,
    /// 当前用户安装，通常位于 `%LOCALAPPDATA%\Programs`，无需提权。
    PerUser,
}

impl InstallScope {
    /// 根据当前可执行文件的位置推断已有的安装范围。
    ///
    /// 依次尝试：
    ///
    /// 1. 与系统已知目录比对（`Program Files` / `%LOCALAPPDATA%\Programs` /
    ///    `/Applications` / `/usr` / `~/.local` …），命中即返回；
    /// 2. 兜底探测：能写入可执行文件所在目录说明是用户级安装，
    ///    否则按全局安装处理。
    ///
    /// 如果安装到自定义目录（例如 `D:\Apps\RemindMe`），推断只能靠第 2 步，
    /// 可能不准。这类场景请用明确范围的构造函数，或自行调用
    /// [`InstallerConfig::with_scope_args`] / 实现 [`InstallHandler`]。
    pub fn detect() -> Self {
        if let Ok(exe) = std::env::current_exe()
            && let Some(scope) = scope_from_roots(&exe, &known_roots())
        {
            return scope;
        }

        match std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            Some(dir) if dir_writable(&dir) => Self::PerUser,
            _ => Self::PerMachine,
        }
    }

    /// 是否为全局安装。
    pub fn is_per_machine(self) -> bool {
        matches!(self, Self::PerMachine)
    }
}

/// 安装范围对应的命令行参数。
///
/// 不同安装器的写法不一样，所以由调用方（或内置构造函数）提供：
/// Inno Setup 用 `/CURRENTUSER` / `/ALLUSERS`，MSI 用 `ALLUSERS=2` / `ALLUSERS=1`。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeArgs {
    /// 用户级安装时追加的参数。
    pub per_user: Vec<String>,
    /// 全局安装时追加的参数。
    pub per_machine: Vec<String>,
}

impl ScopeArgs {
    /// 创建参数映射。
    pub fn new<U, M, S>(per_user: U, per_machine: M) -> Self
    where
        U: IntoIterator<Item = S>,
        M: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            per_user: per_user.into_iter().map(Into::into).collect(),
            per_machine: per_machine.into_iter().map(Into::into).collect(),
        }
    }

    /// 取某个安装范围对应的参数。
    pub fn for_scope(&self, scope: InstallScope) -> &[String] {
        match scope {
            InstallScope::PerUser => &self.per_user,
            InstallScope::PerMachine => &self.per_machine,
        }
    }

    /// 两个范围都没有参数。
    pub fn is_empty(&self) -> bool {
        self.per_user.is_empty() && self.per_machine.is_empty()
    }
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
    /// 推断出的当前安装范围，供自定义实现决定如何安装。
    pub scope: InstallScope,
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
        /// 执行前根据当前安装位置追加的参数。
        scope_args: ScopeArgs,
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
                scope_args,
                timing,
                relocate_to,
                hide_console,
            } => f
                .debug_struct("Command")
                .field("program", program)
                .field("args", args)
                .field("scope_args", scope_args)
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
            scope_args: ScopeArgs::default(),
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
            scope_args: ScopeArgs::default(),
            timing: InstallTiming::OnQuit,
            relocate_to: None,
            hide_console: true,
        }
    }

    /// Inno Setup 静默安装，**自动沿用当前安装范围**。
    ///
    /// 程序装在 `Program Files` 时传 `/ALLUSERS`（会触发 UAC）；
    /// 装在用户目录时传 `/CURRENTUSER`（不需要提权）。
    ///
    /// 需要安装脚本里设置
    /// `PrivilegesRequiredOverridesAllowed=commandline dialog`，
    /// 否则命令行参数会被忽略。
    pub fn inno_setup() -> Self {
        Self::run_downloaded(["/SILENT"]).with_scope_args(ScopeArgs::new(
            ["/CURRENTUSER"],
            ["/ALLUSERS"],
        ))
    }

    /// Inno Setup 静默安装（强制当前用户，不需要提权）。
    pub fn inno_setup_per_user() -> Self {
        Self::run_downloaded(["/SILENT", "/CURRENTUSER"])
    }

    /// Inno Setup 静默安装（强制所有用户，需要管理员权限）。
    pub fn inno_setup_all_users() -> Self {
        Self::run_downloaded(["/SILENT", "/ALLUSERS"])
    }

    /// NSIS 静默安装，自动沿用当前安装范围。
    pub fn nsis() -> Self {
        Self::run_downloaded(["/S"]).with_scope_args(ScopeArgs::new(
            Vec::<&str>::new(),
            ["/ALLUSERS"],
        ))
    }

    /// MSI 静默安装，自动沿用当前安装范围。
    pub fn msi() -> Self {
        Self::run_downloaded(["/qn", "/norestart"])
            .with_scope_args(ScopeArgs::new(["ALLUSERS=2"], ["ALLUSERS=1"]))
    }

    /// 使用自定义安装钩子。
    pub fn custom(handler: impl InstallHandler) -> Self {
        Self::Custom(Arc::new(handler))
    }

    /// 设置按安装范围追加的参数。
    pub fn with_scope_args(mut self, scope_args: ScopeArgs) -> Self {
        if let Self::Command {
            scope_args: slot, ..
        } = &mut self
        {
            *slot = scope_args;
        }
        self
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
            scope: InstallScope::detect(),
        }),
        InstallerConfig::Command {
            program,
            args,
            scope_args,
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

            // 全局安装 / 用户安装必须和当前安装保持一致，否则会出现
            // 「用户级安装被要求提权」或「全局安装被装进用户目录」。
            if !scope_args.is_empty() {
                let scope = InstallScope::detect();
                command.args(scope_args.for_scope(scope));
                info!("update install scope: {scope:?}");
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

/// 系统上「全局安装」与「用户级安装」的典型目录。
fn known_roots() -> Vec<(PathBuf, InstallScope)> {
    let mut roots = Vec::new();

    #[cfg(windows)]
    {
        for var in ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(var) {
                roots.push((PathBuf::from(dir), InstallScope::PerMachine));
            }
        }
        if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
            roots.push((
                PathBuf::from(dir).join("Programs"),
                InstallScope::PerUser,
            ));
        }
        if let Some(dir) = std::env::var_os("APPDATA") {
            roots.push((PathBuf::from(dir), InstallScope::PerUser));
        }
    }

    #[cfg(target_os = "macos")]
    {
        roots.push((PathBuf::from("/Applications"), InstallScope::PerMachine));
        if let Some(home) = std::env::var_os("HOME") {
            roots.push((
                PathBuf::from(home).join("Applications"),
                InstallScope::PerUser,
            ));
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for dir in ["/usr", "/opt"] {
            roots.push((PathBuf::from(dir), InstallScope::PerMachine));
        }
        if let Some(home) = std::env::var_os("HOME") {
            roots.push((PathBuf::from(home).join(".local"), InstallScope::PerUser));
        }
    }

    roots
}

/// 在候选目录里找出最深的匹配项（`/opt/app` 优先于 `/opt`）。
fn scope_from_roots(exe: &Path, roots: &[(PathBuf, InstallScope)]) -> Option<InstallScope> {
    let mut best: Option<(usize, InstallScope)> = None;
    for (root, scope) in roots {
        if !path_starts_with(exe, root) {
            continue;
        }
        let depth = root.components().count();
        if best.is_none_or(|(best_depth, _)| depth > best_depth) {
            best = Some((depth, *scope));
        }
    }
    best.map(|(_, scope)| scope)
}

#[cfg(windows)]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    // Windows 路径大小写不敏感，`C:\PROGRAM FILES` 与 `C:\Program Files` 视为同一目录。
    let path = PathBuf::from(path.to_string_lossy().to_lowercase());
    let root = PathBuf::from(root.to_string_lossy().to_lowercase());
    path.starts_with(&root)
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// 目录是否可写（推断不了安装位置时的兜底判断）。
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".auto-update-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deepest_root_wins() {
        let roots = vec![
            (PathBuf::from("/opt"), InstallScope::PerMachine),
            (PathBuf::from("/opt/app"), InstallScope::PerUser),
        ];
        assert_eq!(
            scope_from_roots(Path::new("/opt/app/bin/app.exe"), &roots),
            Some(InstallScope::PerUser)
        );
        assert_eq!(
            scope_from_roots(Path::new("/opt/other/app.exe"), &roots),
            Some(InstallScope::PerMachine)
        );
        assert_eq!(scope_from_roots(Path::new("/home/user/app.exe"), &roots), None);
    }

    #[test]
    fn scope_args_select_flags() {
        let args = ScopeArgs::new(["/CURRENTUSER"], ["/ALLUSERS"]);
        assert_eq!(args.for_scope(InstallScope::PerUser), ["/CURRENTUSER"]);
        assert_eq!(args.for_scope(InstallScope::PerMachine), ["/ALLUSERS"]);
        assert!(!args.is_empty());
        assert!(ScopeArgs::default().is_empty());
    }

    #[test]
    fn inno_setup_matches_install_scope() {
        let InstallerConfig::Command {
            args, scope_args, ..
        } = InstallerConfig::inno_setup()
        else {
            panic!("inno_setup 应该是 Command 策略");
        };
        assert_eq!(args, vec!["/SILENT".to_string()]);
        assert_eq!(scope_args.per_user, vec!["/CURRENTUSER".to_string()]);
        assert_eq!(scope_args.per_machine, vec!["/ALLUSERS".to_string()]);
    }

    #[test]
    fn explicit_scope_does_not_append_extra_args() {
        let InstallerConfig::Command { scope_args, .. } = InstallerConfig::inno_setup_all_users()
        else {
            panic!("inno_setup_all_users 应该是 Command 策略");
        };
        assert!(scope_args.is_empty());
    }
}
