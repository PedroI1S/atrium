// Parses the binary screencast packet from the Rust side (layout in src-tauri/src/packet.rs).

export const HEADER_LEN = 52;
export const FLAG_PNG = 1;

export interface FrameHeader {
  ackId: number;
  context: number;
  seq: number;
  flags: number;
  deviceWidth: number;
  deviceHeight: number;
  /** Chromium's timestamp, seconds since the epoch. */
  cdpTimestamp: number;
  /** Backend received the CDP event (ms since the epoch). */
  rxEpochMs: number;
  /** Backend wrote the packet to the channel (ms since the epoch). */
  sentEpochMs: number;
  /** Which `pane_attach` produced the frame; echoed back in the ack. */
  stream: number;
}

export interface Frame {
  header: FrameHeader;
  image: Uint8Array<ArrayBuffer>;
  mime: "image/jpeg" | "image/png";
}

export function parseFrame(buf: ArrayBuffer): Frame | null {
  if (buf.byteLength <= HEADER_LEN) return null;
  const v = new DataView(buf);
  const flags = v.getUint32(12, true);
  return {
    header: {
      ackId: v.getUint32(0, true),
      context: v.getUint32(4, true),
      seq: v.getUint32(8, true),
      flags,
      deviceWidth: v.getFloat32(16, true),
      deviceHeight: v.getFloat32(20, true),
      cdpTimestamp: v.getFloat64(24, true),
      rxEpochMs: v.getFloat64(32, true),
      sentEpochMs: v.getFloat64(40, true),
      stream: v.getUint32(48, true),
    },
    image: new Uint8Array(buf, HEADER_LEN),
    mime: flags & FLAG_PNG ? "image/png" : "image/jpeg",
  };
}
