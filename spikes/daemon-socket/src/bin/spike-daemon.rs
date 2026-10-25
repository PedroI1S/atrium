use spike_daemon_socket::daemon;
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = match args.next().as_deref() {
        None => daemon::default_dir(),
        Some("--dir") => args.next().expect("--dir needs a path").into(),
        _ => panic!("usage: spike-daemon [--dir PATH]"),
    };
    let fake = std::env::current_exe()?.with_file_name("spike-fake-agent");
    daemon::run(&dir, fake).await
}
