//! Frames on the socket (`atrium_protocol::wire`, "Framing").

use std::io;
use std::sync::Arc;

use atrium_protocol::limits::MAX_CLIENT_FRAME_LEN;
use atrium_protocol::wire::{FrameKind, MAX_FRAME_LEN, ServerMessage};
use tokio::io::{AsyncRead, AsyncReadExt};

/// One encoded frame, shared by every connection it is queued for.
pub(crate) type Frame = Arc<[u8]>;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Reads one client frame's JSON payload. `InvalidData` for a frame that is
/// empty, over `MAX_CLIENT_FRAME_LEN`, of an unknown kind, or a stream frame
/// (M1 clients send none): the connection is closed.
pub(crate) async fn read_client<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let len = reader.read_u32().await?;
    if len == 0 || len > MAX_CLIENT_FRAME_LEN {
        return Err(invalid("frame length out of range"));
    }
    match FrameKind::try_from(reader.read_u8().await?) {
        Ok(FrameKind::Control) => {}
        Ok(FrameKind::Stream) => return Err(invalid("stream frames aren't accepted")),
        Err(_) => return Err(invalid("unknown frame kind")),
    }
    let mut payload = vec![0; len as usize - 1];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

/// The message as a control frame. `None` when it is over `MAX_FRAME_LEN`
/// (the client would close the connection) or doesn't serialize.
pub(crate) fn encode(message: &ServerMessage) -> Option<Frame> {
    let mut frame = vec![0, 0, 0, 0, FrameKind::Control as u8];
    serde_json::to_writer(&mut frame, message).ok()?;
    let len = frame.len() - 4;
    if len > MAX_FRAME_LEN as usize {
        return None;
    }
    frame[..4].copy_from_slice(&(len as u32).to_be_bytes());
    Some(frame.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_protocol::thread::ThreadId;

    #[tokio::test]
    async fn client_frames_are_bounded_and_typed() {
        let cases: [&[u8]; 4] = [
            &[0, 0, 0, 0],
            &[0, 0x10, 0, 1, 0],
            &[0, 0, 0, 2, 9, b'x'],
            &[0, 0, 0, 6, 1, 0, 0, 0, 1, b'x'],
        ];
        for mut bytes in cases {
            let error = read_client(&mut bytes).await.expect_err("invalid");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
        let mut good: &[u8] = &[0, 0, 0, 3, 0, b'{', b'}'];
        assert_eq!(read_client(&mut good).await.expect("reads"), b"{}");
    }

    #[test]
    fn a_server_frame_has_its_length_and_kind() {
        let frame = encode(&ServerMessage::ThreadRemoved {
            thread_id: ThreadId("t".into()),
        })
        .expect("small");
        assert_eq!(frame[4], 0);
        assert_eq!(
            u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize,
            frame.len() - 4
        );
    }
}
