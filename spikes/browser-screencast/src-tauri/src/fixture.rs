//! A tiny HTTP server for the fixture pages, so every test and measurement runs against
//! local content that this spike serves itself (no external sites).
//!
//! Besides the static pages it has two cookie routes: `/set-cookie?v=X` sends a real
//! `Set-Cookie` header (network-stack cookie jar) and `/echo-cookie` returns the
//! `Cookie` header the request carried.

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const PAGES: &[(&str, &str)] = &[
    ("/", include_str!("../fixtures/index.html")),
    ("/animation.html", include_str!("../fixtures/animation.html")),
    ("/click.html", include_str!("../fixtures/click.html")),
    ("/form.html", include_str!("../fixtures/form.html")),
    ("/state.html", include_str!("../fixtures/state.html")),
    ("/scroll.html", include_str!("../fixtures/scroll.html")),
];

pub struct Fixtures {
    pub port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Fixtures {
    /// Binds `127.0.0.1:0` (loopback only) and serves until dropped.
    pub async fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let _ = serve(stream).await;
                });
            }
        });
        Ok(Self { port, task })
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let target = head.lines().next().and_then(|l| l.split(' ').nth(1)).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let cookie = head
        .lines()
        .find_map(|l| l.strip_prefix("Cookie: ").or_else(|| l.strip_prefix("cookie: ")))
        .unwrap_or("")
        .to_string();

    let (status, ctype, extra, body) = match path {
        "/set-cookie" => {
            let v = query.strip_prefix("v=").unwrap_or("x");
            ("200 OK", "text/plain", format!("Set-Cookie: srv={v}; Path=/\r\n"), format!("set srv={v}"))
        }
        "/echo-cookie" => ("200 OK", "text/plain", String::new(), cookie),
        "/favicon.ico" => ("204 No Content", "text/plain", String::new(), String::new()),
        p => match PAGES.iter().find(|(route, _)| *route == p) {
            Some((_, html)) => ("200 OK", "text/html; charset=utf-8", String::new(), (*html).to_string()),
            None => ("404 Not Found", "text/plain", String::new(), "not found".into()),
        },
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}
