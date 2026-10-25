//! Replays a recorded fixture through `AcpNormalizer` and prints the events.
//!
//! ```text
//! cargo run -p spike-agent-acp --example replay -- <fixture.jsonl>           # print events
//! cargo run -p spike-agent-acp --example replay -- --write <fixture.jsonl>   # write <name>.events.json next to it
//! ```
//!
//! Useful to check a live recording offline, and to regenerate the golden file
//! of a hand-made fixture after you have read the printed events.

use std::env;
use std::path::PathBuf;

use atrium_agents::fixture;
use spike_agent_acp::AcpNormalizer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let write = args.iter().any(|a| a == "--write");
    args.retain(|a| a != "--write");
    let Some(path) = args.first().map(PathBuf::from) else {
        eprintln!("Usage: replay [--write] <fixture.jsonl>");
        std::process::exit(2);
    };
    let lines = fixture::read(&path)?;
    let events = fixture::normalize(&lines, &mut AcpNormalizer::new());
    let json = serde_json::to_string_pretty(&events)? + "\n";
    if write {
        let out = path.with_extension("events.json");
        std::fs::write(&out, &json)?;
        println!("wrote {} events to {}", events.len(), out.display());
    } else {
        print!("{json}");
    }
    Ok(())
}
