use crate::launcher_error::LauncherError;
use std::io::Read;
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE},
        System::{
            Registry::{
                HKEY, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE,
                RegCloseKey, RegCreateKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
            },
            Threading::CreateMutexW,
        },
    },
    core::PCWSTR,
};

pub fn wstr(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

pub struct SingleInstanceGuard(HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub enum SingleInstance {
    Acquired(SingleInstanceGuard),
    AlreadyRunning,
    Unavailable,
}

pub fn acquire_single_instance(name: &str) -> SingleInstance {
    let name_w = wstr(name);

    match unsafe { CreateMutexW(None, true, PCWSTR(name_w.as_ptr())) } {
        Ok(handle) => {
            let already_exists =
                windows::core::Error::from_thread().code() == ERROR_ALREADY_EXISTS.to_hresult();

            if already_exists {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                SingleInstance::AlreadyRunning
            } else {
                SingleInstance::Acquired(SingleInstanceGuard(handle))
            }
        }
        Err(e) => {
            tracing::debug!("Failed to create single-instance mutex: {e}");
            SingleInstance::Unavailable
        }
    }
}

pub fn reg_read(hkey_root: HKEY, subkey: &str, value_name: &str) -> Result<String, LauncherError> {
    unsafe {
        let hkey = {
            let subkey_w = wstr(subkey);
            let mut phkresult = HKEY::default();

            RegOpenKeyExW(
                hkey_root,
                PCWSTR(subkey_w.as_ptr()),
                None,
                KEY_READ,
                &mut phkresult,
            )
            .ok()
            .map(|_| phkresult)
            .map_err(LauncherError::Windows)?
        };

        let value_w = wstr(value_name);

        let mut buffer = [0u16; 512];
        let mut buffer_size = (buffer.len() * 2) as u32;
        let mut value_type = REG_VALUE_TYPE::default();

        let status = RegQueryValueExW(
            hkey,
            PCWSTR(value_w.as_ptr()),
            None,
            Some(&mut value_type),
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut buffer_size),
        );

        RegCloseKey(hkey).ok()?;

        if status != ERROR_SUCCESS {
            return Err(LauncherError::Windows(status.into()));
        }

        let len = (buffer_size as usize / 2).saturating_sub(1);

        Ok(String::from_utf16_lossy(&buffer[..len]))
    }
}

pub fn reg_write(
    hkey: HKEY,
    subkey: &str,
    value_name: &str,
    data: &str,
) -> Result<(), LauncherError> {
    unsafe {
        let subkey_w = wstr(subkey);

        let mut phkresult = HKEY::default();

        RegCreateKeyExW(
            hkey,
            PCWSTR(subkey_w.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut phkresult,
            None,
        )
        .ok()
        .map_err(LauncherError::Windows)?;

        let value_w = wstr(value_name);
        let data_w = wstr(data);

        let data_bytes = std::slice::from_raw_parts(data_w.as_ptr().cast::<u8>(), data_w.len() * 2);

        let result = RegSetValueExW(
            phkresult,
            PCWSTR(value_w.as_ptr()),
            None,
            REG_SZ,
            Some(data_bytes),
        )
        .ok();

        RegCloseKey(phkresult).ok()?;

        result.map_err(LauncherError::Windows)?;
        Ok(())
    }
}

pub fn wait_for_exit() -> ! {
    tracing::info!("Press any key to exit...");
    let _ = std::io::stdin().read(&mut [0u8]);
    std::process::exit(1);
}

pub fn promp_confirmation() {
    tracing::warn!("Press any key to continue...");
    let _ = std::io::stdin().read(&mut [0u8]);
}
