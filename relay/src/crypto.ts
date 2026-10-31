// Host-key verification helpers. WebCrypto wants ECDSA signatures as raw r||s (IEEE P1363);
// Helm uses ASN.1 DER (Java SHA256withECDSA), so we convert.

export function base64ToBytes(b64: string): Uint8Array | null {
  if (typeof b64 !== "string" || b64.length === 0 || b64.length > 4096) return null;
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(b64)) return null;
  try {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  } catch {
    return null;
  }
}

export function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** DER `SEQUENCE { INTEGER r, INTEGER s }` to 64-byte r||s for P-256. Null if malformed. */
export function derToRawP256(der: Uint8Array): Uint8Array | null {
  let pos = 0;
  const readLen = (): number | null => {
    const first = der[pos++];
    if (first === undefined) return null;
    if (first < 0x80) return first;
    if (first !== 0x81) return null; // a P-256 signature never needs more than one length byte
    const n = der[pos++];
    return n !== undefined && n >= 0x80 ? n : null;
  };
  if (der[pos++] !== 0x30) return null;
  const seqLen = readLen();
  if (seqLen === null || pos + seqLen !== der.length) return null;
  const out = new Uint8Array(64);
  for (let k = 0; k < 2; k++) {
    if (der[pos++] !== 0x02) return null;
    const len = readLen();
    if (len === null || len === 0 || len > 33 || pos + len > der.length) return null;
    let int = der.subarray(pos, pos + len);
    pos += len;
    if ((int[0]! & 0x80) !== 0) return null; // negative
    if (int.length > 1 && int[0] === 0 && (int[1]! & 0x80) === 0) return null; // non-minimal
    while (int.length > 0 && int[0] === 0) int = int.subarray(1);
    if (int.length > 32) return null;
    out.set(int, k * 32 + (32 - int.length));
  }
  return pos === der.length ? out : null;
}

export interface VerifiedKey {
  /** Uncompressed EC point (65 bytes) as hex: the canonical form used for TOFU comparison. */
  pointHex: string;
}

/** Verifies a DER ECDSA-P256-SHA256 signature by an SPKI public key. Null on any failure. */
export async function verifyHostSignature(
  spkiB64: string,
  sigB64: string,
  message: string,
): Promise<VerifiedKey | null> {
  const spki = base64ToBytes(spkiB64);
  const der = base64ToBytes(sigB64);
  if (!spki || !der) return null;
  const raw = derToRawP256(der);
  if (!raw) return null;
  let key: CryptoKey;
  try {
    key = await crypto.subtle.importKey("spki", spki, { name: "ECDSA", namedCurve: "P-256" }, true, [
      "verify",
    ]);
  } catch {
    return null;
  }
  const ok = await crypto.subtle
    .verify({ name: "ECDSA", hash: "SHA-256" }, key, raw, new TextEncoder().encode(message))
    .catch(() => false);
  if (!ok) return null;
  const point = new Uint8Array((await crypto.subtle.exportKey("raw", key)) as ArrayBuffer);
  return { pointHex: bytesToHex(point) };
}

export function randomNonce(): string {
  return bytesToBase64(crypto.getRandomValues(new Uint8Array(32)));
}
