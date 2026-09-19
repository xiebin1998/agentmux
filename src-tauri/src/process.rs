//! 子进程统一隐藏控制台窗口。
//!
//! Windows 上 spawn 控制台程序（cmd / tasklist / dws / qodercli …）时，如果不指定
//! `CREATE_NO_WINDOW`，系统会为它弹出一个控制台窗口。宿主在轮询里频繁 spawn，
//! 用户看到的现象就是「cmd 窗口一直闪」。

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 给 tokio 的子进程隐藏控制台窗口。
pub fn hide_console(command: &mut tokio::process::Command) {
    #[cfg(not(windows))]
    let _ = command;

    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
}

/// 给标准库的子进程隐藏控制台窗口。
pub fn hide_console_std(command: &mut std::process::Command) {
    #[cfg(not(windows))]
    let _ = command;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
}
