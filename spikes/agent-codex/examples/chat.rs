use atrium_agents::*;
use atrium_protocol::agent::*;
use spike_agent_codex::CodexAdapter;
use std::sync::Arc;

struct Ask;
impl ToolGate for Ask {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { GateDecision::Ask })
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = SessionConfig {
        cwd: std::env::current_dir()?,
        model: std::env::var("CODEX_MODEL").ok(),
        effort: None,
        permissions: PermissionPolicy::ReadOnly,
        gate: Arc::new(Ask),
        mcp_servers: vec![],
        resume: std::env::var("CODEX_RESUME").ok(),
        env: vec![],
        program: None,
    };
    let mut session = CodexAdapter::default()
        .start(config, Transport::Local)
        .await?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::stdin().lock().lines() {
            if tx.blocking_send(line).is_err() {
                return;
            }
        }
    });
    eprintln!(
        "Enter a prompt, /interrupt, /quit, or /allow ID, /deny ID. Questions accept SessionCommand JSON."
    );
    loop {
        tokio::select! {
            event=session.events.recv()=>match event {Some(e)=>println!("{}",serde_json::to_string(&e)?),None=>break},
            line=rx.recv()=> {
                let cmd=match line {
                    None=>SessionCommand::Shutdown,
                    Some(line)=>{
                        let line=line?;
                        if line=="/quit" {SessionCommand::Shutdown}else if line=="/interrupt"{SessionCommand::Interrupt}
                        else if let Some(id)=line.strip_prefix("/allow "){SessionCommand::ResolveApproval {request_id:id.into(),decision:ApprovalDecision::Allow}}
                        else if let Some(id)=line.strip_prefix("/deny "){SessionCommand::ResolveApproval {request_id:id.into(),decision:ApprovalDecision::Deny {message:None}}}
                        else if line.starts_with('{'){serde_json::from_str(&line)?}else{SessionCommand::SendUserMessage {text:line}}
                    }
                };
                let shutdown=matches!(cmd,SessionCommand::Shutdown);session.commands.send(cmd).await?;
                if shutdown {while let Some(e)=session.events.recv().await {println!("{}",serde_json::to_string(&e)?);}break;}
            }
        }
    }
    Ok(())
}
