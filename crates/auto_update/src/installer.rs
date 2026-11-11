//! 安装方式：下载完成后用什么程序、以什么参数、在什么时机安装

use std::{
    fmt,
    path::{Path, PathBuf},
    process::Command,
};

use semver::Version;
use tracing::{info, warn};

use crate::AutoUpdateError;

/// 执行安装的时机
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstallTiming {
    /// 应用退出时执行，Windows 上覆盖正在运行的 exe 只能在这个时机
    #[default]
    OnQuit,
    /// 下载完成后立即执行
    Immediately,
}

/// 安装范围，决定安装包写入 `Program Files` 还是用户目录
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallScope {
    /// 所有用户，需要管理员权限
    PerMachine,
    /// 当前用户，不需要提权
    PerUser,
}

impl InstallScope {
    /// 按当前 exe 的位置推断已有安装范围
    ///
    /// 先比对系统已知目录（`Program Files`、`%LOCALAPPDATA%\Programs`、
    /// `/Applications`、`/usr` 等），未命中时探测 exe 所在目录是否可写，
    /// 可写视为 [`InstallScope::PerUser`]，否则视为 [`InstallScope::PerMachine`]
    ///
    /// 安装到自定义目录时只能依赖可写性探测，可能判断错误，此时应当用
    /// `InstallerConfig` 的 `per_user_args` 或 `per_machine_args` 显式指定参数
    pub fn detect() -> Self {
        if let Ok(exe) = std::env::current_exe()
            && let Some(scope) = scope_from_roots(&exe, &known_roots())
        {
            return scope;
        }

        let dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        match dir {
            Some(dir) if dir_writable(&dir) => Self::PerUser,
            _ => Self::PerMachine,
        }
    }
}

/// 安装策略
///
/// 用 `InstallerConfig::builder()` 构造，未设置的字段取默认值：`program` 与
/// `relocate_to` 为 `None`，`args`、`per_user_args`、`per_machine_args` 为空，
/// `timing` 为 [`InstallTiming::OnQuit`]，`hide_console` 为 `true`
///
/// 常用安装器直接调用预设构造函数 [`InstallerConfig::inno_setup`]、
/// [`InstallerConfig::nsis`]、[`InstallerConfig::msi`]
#[derive(Clone, bon::Builder)]
pub struct InstallerConfig {
    /// 要运行的程序，`None` 表示直接运行下载好的安装包
    #[builder(into)]
    program: Option<PathBuf>,

    /// 传给程序的固定参数，支持 `{installer}`、`{app}`、`{pid}` 占位符
    #[builder(default, into)]
    args: Vec<String>,

    /// 当前用户安装时追加的参数，由 [`InstallScope::detect`] 的结果决定是否使用
    #[builder(default, into)]
    per_user_args: Vec<String>,

    /// 全局安装时追加的参数，由 [`InstallScope::detect`] 的结果决定是否使用
    #[builder(default, into)]
    per_machine_args: Vec<String>,

    /// 执行时机
    #[builder(default)]
    timing: InstallTiming,

    /// 执行前把程序复制到该目录，用于「安装会清空应用目录」的场景
    #[builder(into)]
    relocate_to: Option<PathBuf>,

    /// 隐藏控制台窗口，仅 Windows 生效
    #[builder(default = true)]
    hide_console: bool,
}

impl fmt::Debug for InstallerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstallerConfig")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("per_user_args", &self.per_user_args)
            .field("per_machine_args", &self.per_machine_args)
            .field("timing", &self.timing)
            .field("relocate_to", &self.relocate_to)
            .field("hide_console", &self.hide_console)
            .finish()
    }
}

impl InstallerConfig {
    /// Inno Setup 静默安装，安装范围跟随当前 exe 的位置
    ///
    /// 构造结果：`args` 为 `["/SILENT"]`，`per_user_args` 为 `["/CURRENTUSER"]`，
    /// `per_machine_args` 为 `["/ALLUSERS"]`，`program` 与 `relocate_to` 为
    /// `None`，`timing` 为 [`InstallTiming::OnQuit`]，`hide_console` 为 `true`
    ///
    /// 全局安装会传 `/ALLUSERS` 并触发 UAC，用户级安装传 `/CURRENTUSER` 不触发
    /// UAC；需要安装脚本设置 `PrivilegesRequiredOverridesAllowed=commandline dialog`
    pub fn inno_setup() -> Self {
        Self::builder()
            .args(bon::vec!["/SILENT"])
            .per_user_args(bon::vec!["/CURRENTUSER"])
            .per_machine_args(bon::vec!["/ALLUSERS"])
            .build()
    }

