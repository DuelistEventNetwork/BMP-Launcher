use windows::Win32::Foundation::{CloseHandle, HANDLE, MAX_PATH};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

use crate::constants::{ELDENRING_ID, PROCESS_INJECTION_ACCESS};
use crate::launcher_error::LauncherError;
use crate::steamlocate::locate_steam_game;
use crate::util::{promp_confirmation, wstr};

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED, CreateProcessA, CreateRemoteThread,
    GetExitCodeThread, INFINITE, OpenProcess, PROCESS_INFORMATION, STARTUPINFOA, TerminateProcess,
    WaitForSingleObject,
};
use windows::core::s;
use windows::core::{PCSTR, PCWSTR};

fn is_under_onedrive(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .to_lowercase()
            .contains("onedrive")
    })
}

fn is_under_tmp(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .to_lowercase()
            .eq("temp")
    })
}

fn is_steam_running() -> bool {
    !get_pids_by_name("steam.exe").is_empty()
}

fn open_process_by_pid(pid: u32) -> Option<HANDLE> {
    unsafe { OpenProcess(PROCESS_INJECTION_ACCESS, false, pid) }.ok()
}

pub fn kill_process(pid: u32) {
    open_process_by_pid(pid).and_then(|handle| {
        unsafe { TerminateProcess(handle, 1) }
            .map_err(|err| tracing::error!("Failed to terminate process: {:?}", err))
            .ok()
    });
}

pub fn get_pids_by_name(name: &str) -> Vec<u32> {
    let name_lower = name.to_lowercase();
    let mut pids = Vec::new();

    let snapshot = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(h) => h,
        Err(_) => return pids,
    };

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };

    if unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok() {
        loop {
            let exe_name = String::from_utf16_lossy(
                &entry.szExeFile[..entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(MAX_PATH as usize)],
            )
            .to_lowercase();

            if exe_name.contains(&name_lower) {
                pids.push(entry.th32ProcessID);
            }

            entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };

            if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
                break;
            }
        }
    }

    let _ = unsafe { CloseHandle(snapshot) };
    pids
}

pub fn start_game(
    content_dir: &PathBuf,
    dll_name: &str,
    game_executable: &str,
    game_dir: Option<&PathBuf>,
    debug: bool,
) -> Result<(), LauncherError> {
    if !is_steam_running() {
        return Err("Steam does not appear to be running. Start Steam before launching Better Multiplayer Launcher.".into());
    }

    // Kill existing processes
    for pid in get_pids_by_name(game_executable) {
        if debug {
            tracing::warn!(
                "Skipping process {} termination, because DEN_DEBUG is set",
                pid
            );
            continue;
        }
        kill_process(pid);
    }

    // Setup paths
    let executable_path = match game_dir {
        Some(dir) => {
            let path = dir.join(game_executable);
            if !path.exists() {
                return Err(LauncherError::GameNotFound(format!(
                    "Game executable not found at explicit game dir: {}",
                    path.display()
                )));
            }
            path
        }
        None => locate_steam_game(game_executable)?,
    };
    tracing::info!("Located game executable at {}", executable_path.display());
    let current_exe = std::env::current_exe()?;
    let parent_dir = current_exe
        .parent()
        .ok_or("Failed to get current executable dir path")?;

    if is_under_onedrive(parent_dir) {
        tracing::warn!("The launcher is running from a OneDrive-managed folder:");
        tracing::warn!("{}", parent_dir.display());
        tracing::warn!("This can interfere with launcher operations.");
        tracing::warn!(
            "Please try moving it out of the OneDrive folder if you encounter any issues."
        );
        promp_confirmation()
    }

    if is_under_tmp(parent_dir) {
        return Err(LauncherError::UnsupportedLaunchPath(format!(
            "The launcher is running from a temporary folder \"{}\", which can interfere with launcher operations. \
            This usually means the release ZIP was not fully extracted and the launcher is being run directly from the ZIP. \
            Unpack the entire archive to a local directory and run the launcher from there.",
            parent_dir.display()
        )));
    }

    let content_dir_path = parent_dir.join(content_dir);
    if !content_dir_path.exists() {
        return Err(LauncherError::ContentDirMissing(format!(
            "The content directory \"{}\" does not exist. This usually means the release ZIP was not fully extracted. Unpack the entire archive, not just the launcher executable.",
            content_dir_path.display()
        )));
    }

    let dll_path = content_dir_path.join(dll_name);
    tracing::info!("Injecting DLL: {}", dll_path.display());

    if !dll_path.exists() {
        return Err(LauncherError::DllNotFound(format!(
            "DLL not found at {}. Make sure all files were unpacked from the release archive, not just the launcher executable.",
            dll_path.display()
        )));
    }

    unsafe {
        // Set Steam App ID
        std::env::set_var("SteamAppId", ELDENRING_ID.to_string());
        // Set Content Dir
        std::env::set_var("DEN_CONTENT_DIR", &content_dir_path);
    }

    // Create process
    let process_info = create_suspended_process(&executable_path)?;

    // Inject main DLL
    inject_dll(&process_info, &dll_path)?;

    // Resume process
    unsafe { windows::Win32::System::Threading::ResumeThread(process_info.hThread) };

    Ok(())
}

