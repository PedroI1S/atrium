//! A small Chrome DevTools Protocol client over `--remote-debugging-pipe`.
//!
//! Wire format: every message is one JSON object terminated by a single NUL byte, in both
//! directions. Chromium reads commands from fd 3 and writes responses and events to fd 4.
//! `Cdp::new` takes any `Read` / `Write` pair so the framing is testable without a browser.
//!
//! Sessions use the "flat" mode: commands for a page carry a top-level `sessionId`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use tokio::sync::{broadcast, oneshot};

pub fn epoch_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

#[derive(Debug, Clone, Deserialize)]
pub struct CdpErrorBody {
    pub code: i64,
    pub message: String,
}

#[derive(Debug)]
pub enum CdpError {
    /// The browser answered with a protocol error.
    Protocol { method: String, error: CdpErrorBody },
    /// The pipe closed (browser exited) before an answer arrived.
    Closed,
    Timeout(String),
    Serde(String),
    Io(String),
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol { method, error } => write!(f, "{method}: {} ({})", error.message, error.code),
            Self::Closed => write!(f, "CDP pipe closed"),
            Self::Timeout(m) => write!(f, "{m}: timed out"),
            Self::Serde(e) => write!(f, "bad CDP json: {e}"),
            Self::Io(e) => write!(f, "CDP io: {e}"),
        }
    }
}
impl std::error::Error for CdpError {}

/// An event from the browser. `params` is kept as raw JSON so a 130 KB screencast frame
/// is copied once, not re-built into a `Value` tree.
#[derive(Debug)]
pub struct CdpEvent {
    pub method: String,
    pub session_id: Option<String>,
    pub params: Box<RawValue>,
    /// Wall clock (ms since the Unix epoch) when the reader thread got the message.
    pub received_epoch_ms: f64,
}

impl CdpEvent {
    pub fn params<T: DeserializeOwned>(&self) -> Result<T, CdpError> {
        serde_json::from_str(self.params.get()).map_err(|e| CdpError::Serde(e.to_string()))
    }
}

#[derive(Deserialize)]
struct Incoming<'a> {
    id: Option<u64>,
    method: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(borrow)]
    params: Option<&'a RawValue>,
    #[serde(borrow)]
    result: Option<&'a RawValue>,
    error: Option<CdpErrorBody>,
}

type Pending = Mutex<HashMap<u64, (String, oneshot::Sender<Result<Box<RawValue>, CdpError>>)>>;

struct Inner {
    next_id: AtomicU64,
    pending: Pending,
    writer: Mutex<Box<dyn Write + Send>>,
    /// Dropped at EOF so every subscriber's `recv()` returns `Closed` instead of waiting forever.
    events: Mutex<Option<broadcast::Sender<Arc<CdpEvent>>>>,
    closed: std::sync::atomic::AtomicBool,
}

#[derive(Clone)]
pub struct Cdp {
    inner: Arc<Inner>,
}

#[derive(Serialize)]
struct Outgoing<'a, P: Serialize> {
    id: u64,
    method: &'a str,
    params: P,
    #[serde(rename = "sessionId", skip_serializing_if = "Option::is_none")]
    session_id: Option<&'a str>,
}

impl Cdp {
    /// Starts the reader thread. `reader` yields NUL-terminated JSON messages; when it hits
    /// EOF every pending call fails with `Closed`.
    pub fn new(reader: impl Read + Send + 'static, writer: impl Write + Send + 'static) -> Self {
        let (events, _) = broadcast::channel(1024);
        let inner = Arc::new(Inner {
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            writer: Mutex::new(Box::new(writer)),
            events: Mutex::new(Some(events)),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        let thread_inner = inner.clone();
        std::thread::Builder::new()
            .name("cdp-reader".into())
            .spawn(move || read_loop(reader, &thread_inner))
            .expect("spawn cdp reader");
        Self { inner }
    }

    /// A receiver for browser events. After the connection closed it is already closed
    /// (`recv()` returns `Closed` at once), so consumers always terminate.
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<CdpEvent>> {
        match self.inner.events.lock().unwrap().as_ref() {
            Some(tx) => tx.subscribe(),
            None => broadcast::channel(1).1,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Relaxed)
    }

    /// Sends a command and waits for its result (30 s cap).
    pub async fn call_raw<P: Serialize>(
        &self,
        method: &str,
        params: P,
        session_id: Option<&str>,
    ) -> Result<Box<RawValue>, CdpError> {
        let (rx, _) = self.send(method, params, session_id)?;
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Err(_) => Err(CdpError::Timeout(method.to_string())),
            Ok(Err(_)) => Err(CdpError::Closed),
            Ok(Ok(r)) => r,
        }
    }

    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
        session_id: Option<&str>,
    ) -> Result<R, CdpError> {
        let raw = self.call_raw(method, params, session_id).await?;
        serde_json::from_str(raw.get()).map_err(|e| CdpError::Serde(format!("{method}: {e}")))
    }

