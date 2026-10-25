import { describe, expect, it } from "vitest";
import { HEADER_LEN, parseFrame } from "./frame";

// Same bytes as GOLDEN_HEADER_HEX in src-tauri/src/packet.rs.
const GOLDEN =
  "070000000100000063000000010000000010a044000034443bdf0740fc54d94100b88756febc784200c48756febc784203000000";

function bytes(hex: string): Uint8Array {
  return Uint8Array.from(hex.match(/../g)!.map((h) => parseInt(h, 16)));
}

describe("parseFrame", () => {
  it("reads the header the Rust side wrote and exposes the image bytes", () => {
    const header = bytes(GOLDEN);
    expect(header.length).toBe(HEADER_LEN);
    const image = Uint8Array.from([0xff, 0xd8, 1, 2, 3]);
    const wire = new Uint8Array(header.length + image.length);
    wire.set(header);
    wire.set(image, header.length);
    const f = parseFrame(wire.buffer)!;
    expect(f.header).toEqual({
      ackId: 7,
      context: 1,
      seq: 99,
      flags: 1,
      deviceWidth: 1280.5,
      deviceHeight: 720,
      cdpTimestamp: 1700000000.123,
      rxEpochMs: 1700000000123.5,
      sentEpochMs: 1700000000124.25,
      stream: 3,
    });
    expect(f.mime).toBe("image/png");
    expect([...f.image]).toEqual([0xff, 0xd8, 1, 2, 3]);
  });

  it("picks JPEG without the PNG flag and rejects short buffers", () => {
    const wire = new Uint8Array(HEADER_LEN + 4);
    wire.set(bytes(GOLDEN));
    new DataView(wire.buffer).setUint32(12, 0, true);
    expect(parseFrame(wire.buffer)!.mime).toBe("image/jpeg");
    expect(parseFrame(new ArrayBuffer(HEADER_LEN))).toBeNull();
    expect(parseFrame(new ArrayBuffer(3))).toBeNull();
  });
});
