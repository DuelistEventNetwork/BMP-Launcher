mod constants;
mod injector;
mod launcher_error;
mod logging;
mod migrations;
mod steamlocate;
mod updater;
mod url_handler;
mod util;
mod winhttp_client;

use std::path::PathBuf;

use constants::ELDENRING_EXE;
use den_signer::VerifyingKey;
use dotenvy_macro::dotenv;
use injector::start_game;
use logging::{den_panic_hook, enable_ansi_support, setup_logging};
use updater::start_updater;

use crate::{
    constants::{RELEASE_PUBLIC_KEY, SINGLE_INSTANCE_MUTEX, URL_PREFIX},
    launcher_error::LauncherError,
    util::{SingleInstance, acquire_single_instance, wait_for_exit},
};

struct Args {
    skip_updates: bool,
    skip_url_scheme: bool,
    updater_repo_owner: String,
    updater_repo_name: String,
    updater_repo_private_key: Option<String>,
    content_dir: PathBuf,
    dll_name: String,
    game_executable: String,
    game_dir: Option<PathBuf>,
    debug: bool,
}
const HELP: &str = concat!(
    "Better Multiplayer Launcher v",
    env!("CARGO_PKG_VERSION"),
    "

USAGE:
    launcher [OPTIONS]

OPTIONS:
    -h, --help                        Print this help message and exit
        --skip-updates                Skip the update check          [env: DEN_SKIP_UPDATES]
        --skip-url-scheme             Don't register the URL scheme handler
        --updater-repo-owner <OWNER>  Owner of the update repository [env: DEN_REPO_OWNER]
        --updater-repo-name <NAME>    Name of the update repository  [env: DEN_REPO_NAME]
        --updater-repo-private-key <KEY>
                                      Private key used to access a private update repository
        --content-dir <DIR>           Directory containing the mod content [env: DEN_CONTENT_DIR]
        --dll-name <NAME>             Name of the DLL to inject      [env: DEN_DLL_NAME]
        --game-executable <NAME>      Game executable to launch      [env: DEN_GAME_EXECUTABLE]
        --game-dir <DIR>              Explicit game directory, skipping Steam
                                      library lookup                 [env: DEN_GAME_DIR]
        --debug                       Enable debug logging           [env: DEN_DEBUG]
"
);

impl Args {
    fn parse() -> Result<Self, pico_args::Error> {
        let mut pargs = pico_args::Arguments::from_env();

        if pargs.contains(["-h", "--help"]) {
            print!("{HELP}");
            std::process::exit(0);
        }

        Ok(Args {
            skip_updates: pargs.contains("--skip-updates")
                || std::env::var("DEN_SKIP_UPDATES").is_ok(),
            skip_url_scheme: pargs.contains("--skip-url-scheme"),
            updater_repo_owner: pargs
                .opt_value_from_str("--updater-repo-owner")?
                .or_else(|| std::env::var("DEN_REPO_OWNER").ok())
                .unwrap_or_else(|| dotenv!("DEN_REPO_OWNER").into()),
            updater_repo_name: pargs
                .opt_value_from_str("--updater-repo-name")?
                .or_else(|| std::env::var("DEN_REPO_NAME").ok())
                .unwrap_or_else(|| dotenv!("DEN_REPO_NAME").into()),
            updater_repo_private_key: pargs.opt_value_from_str("--updater-repo-private-key")?,
            content_dir: pargs
                .opt_value_from_str("--content-dir")?
                .or_else(|| std::env::var("DEN_CONTENT_DIR").ok().map(Into::into))
                .unwrap_or_else(|| dotenv!("DEN_CONTENT_DIR").into()),
            dll_name: pargs
                .opt_value_from_str("--dll-name")?
                .or_else(|| std::env::var("DEN_DLL_NAME").ok())
                .unwrap_or_else(|| dotenv!("DEN_DLL_NAME").into()),
            game_executable: pargs
                .opt_value_from_str("--game-executable")?
                .or_else(|| std::env::var("DEN_GAME_EXECUTABLE").ok())
                .unwrap_or_else(|| ELDENRING_EXE.to_owned()),
            game_dir: pargs
                .opt_value_from_str::<_, PathBuf>("--game-dir")?
                .or_else(|| std::env::var_os("DEN_GAME_DIR").map(Into::into)),
            debug: pargs.contains("--debug") || std::env::var("DEN_DEBUG").is_ok(),
        })
    }
}

