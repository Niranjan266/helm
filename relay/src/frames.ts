// Tunnel protocol v1 mux frames (host <-> relay): [u8 type][u32 channel BE][payload].

export const FRAME_OPEN = 0x01;
export const FRAME_DATA = 0x02;
export const FRAME_CLOSE = 0x03;
export const HEADER_LEN = 5;

export function encodeFrame(type: number, channel: number, payload?: Uint8Array): Uint8Array {
  const out = new Uint8Array(HEADER_LEN + (payload?.byteLength ?? 0));
  const view = new DataView(out.buffer);
  view.setUint8(0, type);
  view.setUint32(1, channel >>> 0, false);
  if (payload) out.set(payload, HEADER_LEN);
  return out;
}

export interface Frame {
  type: number;
  channel: number;
  payload: Uint8Array;
}

export function decodeFrame(buf: ArrayBuffer): Frame | null {
  if (buf.byteLength < HEADER_LEN) return null;
  const view = new DataView(buf);
  return {
    type: view.getUint8(0),
    channel: view.getUint32(1, false),
    payload: new Uint8Array(buf, HEADER_LEN),
  };
}
