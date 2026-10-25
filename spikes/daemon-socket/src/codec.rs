//! Cancellation: keep a read future alive until it completes; cancelling it can
//! consume part of a frame. The daemon uses a dedicated reader task.
use crate::{
    error,
    wire::{FrameKind, MAX_FRAME_LEN},
};
use serde::{Serialize, de::DeserializeOwned};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
#[derive(Debug, PartialEq)]
pub enum Frame<T> {
    Control(T),
    Stream { stream_id: u32, bytes: Vec<u8> },
}
pub async fn read<R: AsyncRead + Unpin, T: DeserializeOwned>(r: &mut R) -> io::Result<Frame<T>> {
    let len = r.read_u32().await?;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(error("invalid frame length"));
    }
    let kind = FrameKind::try_from(r.read_u8().await?).map_err(|_| error("unknown frame kind"))?;
    let mut payload = vec![0; (len - 1) as usize];
    r.read_exact(&mut payload).await?;
    match kind {
        FrameKind::Control => Ok(Frame::Control(
            serde_json::from_slice(&payload).map_err(error)?,
        )),
        FrameKind::Stream if payload.len() >= 4 => Ok(Frame::Stream {
            stream_id: u32::from_be_bytes(payload[..4].try_into().map_err(error)?),
            bytes: payload[4..].to_vec(),
        }),
        _ => Err(error("short stream frame")),
    }
}
pub async fn write<W: AsyncWrite + Unpin, T: Serialize>(
    w: &mut W,
    frame: &Frame<T>,
) -> io::Result<()> {
    let (kind, payload) = match frame {
        Frame::Control(v) => (0, serde_json::to_vec(v).map_err(error)?),
        Frame::Stream { stream_id, bytes } => {
            let mut p = stream_id.to_be_bytes().to_vec();
            p.extend(bytes);
            (1, p)
        }
    };
    if payload.len() >= MAX_FRAME_LEN as usize {
        return Err(error("oversized frame"));
    }
    w.write_u32((payload.len() + 1) as u32).await?;
    w.write_u8(kind).await?;
    w.write_all(&payload).await?;
    w.flush().await
}
