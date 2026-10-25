//! Binary layout of one screencast frame on the `tauri::ipc::Channel`.
//!
//! Little-endian, fixed 52-byte header followed by the encoded image (JPEG or PNG):
//!
//! | offset | type | field |
//! |---|---|---|
//! | 0 | u32 | `ack_id`: pass back to `pane_ack` |
//! | 4 | u32 | context index |
//! | 8 | u32 | sequence number within this stream |
//! | 12 | u32 | flags (bit 0: PNG, else JPEG) |
//! | 16 | f32 | device width in CSS px |
//! | 20 | f32 | device height in CSS px |
//! | 24 | f64 | Chromium timestamp (seconds since epoch) |
//! | 32 | f64 | backend received the CDP event (ms since epoch) |
//! | 40 | f64 | backend sent the packet to the channel (ms since epoch) |
//! | 48 | u32 | stream generation: which `pane_attach` produced this frame; acks must echo it |

pub const HEADER_LEN: usize = 52;
pub const FLAG_PNG: u32 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub ack_id: u32,
    pub context: u32,
    pub seq: u32,
    pub flags: u32,
    pub device_width: f32,
    pub device_height: f32,
    pub cdp_timestamp: f64,
    pub rx_epoch_ms: f64,
    pub sent_epoch_ms: f64,
    pub stream: u32,
}

pub fn encode(h: &Header, image: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + image.len());
    out.extend_from_slice(&h.ack_id.to_le_bytes());
    out.extend_from_slice(&h.context.to_le_bytes());
    out.extend_from_slice(&h.seq.to_le_bytes());
    out.extend_from_slice(&h.flags.to_le_bytes());
    out.extend_from_slice(&h.device_width.to_le_bytes());
    out.extend_from_slice(&h.device_height.to_le_bytes());
    out.extend_from_slice(&h.cdp_timestamp.to_le_bytes());
    out.extend_from_slice(&h.rx_epoch_ms.to_le_bytes());
    out.extend_from_slice(&h.sent_epoch_ms.to_le_bytes());
    out.extend_from_slice(&h.stream.to_le_bytes());
    out.extend_from_slice(image);
    out
}

pub fn decode(buf: &[u8]) -> Option<(Header, &[u8])> {
    if buf.len() < HEADER_LEN {
        return None;
    }
    let u = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let f = |o: usize| f32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let d = |o: usize| f64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
    Some((
        Header {
            ack_id: u(0),
            context: u(4),
            seq: u(8),
            flags: u(12),
            device_width: f(16),
            device_height: f(20),
            cdp_timestamp: d(24),
            rx_epoch_ms: d(32),
            sent_epoch_ms: d(40),
            stream: u(48),
        },
        &buf[HEADER_LEN..],
    ))
}

#[cfg(test)]
const GOLDEN_HEADER_HEX: &str = "070000000100000063000000010000000010a044000034443bdf0740fc54d94100b88756febc784200c48756febc784203000000";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_keeps_the_image_bytes() {
        let h = Header {
            ack_id: 7,
            context: 1,
            seq: 99,
            flags: FLAG_PNG,
            device_width: 1280.5,
            device_height: 720.0,
            cdp_timestamp: 1_700_000_000.123,
            rx_epoch_ms: 1_700_000_000_123.5,
            sent_epoch_ms: 1_700_000_000_124.25,
            stream: 3,
        };
        let image = [0xff, 0xd8, 1, 2, 3];
        let wire = encode(&h, &image);
        assert_eq!(wire.len(), HEADER_LEN + image.len());
        let (back, rest) = decode(&wire).unwrap();
        assert_eq!(back, h);
        assert_eq!(rest, image);
        assert!(decode(&wire[..HEADER_LEN - 1]).is_none());
        // Golden bytes, mirrored by src/pane/frame.test.ts so both sides agree on the layout.
        let hex: String = wire[..HEADER_LEN].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, GOLDEN_HEADER_HEX);
    }
}
