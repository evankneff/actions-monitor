//! Console attachment for a `windows_subsystem = "windows"` binary.
//!
//! No console is exactly what you want for a background app started at login,
//! but it would also mean `--install-autostart` and `--help` printed into the
//! void. This restores stdout when there is somewhere sensible to send it.

#[cfg(windows)]
use windows::Win32::Foundation::{GENERIC_WRITE, HANDLE};
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
#[cfg(windows)]
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, FreeConsole, GetStdHandle,
    STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
};
#[cfg(windows)]
use windows::core::w;

/// Console (re)attachment is a `windows_subsystem = "windows"` problem: a release
/// binary with no console needs help getting `--help`/`--install-autostart` output
/// anywhere, and a Windows console kills every attached process when its window
/// closes (see `detach` below). Neither is true of a Terminal-launched macOS binary,
/// so `attach`/`detach` are no-ops there - stdout already works, and quitting the
/// Terminal window does not `SIGKILL` this process the way closing a Windows console
/// does.
#[cfg(not(windows))]
pub fn attach(_allocate: bool) -> bool {
    true
}

#[cfg(not(windows))]
pub fn detach() {}

/// Make sure this process can print, and report whether it can.
///
/// Three cases, in order of preference:
///
/// 1. We already have a usable stdout - a debug build, output redirected to a
///    file or pipe, or handles inherited from a console parent. Leave it well
///    alone: overwriting it here is what breaks `actions-monitor --help > x.txt`.
/// 2. Attach to the terminal that launched us, if there is one.
/// 3. Only if `allocate` is set (i.e. `--console`), open a console of our own.
#[cfg(windows)]
pub fn attach(allocate: bool) -> bool {
    if has_usable_stdout() {
        return true;
    }
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            redirect_std_handles();
            return true;
        }
        if allocate && AllocConsole().is_ok() {
            redirect_std_handles();
            return true;
        }
    }
    false
}

/// Let go of whatever console we are attached to.
///
/// This matters more than it looks. Windows terminates **every process attached
/// to a console** when that console window is closed (CTRL_CLOSE_EVENT), so a
/// background app that attached to the terminal it was launched from dies the
/// moment that terminal is closed - which is exactly what happened, and is fatal
/// for something meant to run for weeks.
///
/// So the long-running path prints whatever it needs to and then detaches before
/// entering the event loop. Calling this with no console attached is harmless.
#[cfg(windows)]
pub fn detach() {
    unsafe {
        let _ = FreeConsole();
    }
}

#[cfg(windows)]
fn has_usable_stdout() -> bool {
    unsafe { GetStdHandle(STD_OUTPUT_HANDLE).is_ok_and(|handle| !handle.is_invalid()) }
}

/// Point the process's stdout/stderr at the console we just acquired.
///
/// Rust's standard streams resolve their handle through `GetStdHandle` on every
/// write, so replacing the handles here is enough for `println!` to work.
#[cfg(windows)]
unsafe fn redirect_std_handles() {
    unsafe {
        let Some(handle) = open_conout() else { return };
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, handle);
        let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
    }
}

#[cfg(windows)]
unsafe fn open_conout() -> Option<HANDLE> {
    unsafe {
        CreateFileW(
            w!("CONOUT$"),
            GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
        .ok()
    }
}
