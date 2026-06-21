//! Windows dual-subsystem console reattachment.
//!
//! Shipped Windows builds are GUI-subsystem (see the `windows_subsystem`
//! attribute in `src/main.rs`) so a double-click never flashes a console window.
//! But the same `ashwend.exe` is also the CLI (`server`, `admin`,
//! `multiplayer-test`, `--help`/`--version`), and a GUI-subsystem process is
//! born with no console, so that output would go nowhere when the binary is run
//! from `cmd`/PowerShell.
//!
//! [`attach_parent_console`] reattaches the process to the console of whoever
//! launched it (`AttachConsole(ATTACH_PARENT_PROCESS)`) and rebinds the standard
//! streams onto it. When there is no parent console (the normal GUI double-click,
//! where the launcher is Explorer/LaunchServices), the attach fails and the
//! whole thing is a silent no-op, so the GUI path stays console-free. Best-effort
//! throughout: losing CLI echo is never fatal, and the client also mirrors all
//! logs to a file via the Bevy `LogPlugin` layer (`crate::logging`).
//!
//! Note the standard dual-subsystem quirk: a shell launching a GUI-subsystem
//! process does not wait on it, so `ashwend server` returns the prompt
//! immediately even though its output is written into the attached console, and
//! `>` redirection set up by the shell is not inherited. The dedicated server
//! normally runs on Linux, so this is an accepted trade-off for a console-free
//! double-click on the Windows client. No-op on every non-Windows platform.

/// Reattach stdout/stderr/stdin to the launching terminal's console on Windows,
/// if there is one. See the module docs. No-op off Windows and on a GUI launch.
pub fn attach_parent_console() {
    #[cfg(windows)]
    imp::attach();
}

#[cfg(windows)]
mod imp {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;

    type Handle = *mut c_void;

    const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF; // (DWORD)-1
    const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6; // (DWORD)-10
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // (DWORD)-11
    const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // (DWORD)-12
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const OPEN_EXISTING: u32 = 3;
    const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;

    // Linked from kernel32, always present. Declared inline to avoid taking a
    // direct dependency on `windows-sys` (already in the tree only transitively,
    // so its version is not ours to pin).
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn SetStdHandle(std_handle: u32, handle: Handle) -> i32;
        fn CreateFileW(
            file_name: *const u16,
            desired_access: u32,
            share_mode: u32,
            security_attributes: *mut c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: Handle,
        ) -> Handle;
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Open a console device (`CONOUT$`/`CONIN$`) and bind it to a standard
    /// handle so the freshly-attached console becomes the target of the standard
    /// streams. `AttachConsole` alone leaves the process's standard handles as
    /// they were at startup, which for a GUI-subsystem launch are invalid, so
    /// without this rebind the output would still be dropped. Rust fetches the
    /// handle via `GetStdHandle` on each write, so a `SetStdHandle` done here
    /// (before any output) is what its `println!`/`eprintln!` pick up.
    fn rebind(std_handle: u32, device: &str) {
        let name = wide(device);
        // SAFETY: `name` is a NUL-terminated wide string that outlives the call;
        // the security-attributes and template-file pointers are nullable.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE && !handle.is_null() {
            // SAFETY: `handle` is a valid console device handle from CreateFileW.
            unsafe {
                SetStdHandle(std_handle, handle);
            }
        }
    }

    pub fn attach() {
        // SAFETY: ATTACH_PARENT_PROCESS is the documented sentinel; the call has
        // no preconditions. Returns 0 with no parent console (the GUI launch),
        // which is the common, intended no-op case.
        let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
        if attached == 0 {
            return;
        }
        rebind(STD_OUTPUT_HANDLE, "CONOUT$");
        rebind(STD_ERROR_HANDLE, "CONOUT$");
        rebind(STD_INPUT_HANDLE, "CONIN$");
    }
}
