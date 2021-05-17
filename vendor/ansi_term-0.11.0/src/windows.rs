/// Enables ANSI code support on Windows 10.
///
/// This uses Windows API calls to alter the properties of the console that
/// the program is running in.
///
/// https://msdn.microsoft.com/en-us/library/windows/desktop/mt638032(v=vs.85).aspx
///
/// Returns a `Result` with the Windows error code if unsuccessful.
#[cfg(windows)]
pub fn enable_ansi_support() -> Result<(), u32> {
    use winapi::um::processenv::GetStdHandle;
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::consoleapi::{GetConsoleMode, SetConsoleMode};

    const STD_OUT_HANDLE: u32 = -11i32 as u32;
    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    unsafe {
        // https://docs.microsoft.com/en-us/windows/console/getstdhandle
        let std_out_handle = GetStdHandle(STD_OUT_HANDLE);
        let error_code = GetLastError();
        if error_code != 0 { return Err(error_code); }
        
        // https://docs.microsoft.com/en-us/windows/console/getconsolemode
        let mut console_mode: u32 = 0;
        GetConsoleMode(std_out_handle, &mut console_mode);
        let error_code = GetLastError();
        if error_code != 0 { return Err(error_code); }

        // VT processing not already enabled?
        if console_mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING == 0 {
            // https://docs.microsoft.com/en-us/windows/console/setconsolemode
            SetConsoleMode(std_out_handle, console_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            let error_code = GetLastError();
            if error_code != 0 { return Err(error_code); }
        }
    }

    return Ok(());
}
