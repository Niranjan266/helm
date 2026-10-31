import { SELF } from "cloudflare:test";
import { bytesToBase64 } from "../src/crypto";
import { HEADER_LEN } from "../src/frames";

export type Msg = string | ArrayBuffer;

let ipCounter = 0;
/** A fresh fake client IP per call, so the per-IP rate limit only bites where a test wants it. */
export function uniqueIp(): string {
  ipCounter++;
  return `10.${(ipCounter >> 16) & 255}.${(ipCounter >> 8) & 255}.${ipCounter & 255}`;
}

export function newHostId(): string {
  return crypto.randomUUID();
}

export function upgrade(path: string, ip = uniqueIp()): Promise<Response> {
  return SELF.fetch(`https://relay.test${path}`, {
    headers: { Upgrade: "websocket", "CF-Connecting-IP": ip },
  });
}

/** Test-side WebSocket with an awaitable message queue. */
export class Client {
  private queue: Msg[] = [];
  private waiters: ((m: Msg) => void)[] = [];
  readonly closed: Promise<{ code: number; reason: string }>;

  constructor(readonly ws: WebSocket) {
    // Newer compatibility dates deliver client-side binary messages as Blob by default.
    (ws as unknown as { binaryType: string }).binaryType = "arraybuffer";
    ws.accept();
    ws.addEventListener("message", (e) => {
      const data = e.data as Msg;
      const w = this.waiters.shift();
      if (w) w(data);
      else this.queue.push(data);
    });
    this.closed = new Promise((resolve) => {
      ws.addEventListener("close", (e) => resolve({ code: e.code, reason: e.reason }));
    });
  }

  next(timeoutMs = 2000): Promise<Msg> {
    const queued = this.queue.shift();
    if (queued !== undefined) return Promise.resolve(queued);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.waiters = this.waiters.filter((w) => w !== done);
        reject(new Error("timed out waiting for a message"));
      }, timeoutMs);
      const done = (m: Msg) => {
        clearTimeout(timer);
        resolve(m);
      };
      this.waiters.push(done);
    });
  }

  async nextJson(): Promise<Record<string, unknown>> {
    const m = await this.next();
    if (typeof m !== "string") throw new Error("expected a text frame");
    return JSON.parse(m) as Record<string, unknown>;
  }

  async nextBinary(): Promise<Uint8Array> {
    const m = await this.next();
    if (typeof m === "string") throw new Error(`expected a binary frame, got text ${m}`);
    return new Uint8Array(m);
  }

  /** Resolves true if nothing arrives within `ms`. */
  async quiet(ms = 200): Promise<boolean> {
    if (this.queue.length > 0) return false;
    try {
      await this.next(ms);
      return false;
    } catch {
      return true;
    }
  }

  async closeCode(timeoutMs = 5000): Promise<number> {
    const timeout = new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error("socket did not close")), timeoutMs),
    );
    return (await Promise.race([this.closed, timeout])).code;
  }

  send(data: string | Uint8Array) {
    this.ws.send(data);
  }

  close(code = 1000) {
    try {
      this.ws.close(code, "");
    } catch {
      // ignore
    }
  }
}

export async function open(path: string, ip?: string): Promise<Client> {
  const res = await upgrade(path, ip);
  if (res.status !== 101 || !res.webSocket) throw new Error(`upgrade failed: ${res.status}`);
  return new Client(res.webSocket);
}

// ------------------------------------------------------------------ host keys

/** raw r||s (WebCrypto) to ASN.1 DER (what Java/Rust produce). */
export function rawToDer(raw: Uint8Array): Uint8Array {
  const int = (bytes: Uint8Array): number[] => {
    let i = 0;
    while (i < bytes.length - 1 && bytes[i] === 0) i++;
    const v = Array.from(bytes.subarray(i));
    if (v[0]! & 0x80) v.unshift(0);
    return [0x02, v.length, ...v];
  };
  const body = [...int(raw.subarray(0, 32)), ...int(raw.subarray(32))];
  return new Uint8Array([0x30, body.length, ...body]);
}

export class HostKey {
  private constructor(
    private readonly pair: CryptoKeyPair,
    readonly spkiB64: string,
  ) {}

  static async generate(): Promise<HostKey> {
    const pair = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
      "sign",
      "verify",
    ])) as CryptoKeyPair;
    const spki = new Uint8Array((await crypto.subtle.exportKey("spki", pair.publicKey)) as ArrayBuffer);
    return new HostKey(pair, bytesToBase64(spki));
  }

  async signDer(message: string): Promise<string> {
    const raw = new Uint8Array(
      await crypto.subtle.sign(
        { name: "ECDSA", hash: "SHA-256" },
        this.pair.privateKey,
        new TextEncoder().encode(message),
      ),
    );
    return bytesToBase64(rawToDer(raw));
  }
}

export function relayMessage(hostId: string, nonce: string): string {
  return `helm-relay-v1|host|${hostId}|${nonce}`;
}

/** Opens a host socket and completes the challenge/auth handshake. */
export async function connectHost(hostId: string, key: HostKey) {
  const host = await open(`/host/${hostId}`);
  const challenge = await host.nextJson();
  if (challenge.type !== "challenge") throw new Error("expected challenge");
  host.send(
    JSON.stringify({
      type: "auth",
      public_key: key.spkiB64,
      signature: await key.signDer(relayMessage(hostId, challenge.nonce as string)),
    }),
  );
  const ready = await host.nextJson();
  return { host, ready };
}

// ------------------------------------------------------------------ mux frames

export function frame(type: number, channel: number, payload: Uint8Array = new Uint8Array()): Uint8Array {
  const out = new Uint8Array(HEADER_LEN + payload.length);
  out[0] = type;
  new DataView(out.buffer).setUint32(1, channel, false);
  out.set(payload, HEADER_LEN);
  return out;
}

export function parse(buf: Uint8Array) {
  if (buf.byteLength < 5) throw new Error(`short frame: ${buf.byteLength} bytes: ${Array.from(buf)}`);
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  return { type: view.getUint8(0), channel: view.getUint32(1, false), payload: buf.subarray(HEADER_LEN) };
}

export const bytes = (s: string) => new TextEncoder().encode(s);
export const str = (b: Uint8Array) => new TextDecoder().decode(b);
export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