    /// Fire-and-forget variant used for acks: the write happens now, the answer is dropped.
    pub fn notify<P: Serialize>(&self, method: &str, params: P, session_id: Option<&str>) -> Result<(), CdpError> {
        let (_rx, id) = self.send(method, params, session_id)?;
        self.inner.pending.lock().unwrap().remove(&id);
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    fn send<P: Serialize>(
        &self,
        method: &str,
        params: P,
        session_id: Option<&str>,
    ) -> Result<(oneshot::Receiver<Result<Box<RawValue>, CdpError>>, u64), CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let mut bytes = serde_json::to_vec(&Outgoing { id, method, params, session_id })
            .map_err(|e| CdpError::Serde(e.to_string()))?;
        bytes.push(0);
        let (tx, rx) = oneshot::channel();
        self.inner.pending.lock().unwrap().insert(id, (method.to_string(), tx));
        let write = {
            let mut w = self.inner.writer.lock().unwrap();
            w.write_all(&bytes).and_then(|()| w.flush())
        };
        if let Err(e) = write {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(CdpError::Io(e.to_string()));
        }
        Ok((rx, id))
    }
}

fn read_loop(mut reader: impl Read, inner: &Arc<Inner>) {
    let mut buf = vec![0u8; 256 * 1024];
    let mut acc: Vec<u8> = Vec::with_capacity(512 * 1024);
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut rest = &buf[..n];
        while let Some(pos) = rest.iter().position(|&b| b == 0) {
            acc.extend_from_slice(&rest[..pos]);
            if !acc.is_empty() {
                dispatch(&acc, inner);
            }
            acc.clear();
            rest = &rest[pos + 1..];
        }
        acc.extend_from_slice(rest);
    }
    inner.closed.store(true, Ordering::Relaxed);
    inner.events.lock().unwrap().take(); // closes every event subscriber
    let drained: Vec<_> = inner.pending.lock().unwrap().drain().collect();
    for (_, (_, tx)) in drained {
        let _ = tx.send(Err(CdpError::Closed));
    }
}

