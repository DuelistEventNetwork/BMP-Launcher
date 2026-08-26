use std::time::{Duration, Instant};

use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_WRITE, HANDLE},
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_NONE, OPEN_EXISTING, WriteFile,
        },
        System::{Pipes::WaitNamedPipeW, Registry::HKEY_CURRENT_USER},
    },
    core::PCWSTR,
};

use crate::{
    constants::{IPC_PIPE_NAME, LAUNCHER_NAME, URL_SCHEME},
    launcher_error::LauncherError,
    util::{reg_read, reg_write, wstr},
};

const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const RETRY_INTERVAL: Duration = Duration::from_millis(50);

pub fn set_payload_env(url: &str) {
    unsafe { std::env::set_var("BMP_URL", url) };
}

pub fn try_send_to_running_game(payload: &str) -> Result<bool, LauncherError> {
    let pipe_name = wstr(IPC_PIPE_NAME);
    let deadline = Instant::now() + CONNECT_TIMEOUT;

    loop {
        let error = match open_pipe(&pipe_name) {
            Ok(handle) => return write_payload(handle, payload).map(|()| true),
            Err(e) => e,
        };

        let code = error.code();

        if code == ERROR_FILE_NOT_FOUND.to_hresult() {
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(RETRY_INTERVAL);
        } else if code == ERROR_PIPE_BUSY.to_hresult() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }

            let signalled =
                unsafe { WaitNamedPipeW(PCWSTR(pipe_name.as_ptr()), remaining.as_millis() as u32) }
                    .as_bool();

            if !signalled {
                if Instant::now() >= deadline {
                    return Ok(false);
                }
                std::thread::sleep(RETRY_INTERVAL);
            }
        } else {
            return Err(LauncherError::Windows(error));
        }
    }
}

fn open_pipe(pipe_name: &[u16]) -> Result<HANDLE, windows::core::Error> {
    unsafe {
        CreateFileW(
            PCWSTR(pipe_name.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
}

fn write_payload(handle: HANDLE, payload: &str) -> Result<(), LauncherError> {
    let payload_bytes = payload.as_bytes();
    let mut written = 0;

    let result = unsafe { WriteFile(handle, Some(payload_bytes), Some(&mut written), None) };

    unsafe {
        CloseHandle(handle)?;
    }

    result.map_err(LauncherError::Windows)?;

    tracing::debug!("Sent {written} bytes to running game via IPC pipe");
    Ok(())
}

pub fn try_register_url_scheme(launcher_exe: &str) -> Result<(), LauncherError> {
    let base_key = format!(r"Software\Classes\{URL_SCHEME}");
    let command_key = format!(r"{base_key}\shell\open\command");
    let expected_command = format!("\"{launcher_exe}\" \"%1\"");

    if reg_read(HKEY_CURRENT_USER, &base_key, "").is_ok_and(|en| en == LAUNCHER_NAME)
        && reg_read(HKEY_CURRENT_USER, &command_key, "").is_ok_and(|ec| ec == expected_command)
    {
        tracing::debug!("URL scheme registration is up to date");
        return Ok(());
    }

    reg_write(HKEY_CURRENT_USER, &base_key, "", LAUNCHER_NAME)?;
    reg_write(HKEY_CURRENT_USER, &base_key, "URL Protocol", "")?;
    reg_write(HKEY_CURRENT_USER, &command_key, "", &expected_command)?;

    tracing::debug!(
        "Registered {}:// URL scheme to {}",
        URL_SCHEME,
        launcher_exe
    );

    Ok(())
}
