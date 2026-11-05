use std::{ffi::OsStr, process::Command};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Create a new [`std::process::Command`] with the console window hidden on
/// Windows (so spawning helper executables does not flash a terminal window).
///
/// This is a blocking/synchronous API: it is only used from background threads
/// where blocking is harmless, avoiding any async-runtime dependency for
/// process management.
pub fn new_command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