fn dispatch(msg: &[u8], inner: &Inner) {
    let received = epoch_ms();
    let Ok(m) = serde_json::from_slice::<Incoming<'_>>(msg) else { return };
    if let Some(id) = m.id {
        let Some((method, tx)) = inner.pending.lock().unwrap().remove(&id) else { return };
        let outcome = match (m.error, m.result) {
            (Some(error), _) => Err(CdpError::Protocol { method, error }),
            (None, Some(r)) => Ok(r.to_owned()),
            (None, None) => Ok(RawValue::from_string("{}".into()).unwrap()),
        };
        let _ = tx.send(outcome);
    } else if let (Some(method), Some(params)) = (m.method, m.params) {
        // No subscribers is fine; the event is simply dropped.
        if let Some(tx) = inner.events.lock().unwrap().as_ref() {
            let _ = tx.send(Arc::new(CdpEvent {
                method,
                session_id: m.session_id,
                params: params.to_owned(),
                received_epoch_ms: received,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// A fake browser on the other end of a socket pair that answers `Echo.*` calls,
    /// fails `Fail.*` calls, and emits one event before each answer.
    fn fake_browser(server: UnixStream) {
        std::thread::spawn(move || {
            let mut s = server;
            let mut acc = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let Ok(n) = s.read(&mut buf) else { return };
                if n == 0 {
                    return;
                }
                acc.extend_from_slice(&buf[..n]);
                while let Some(p) = acc.iter().position(|&b| b == 0) {
                    let line: Vec<u8> = acc.drain(..=p).collect();
                    let v: serde_json::Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
                    let id = v["id"].as_u64().unwrap();
                    let method = v["method"].as_str().unwrap();
                    let event = serde_json::json!({"method":"Test.event","params":{"for":id},"sessionId":"S1"});
                    let reply = if method.starts_with("Fail.") {
                        serde_json::json!({"id":id,"error":{"code":-32000,"message":"nope"}})
                    } else {
                        serde_json::json!({"id":id,"result":{"echo":v["params"],"session":v["sessionId"]}})
                    };
                    // Deliver the event and the reply in one write, split mid-message,
                    // to exercise reassembly across reads.
                    let mut out = serde_json::to_vec(&event).unwrap();
                    out.push(0);
                    out.extend(serde_json::to_vec(&reply).unwrap());
                    out.push(0);
                    let mid = out.len() / 2;
                    s.write_all(&out[..mid]).unwrap();
                    s.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(2));
                    s.write_all(&out[mid..]).unwrap();
                }
            }
        });
    }

    #[tokio::test]
    async fn frames_commands_results_errors_and_events() {
        let (client, server) = UnixStream::pair().unwrap();
        fake_browser(server);
        let cdp = Cdp::new(client.try_clone().unwrap(), client);
        let mut events = cdp.subscribe();

        let r: serde_json::Value = cdp.call("Echo.hi", serde_json::json!({"a": 1}), Some("S1")).await.unwrap();
        assert_eq!(r["echo"]["a"], 1);
        assert_eq!(r["session"], "S1");

        let ev = events.recv().await.unwrap();
        assert_eq!(ev.method, "Test.event");
        assert_eq!(ev.session_id.as_deref(), Some("S1"));
        assert_eq!(ev.params::<serde_json::Value>().unwrap()["for"], 1);

        let err = cdp.call_raw("Fail.now", serde_json::json!({}), None).await.unwrap_err();
        assert!(matches!(err, CdpError::Protocol { ref error, .. } if error.message == "nope"), "{err}");

        // Concurrent calls get their own answers.
        let (a, b) = tokio::join!(
            cdp.call::<_, serde_json::Value>("Echo.x", serde_json::json!({"n": "a"}), None),
            cdp.call::<_, serde_json::Value>("Echo.x", serde_json::json!({"n": "b"}), None),
        );
        assert_eq!(a.unwrap()["echo"]["n"], "a");
        assert_eq!(b.unwrap()["echo"]["n"], "b");
    }

    #[tokio::test]
    async fn eof_fails_pending_calls_and_marks_closed() {
        let (client, server) = UnixStream::pair().unwrap();
        let cdp = Cdp::new(client.try_clone().unwrap(), client);
        let call = tokio::spawn({
            let cdp = cdp.clone();
            async move { cdp.call_raw("Never.answers", serde_json::json!({}), None).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(server);
        assert!(matches!(call.await.unwrap(), Err(CdpError::Closed)));
        assert!(cdp.is_closed());
        assert!(matches!(cdp.call_raw("After.close", serde_json::json!({}), None).await, Err(CdpError::Closed)));
    }

    #[tokio::test]
    async fn eof_closes_event_subscribers_waiting_and_late() {
        let (client, server) = UnixStream::pair().unwrap();
        let cdp = Cdp::new(client.try_clone().unwrap(), client);
        let mut waiting = cdp.subscribe();
        let waiter = tokio::spawn(async move { waiting.recv().await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(server);
        let r = tokio::time::timeout(Duration::from_secs(2), waiter).await.expect("waiter must wake").unwrap();
        assert!(matches!(r, Err(broadcast::error::RecvError::Closed)));
        let mut late = cdp.subscribe();
        assert!(matches!(late.recv().await, Err(broadcast::error::RecvError::Closed)));
    }
}