    /// Inno Setup 静默安装，固定装到当前用户
    ///
    /// 构造结果：`args` 为 `["/SILENT", "/CURRENTUSER"]`，`per_user_args` 与
    /// `per_machine_args` 为空，其余字段同 [`InstallerConfig::inno_setup`]
    pub fn inno_setup_per_user() -> Self {
        Self::builder()
            .args(bon::vec!["/SILENT", "/CURRENTUSER"])
            .build()
    }

    /// Inno Setup 静默安装，固定装到所有用户，需要管理员权限
    ///
    /// 构造结果：`args` 为 `["/SILENT", "/ALLUSERS"]`，`per_user_args` 与
    /// `per_machine_args` 为空，其余字段同 [`InstallerConfig::inno_setup`]
    pub fn inno_setup_all_users() -> Self {
        Self::builder()
            .args(bon::vec!["/SILENT", "/ALLUSERS"])
            .build()
    }

    /// NSIS 静默安装，全局安装时追加 `/ALLUSERS`
    ///
    /// 构造结果：`args` 为 `["/S"]`，`per_machine_args` 为 `["/ALLUSERS"]`，
    /// `per_user_args` 为空，其余字段取默认值
    pub fn nsis() -> Self {
        Self::builder()
            .args(bon::vec!["/S"])
            .per_machine_args(bon::vec!["/ALLUSERS"])
            .build()
    }

    /// MSI 静默安装，全局安装使用 `ALLUSERS=1`，用户级安装使用 `ALLUSERS=2`
    ///
    /// 构造结果：`args` 为 `["/qn", "/norestart"]`，`per_user_args` 为
    /// `["ALLUSERS=2"]`，`per_machine_args` 为 `["ALLUSERS=1"]`，其余字段取默认值
    pub fn msi() -> Self {
        Self::builder()
            .args(bon::vec!["/qn", "/norestart"])
            .per_user_args(bon::vec!["ALLUSERS=2"])
            .per_machine_args(bon::vec!["ALLUSERS=1"])
            .build()
    }

    /// 执行时机
    pub fn timing(&self) -> InstallTiming {
        self.timing
    }

    /// 用 `package_path` 启动安装
    ///
    /// 运行 `program` 指定的程序，未指定时运行安装包本身；`args` 中的
    /// `{installer}` 替换为 `package_path`，`{app}` 替换为当前程序路径，
    /// `{pid}` 替换为当前进程 id；`per_user_args` 或 `per_machine_args` 按
    /// [`InstallScope::detect`] 的结果追加在最后
    ///
    /// 返回 `Ok` 表示安装进程已创建，不等待安装结束
    pub(crate) fn launch(
        &self,
        package_path: &Path,
        version: &Version,
    ) -> Result<(), AutoUpdateError> {
        if !package_path.exists() {
            return Err(AutoUpdateError::PackageNotFound(package_path.to_path_buf()));
        }

        let mut program_path = match &self.program {
            Some(program) => resolve_program(program),
            None => package_path.to_path_buf(),
        };

        if let Some(dir) = &self.relocate_to {
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
        command.args(self.resolve_args(package_path));

        let install_scope = self.detect_scope();
        if let Some(install_scope) = install_scope {
            command.args(self.scope_args(install_scope));
        }

        if self.hide_console {
            hide_console_window(&mut command);
        }

        info!(
            program = %program_path.display(),
            package = %package_path.display(),
            ?install_scope,
            %version,
            "starting update installer"
        );

        command.spawn().map(|_| ()).map_err(|error| {
            warn!(%error, "failed to start update installer");
            AutoUpdateError::Io(error)
        })
    }

    /// 替换 `args` 中的 `{installer}`、`{app}`、`{pid}` 占位符
    fn resolve_args(&self, package_path: &Path) -> Vec<String> {
        let package = package_path.display().to_string();
        let app = std::env::current_exe()
            .map(|exe| exe.display().to_string())
            .unwrap_or_default();
        let pid = std::process::id().to_string();

        self.args
            .iter()
            .map(|arg| {
                arg.replace("{installer}", &package)
                    .replace("{app}", &app)
                    .replace("{pid}", &pid)
            })
            .collect()
    }

    /// 取 `install_scope` 对应的追加参数
    fn scope_args(&self, install_scope: InstallScope) -> &[String] {
        match install_scope {
            InstallScope::PerUser => &self.per_user_args,
            InstallScope::PerMachine => &self.per_machine_args,
        }
    }

    /// 需要追加范围参数时才推断安装范围
    fn detect_scope(&self) -> Option<InstallScope> {
        let needed = !self.per_user_args.is_empty() || !self.per_machine_args.is_empty();
        needed.then(InstallScope::detect)
    }
}

/// 相对路径以当前 exe 所在目录为基准，便于打包 `tools/` 里的助手程序
fn resolve_program(program: &Path) -> PathBuf {
    if program.is_absolute() {
        return program.to_path_buf();
    }
    match std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        Some(dir) => dir.join(program),
        None => program.to_path_buf(),
    }
}