fn create_suspended_process(executable_path: &Path) -> Result<PROCESS_INFORMATION, LauncherError> {
    let exe_path_cstr = std::ffi::CString::new(
        executable_path
            .to_str()
            .ok_or_else(|| LauncherError::InvalidPath("Invalid path".into()))?,
    )?;

    let startup_info = STARTUPINFOA::default();
    let mut process_info = PROCESS_INFORMATION::default();

    let cwd = executable_path
        .parent()
        .ok_or_else(|| LauncherError::InvalidPath("Invalid executable path".into()))?;
    std::env::set_current_dir(cwd)?;

    unsafe {
        CreateProcessA(
            PCSTR(exe_path_cstr.as_ptr() as *const u8),
            None,
            None,
            None,
            false,
            CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP,
            None,
            None,
            &startup_info,
            &mut process_info,
        )
    }?;

    Ok(process_info)
}

fn inject_dll(process_info: &PROCESS_INFORMATION, dll_path: &Path) -> Result<(), LauncherError> {
    let dll_path_str = dll_path
        .to_str()
        .ok_or_else(|| LauncherError::InvalidPath("Invalid path".into()))?;
    let wide_path = wstr(dll_path_str);
    let pcwstr = PCWSTR::from_raw(wide_path.as_ptr());
    let buffer_size = (wide_path.len()) * std::mem::size_of::<u16>();

    let str_addr = unsafe {
        VirtualAllocEx(
            process_info.hProcess,
            None,
            buffer_size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };

    if str_addr.is_null() {
        return Err(LauncherError::Other(
            "Failed to allocate memory in target process".into(),
        ));
    }

    let mut bytes_written: usize = 0;
    unsafe {
        WriteProcessMemory(
            process_info.hProcess,
            str_addr,
            pcwstr.as_ptr() as *const c_void,
            buffer_size,
            Some(&mut bytes_written as *mut usize),
        )?;
    }

    tracing::debug!(
        "Wrote DLL path to target process memory, bytes written: {}",
        bytes_written
    );

    let kernel32 = unsafe { GetModuleHandleA(s!("kernel32.dll"))? };
    let load_library = unsafe {
        GetProcAddress(kernel32, s!("LoadLibraryW")).ok_or_else(|| {
            LauncherError::InjectionFailed("Failed to resolve LoadLibraryW in kernel32.dll.".into())
        })
    }? as *const ();

    unsafe {
        let thread_handle = CreateRemoteThread(
            process_info.hProcess,
            None,
            0,
            Some(std::mem::transmute::<
                *const (),
                unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
            >(load_library)),
            Some(str_addr),
            0,
            None,
        )?;

        tracing::debug!("DLL injected, waiting for thread completion");
        WaitForSingleObject(thread_handle, INFINITE);

        VirtualFreeEx(process_info.hProcess, str_addr, 0, MEM_RELEASE)?;

        let mut load_offset: u32 = 0;
        GetExitCodeThread(thread_handle, &mut load_offset)?;

        if load_offset == 0 {
            return Err(LauncherError::InjectionFailed(
                "LoadLibraryW returned NULL. The DLL could not be loaded into the game process."
                    .into(),
            ));
        }

        tracing::debug!("DLL successfully loaded at: eldenring.exe + {load_offset:#010x}",);
        CloseHandle(thread_handle)?;
    }

    Ok(())
}
