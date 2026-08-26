use windows::Win32::System::Threading::{
    PROCESS_ACCESS_RIGHTS, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION, PROCESS_TERMINATE,
    PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const LAUNCHER_NAME: &str = "Better Multiplayer Launcher";

pub const ELDENRING_ID: u32 = 1245620;
pub const ELDENRING_EXE: &str = "eldenring.exe";

pub static PROCESS_INJECTION_ACCESS: PROCESS_ACCESS_RIGHTS = PROCESS_ACCESS_RIGHTS(
    PROCESS_CREATE_THREAD.0
        | PROCESS_QUERY_INFORMATION.0
        | PROCESS_VM_OPERATION.0
        | PROCESS_VM_READ.0
        | PROCESS_VM_WRITE.0
        | PROCESS_TERMINATE.0,
);

pub const RELEASE_PUBLIC_KEY: &[u8; 32] = include_bytes!("../release_public_key.bin");

pub const URL_SCHEME: &str = "bmp";
pub const URL_PREFIX: &str = "bmp://";

pub const IPC_PIPE_NAME: &str = r"\\.\pipe\better_multiplayer_ipc";

pub const SINGLE_INSTANCE_MUTEX: &str = r"Local\better_multiplayer_launcher";
