//! Cancellation: keep a read future alive until it completes; cancelling it can
//! consume part of a frame. The daemon uses a dedicated reader task.
use crate::{
    wire::{FrameKind, MAX_FRAME_LEN},
};
use serde::{Serialize, de::DeserializeOwned};
use std::io;
fn invalid(e: impl std::fmt::Display) -> io::Error { io::Error::new(io::ErrorKind::InvalidData,e.to_string()) }
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[derive(Debug, PartialEq)]
pub enum Frame<T> {
    Control(T),
    Stream { stream_id: u32, bytes: Vec<u8> },
}
pub async fn read<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> io::Result<Frame<T>> {
    let len = r.read_u32().await?;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(invalid("invalid frame length"));
    }
    let kind = FrameKind::try_from(r.read_u8().await?).map_err(|_| invalid("unknown frame kind"))?;
    let mut payload = vec![0; (len - 1) as usize];
    r.read_exact(&mut payload).await?;
    match kind {
        FrameKind::Control => Ok(Frame::Control(
            serde_json::from_slice(&payload).map_err(invalid)?,
        )),
        FrameKind::Stream if payload.len() >= 4 => Ok(Frame::Stream {
            stream_id: u32::from_be_bytes(payload[..4].try_into().map_err(invalid)?),
            bytes: payload[4..].to_vec(),
        }),
        _ => Err(invalid("short stream frame")),
    }
}
pub async fn write<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    frame: &Frame<T>,
) -> io::Result<()> {
    let mut bytes = vec![0; 5];
    match frame {
        Frame::Control(value) => { bytes[4] = 0; serde_json::to_writer(&mut bytes, value).map_err(invalid)?; }
        Frame::Stream { stream_id, bytes: payload } => {
            if payload.len() > MAX_FRAME_LEN as usize - 5 { return Err(invalid("oversized frame")); }
            bytes[4] = 1; bytes.extend_from_slice(&stream_id.to_be_bytes()); bytes.extend_from_slice(payload);
        }
    }
    let len = bytes.len() - 4;
    if len > MAX_FRAME_LEN as usize { return Err(invalid("oversized frame")); }
    bytes[..4].copy_from_slice(&(len as u32).to_be_bytes());
    w.write_all(&bytes).await?;
    w.flush().await
}