fn get_verifying_key(bytes: &[u8; 32]) -> VerifyingKey {
    VerifyingKey::from_bytes(bytes).expect("release_public_key.bin is not a valid ed25519 key")
}

fn main() {
    enable_ansi_support().ok();
    std::panic::set_hook(Box::new(den_panic_hook));

    let raw_args: Vec<String> = std::env::args().collect();
    let url_arg = raw_args
        .get(1)
        .filter(|a| a.starts_with(URL_PREFIX))
        .cloned();

    if let Some(ref url) = url_arg {
        setup_logging(false);
        tracing::info!("Launched via URL scheme: {url}");

        match url_handler::try_send_to_running_game(url) {
            Ok(true) => {
                tracing::info!("Payload delivered to running game");
                return;
            }
            Ok(false) => {
                tracing::debug!("Game not running; setting BMP_URL and launching normally");
                url_handler::set_payload_env(url);
            }
            Err(e) => {
                tracing::error!("IPC error forwarding URL payload: {e}");
                return;
            }
        }
    }

    let args = Args::parse().unwrap_or_else(|e| {
        eprintln!("Error parsing arguments: {e}");
        wait_for_exit();
    });

    if url_arg.is_none() {
        setup_logging(args.debug);
    }

    let _instance_guard = if args.game_dir.is_some() {
        tracing::warn!("Skipping single-instance check, because --game-dir/DEN_GAME_DIR is set");
        None
    } else {
        match acquire_single_instance(SINGLE_INSTANCE_MUTEX) {
            SingleInstance::Acquired(guard) => Some(guard),
            SingleInstance::AlreadyRunning => {
                tracing::warn!("Another launcher instance is already running, exiting.");
                return;
            }
            SingleInstance::Unavailable => None,
        }
    };

    if !args.skip_url_scheme {
        let exe = std::env::current_exe().expect("Failed to get current exe path");
        if let Err(e) = url_handler::try_register_url_scheme(&exe.to_string_lossy()) {
            tracing::error!("Failed to register URL scheme: {e}");
            wait_for_exit();
        }
    }

    tracing::info!(
        "Starting Better Multiplayer Launcher v{}",
        env!("CARGO_PKG_VERSION")
    );

    let current_exe = std::env::current_exe().expect("Failed to get current exe path");
    let exe_dir = current_exe
        .parent()
        .expect("Failed to get current exe dir")
        .to_path_buf();

    tracing::debug!("Running migrations in {}", exe_dir.display());
    migrations::run(&exe_dir);

    let _ = if args.skip_updates {
        tracing::info!("--skip-updates flag passed, skipping update check.");
        None
    } else {
        tracing::info!("Checking for updates...");
        match start_updater(
            &args.updater_repo_owner,
            &args.updater_repo_name,
            args.updater_repo_private_key.as_deref(),
            &get_verifying_key(RELEASE_PUBLIC_KEY),
        ) {
            Ok(opt) => opt,
            Err(LauncherError::RestartRequired) => {
                wait_for_exit();
            }
            Err(err) => {
                tracing::error!("Updater failed: {err}");
                wait_for_exit();
            }
        }
    };

    tracing::info!("Starting Elden Ring...");

    if let Err(err) = start_game(
        &args.content_dir,
        &args.dll_name,
        &args.game_executable,
        args.game_dir.as_ref(),
        args.debug,
    ) {
        tracing::error!("Failed to start Elden Ring: {}", err);
        wait_for_exit();
    } else {
        tracing::info!("Elden Ring started successfully!");
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}
