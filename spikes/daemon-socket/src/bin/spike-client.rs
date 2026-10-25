use spike_daemon_socket::{Client, daemon, wire::*};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = match args.next().as_deref() {
        None => daemon::default_dir(),
        Some("--dir") => args.next().expect("--dir needs path").into(),
        _ => panic!("usage: spike-client [--dir PATH]"),
    };
    let executable = std::env::current_exe()?.with_file_name("spike-daemon");
    let mut client = Client::connect_or_spawn(&dir, executable, "spike-client").await?;
    let result = client
        .command(DaemonCommand::StartThread {
            agent: AgentLaunch::Fake {
                tokens_per_second: 100,
                chunk_chars: 200,
                duration_ms: 300_000,
                tool_call_every_ms: 2000,
            },
            cwd: std::env::current_dir()?.to_string_lossy().into_owned(),
        })
        .await?;
    let thread = match result {
        CommandResult::Ok { value } => {
            ThreadId(value["thread_id"].as_str().ok_or("missing thread")?.into())
        }
        CommandResult::Err { message } => return Err(message.into()),
    };
    client.subscribe(thread.clone(), None).await?;
    eprintln!(
        "following {}; Ctrl-C detaches (agent keeps running)",
        thread.0
    );
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);
    loop {
        tokio::select! {
            _ = &mut interrupt => break,
            msg = client.next() => if let ServerMessage::Event { event } = msg? { println!("{}", serde_json::to_string(&event)?); client.applied(&thread, event.id); }
        }
    }
    Ok(())
}
