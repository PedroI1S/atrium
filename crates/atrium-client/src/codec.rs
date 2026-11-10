//! Frames on the daemon socket (`atrium_protocol::wire`, "Framing").

use std::io;

use atrium_protocol::limits::MAX_CLIENT_FRAME_LEN;
use atrium_protocol::wire::{ClientMessage, FrameKind, MAX_FRAME_LEN};
use tokio::io::{AsyncRead, AsyncReadExt};

/// One encoded frame, ready to write.
pub(crate) type Frame = Vec<u8>;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

/// Reads one control frame's JSON payload.
///
/// Not cancellation-safe: dropping the future can leave half a frame read.
/// The connection reads from one task that is only dropped with the
/// connection.
///
/// `InvalidData` for a frame that is empty, longer than `MAX_FRAME_LEN`, of
/// an unknown kind, or a stream frame (M1 announces no streams).
pub(crate) async fn read_control<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let len = reader.read_u32().await?;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(invalid("frame length out of range"));
    }
    match FrameKind::try_from(reader.read_u8().await?) {
        Ok(FrameKind::Control) => {}
        Ok(FrameKind::Stream) => return Err(invalid("stream frame without an announced stream")),
        Err(_) => return Err(invalid("unknown frame kind")),
    }
    let mut payload = vec![0; len as usize - 1];
    reader.read_exact(&mut payload).await?;
    Ok(payload)
}

/// The message as a control frame. `Err` with the frame's size when it is
/// over `MAX_CLIENT_FRAME_LEN`: the daemon would close the connection.
pub(crate) fn encode(message: &ClientMessage) -> Result<Frame, usize> {
    let mut frame = vec![0, 0, 0, 0, FrameKind::Control as u8];
    // A `ClientMessage` always serializes: its maps have string keys.
    if serde_json::to_writer(&mut frame, message).is_err() {
        return Err(usize::MAX);
    }
    let len = frame.len() - 4;
    if len > MAX_CLIENT_FRAME_LEN as usize {
        return Err(len);
    }
    frame[..4].copy_from_slice(&(len as u32).to_be_bytes());
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_protocol::wire::{DaemonCommand, ThreadId};

    #[tokio::test]
    async fn a_frame_round_trips() {
        let message = ClientMessage::Unsubscribe {
            thread_id: ThreadId("t".into()),
        };
        let frame = encode(&message).expect("small");
        assert_eq!(frame[4], 0);
        assert_eq!(
            u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize,
            frame.len() - 4
        );
        let payload = read_control(&mut frame.as_slice()).await.expect("reads");
        assert_eq!(
            serde_json::from_slice::<ClientMessage>(&payload).expect("parses"),
            message
        );
    }

    #[tokio::test]
    async fn bad_frames_are_invalid_data() {
        let cases: [&[u8]; 4] = [
            &[0, 0, 0, 0],
            &[0xff, 0xff, 0xff, 0xff, 0],
            &[0, 0, 0, 2, 9, b'x'],
            &[0, 0, 0, 6, 1, 0, 0, 0, 1, b'x'],
        ];
        for mut bytes in cases {
            let error = read_control(&mut bytes).await.expect_err("invalid");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
        let mut short: &[u8] = &[0, 0, 0, 9, 0, b'{'];
        let error = read_control(&mut short).await.expect_err("cut off");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_command_over_the_client_limit_is_refused() {
        let message = ClientMessage::Command {
            request_id: 1,
            command: DaemonCommand::RenameThread {
                thread_id: ThreadId("t".into()),
                title: "x".repeat(MAX_CLIENT_FRAME_LEN as usize),
            },
        };
        assert!(encode(&message).is_err_and(|len| len > MAX_CLIENT_FRAME_LEN as usize));
    }
}
