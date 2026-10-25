use spike_daemon_socket::daemon;
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("spike-daemon: {e}");
        std::process::exit(1);
    }
}
async fn run() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = match args.next().as_deref() {
        None => daemon::default_dir(),
        Some("--dir") => args
            .next()
            .ok_or_else(|| std::io::Error::other("--dir needs a path"))?
            .into(),
        _ => return Err(std::io::Error::other("usage: spike-daemon [--dir PATH]")),
    };
    let fake = std::env::current_exe()?.with_file_name("spike-fake-agent");
    daemon::run(&dir, fake).await
}