/// 系统上全局安装与用户级安装的典型目录
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
            roots.push((PathBuf::from(dir).join("Programs"), InstallScope::PerUser));
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

/// 在候选目录中取最深的匹配项，`/opt/app` 优先于 `/opt`
fn scope_from_roots(exe: &Path, roots: &[(PathBuf, InstallScope)]) -> Option<InstallScope> {
    let mut matched: Option<(usize, InstallScope)> = None;
    for (root, scope) in roots {
        if !path_starts_with(exe, root) {
            continue;
        }
        let depth = root.components().count();
        if matched.is_none_or(|(matched_depth, _)| depth > matched_depth) {
            matched = Some((depth, *scope));
        }
    }
    matched.map(|(_, scope)| scope)
}

#[cfg(windows)]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    let path = PathBuf::from(path.to_string_lossy().to_lowercase());
    let root = PathBuf::from(root.to_string_lossy().to_lowercase());
    path.starts_with(&root)
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// 探测目录是否可写，用于推断安装范围
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".auto-update-probe-{}", std::process::id()));
    if std::fs::write(&probe, b"").is_err() {
        return false;
    }
    let _ = std::fs::remove_file(&probe);
    true
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
        assert_eq!(
            scope_from_roots(Path::new("/home/user/app.exe"), &roots),
            None
        );
    }

    #[test]
    fn inno_setup_uses_detected_scope() {
        let installer_config = InstallerConfig::inno_setup();
        assert_eq!(installer_config.timing(), InstallTiming::OnQuit);
        assert_eq!(
            installer_config.scope_args(InstallScope::PerUser),
            ["/CURRENTUSER".to_owned()]
        );
        assert_eq!(
            installer_config.scope_args(InstallScope::PerMachine),
            ["/ALLUSERS".to_owned()]
        );
        assert_eq!(installer_config.args, ["/SILENT".to_owned()]);
    }

    #[test]
    fn explicit_preset_has_no_scope_args() {
        let installer_config = InstallerConfig::inno_setup_all_users();
        assert!(
            installer_config
                .scope_args(InstallScope::PerUser)
                .is_empty()
        );
        assert!(
            installer_config
                .scope_args(InstallScope::PerMachine)
                .is_empty()
        );
    }

    #[test]
    fn resolves_installer_placeholder() {
        let installer_config = InstallerConfig::builder()
            .args(bon::vec![
                "--installer-path",
                "{installer}",
                "--auto-remove"
            ])
            .build();
        assert_eq!(
            installer_config.resolve_args(Path::new(r"C:\Temp\Setup.exe")),
            vec![
                "--installer-path".to_owned(),
                r"C:\Temp\Setup.exe".to_owned(),
                "--auto-remove".to_owned(),
            ]
        );
    }

    #[test]
    fn resolves_app_and_pid_placeholders() {
        let installer_config = InstallerConfig::builder()
            .args(bon::vec!["--pid", "{pid}", "--app", "{app}"])
            .build();
        let args = installer_config.resolve_args(Path::new("Setup.exe"));

        assert_eq!(args[1], std::process::id().to_string());
        assert!(Path::new(&args[3]).exists());
    }
}
