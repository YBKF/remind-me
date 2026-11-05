#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the installer executable
    #[arg(short, long)]
    installer_path: String,

    /// Install for all users
    #[arg(long, default_value_t = false)]
    all_users: bool,

    /// Automatically remove the installer after installation
    #[arg(long, default_value_t = false)]
    auto_remove: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let mut cmd = std::process::Command::new(&args.installer_path);

    cmd.arg("/silent");

    if args.all_users {
        cmd.arg("/ALLUSERS");
    } else {
        cmd.arg("/CURRENTUSER");
    }

    let output = cmd.output()?;
    anyhow::ensure!(
        output.status.success(),
        "failed to start installer: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );

    if args.auto_remove {
        std::fs::remove_file(&args.installer_path)?;
    }

    Ok(())
}
