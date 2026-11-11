//! `atriumd`: see the library's docs.
//!
//! ```text
//! atriumd [--dev-socket]
//! ```
//!
//! - `--dev-socket`: accept `Dev` connections (tests and harnesses). Only a
//!   debug build honours it.
//! - `SIGTERM`: exit once no turn is running. `SIGINT`: stop sessions now
//!   (resumable) and exit.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use atrium_daemon::{AppIdentitySource, CommandNotifier, DAEMON_VERSION, Daemon, Options};
use atrium_host::lifecycle::default_paths;
use tokio::signal::unix::{SignalKind, signal};

const USAGE: &str = "usage: atriumd [--dev-socket] [--version]";

/// A few workers are plenty: the daemon mostly waits, and each worker's
/// stack and allocator arena count against its idle memory (contract §14).
const WORKER_THREADS: usize = 2;

fn main() -> ExitCode {
    let mut dev_socket = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--dev-socket" => dev_socket = true,
            "--version" => {
                println!("atriumd {DAEMON_VERSION}");
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("atriumd: unknown argument {other:?}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if dev_socket && !cfg!(debug_assertions) {
        eprintln!("atriumd: --dev-socket is ignored by release builds");
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("atriumd: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(dev_socket)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("atriumd: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(dev_socket: bool) -> Result<(), Box<dyn std::error::Error>> {
    let paths = default_paths()?;
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let mut options = Options::new(paths.clone(), home);
    options.dev_socket = dev_socket;
    // Release daemons get the app's identity compiled in when the app is
    // packaged; until then only development builds know one, from the file
    // the dev app writes.
    options.app_identity = if cfg!(debug_assertions) {
        AppIdentitySource::File(paths.app_identity)
    } else {
        AppIdentitySource::None
    };
    options.notifier = Arc::new(CommandNotifier);
    options.manage_service = true;

    // Installed before the daemon starts, so a signal sent right after the
    // socket appears is never the default action.
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut daemon = Daemon::start(options).await?;
    loop {
        tokio::select! {
            () = daemon.served() => break,
            _ = terminate.recv() => daemon.stop(false),
            _ = interrupt.recv() => daemon.stop(true),
        }
    }
    daemon.stopped().await;
    Ok(())
}
