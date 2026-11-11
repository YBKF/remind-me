#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! 安装助手：等待主程序退出、执行安装包、成功后删除安装包、重新启动主程序
//!
//! 由 `auto_update` 的 `InstallerConfig` 在程序退出时启动，参数全部来自调用方，
//! 不硬编码任何应用信息

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{Context as _, Result};
use clap::Parser;

/// 安装失败时的诊断日志，写在安装包所在目录
const LOG_FILE: &str = "auto_update_helper.log";

#[derive(Debug, Parser)]
#[command(version, about = "安装包执行助手")]
struct Args {
    /// 安装包路径
    #[arg(short, long)]
    installer_path: PathBuf,

    /// 安装成功后删除安装包
    #[arg(long)]
    auto_remove: bool,

    /// 安装结束后启动的程序
    #[arg(long)]
    relaunch: Option<PathBuf>,

    /// 等待该进程退出后再执行安装包，避免安装时程序仍在运行
    #[arg(long)]
    wait_pid: Option<u32>,

    /// 等待 `wait_pid` 退出的最长秒数，超时后继续安装
    #[arg(long, default_value_t = 60)]
    wait_timeout: u64,

    /// 原样传给安装包的参数，需要写在 `--` 之后
    #[arg(last = true)]
    installer_args: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if let Some(pid) = args.wait_pid {
        log_line(&args.installer_path, &format!("waiting for pid {pid}"));
        wait_for_exit(pid, Duration::from_secs(args.wait_timeout));
    }

    let installed = match run_installer(&args) {
        Ok(success) => success,
        Err(error) => {
            log_line(
                &args.installer_path,
                &format!("failed to run installer: {error:#}"),
            );
            false
        }
    };

    if installed && args.auto_remove {
        log_line(&args.installer_path, "removing installer");
        let _ = std::fs::remove_file(&args.installer_path);
    }

    if let Some(relaunch) = &args.relaunch
        && let Err(error) = relaunch_app(relaunch)
    {
        log_line(
            &args.installer_path,
            &format!("failed to relaunch {}: {error:#}", relaunch.display()),
        );
    }

    anyhow::ensure!(installed, "installer did not finish successfully");
    Ok(())
}

/// 执行 `args.installer_path` 并等待结束，`installer_args` 原样传给安装包
///
/// 返回安装包是否以退出码 0 结束
fn run_installer(args: &Args) -> Result<bool> {
    let exit_status = Command::new(&args.installer_path)
        .args(&args.installer_args)
        .status()
        .with_context(|| format!("cannot run {}", args.installer_path.display()))?;

    log_line(
        &args.installer_path,
        &format!("installer exited with {exit_status}"),
    );
    Ok(exit_status.success())
}

/// 启动 `relaunch`，工作目录设为它所在目录
fn relaunch_app(relaunch: &Path) -> Result<()> {
    let mut command = Command::new(relaunch);
    if let Some(dir) = relaunch.parent() {
        command.current_dir(dir);
    }
    command
        .spawn()
        .with_context(|| format!("cannot start {}", relaunch.display()))?;
    Ok(())
}

/// 等待 `pid` 退出，超时或进程不存在时直接返回，文件占用问题交给安装包处理
#[cfg(windows)]
fn wait_for_exit(pid: u32, timeout: Duration) {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, WaitForSingleObject},
    };

    /// 等待进程退出所需的最小权限，windows-sys 未导出该常量
    const SYNCHRONIZE: u32 = 0x0010_0000;

    // SAFETY: 只申请 SYNCHRONIZE 权限用于等待退出，句柄用后立即关闭
    unsafe {
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return;
        }
        WaitForSingleObject(handle, timeout.as_millis() as u32);
        CloseHandle(handle);
    }
}

/// 非 Windows 没有等价的等待接口，固定等待一秒
#[cfg(not(windows))]
fn wait_for_exit(_pid: u32, _timeout: Duration) {
    std::thread::sleep(Duration::from_secs(1));
}

/// 把 `message` 追加到安装包所在目录的 [`LOG_FILE`]
///
/// 助手在 release 下没有控制台，失败原因只能靠日志文件带出
fn log_line(installer_path: &Path, message: &str) {
    let Some(dir) = installer_path.parent() else {
        return;
    };
    let line = format!("{message}\n");
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILE))
    else {
        return;
    };
    let _ = file.write_all(line.as_bytes());
}
