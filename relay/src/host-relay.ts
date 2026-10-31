import { DurableObject } from "cloudflare:workers";
import { randomNonce, verifyHostSignature } from "./crypto";
import { limits, type Env, type Limits } from "./env";
import { FRAME_CLOSE, FRAME_DATA, FRAME_OPEN, decodeFrame, encodeFrame } from "./frames";

/** WebSocket close codes used by the relay (README "Close codes"). */
export const Close = {
  NORMAL: 1000,
  TOO_BIG: 1009,
  PROTOCOL: 4400,
  BAD_SIGNATURE: 4401,
  KEY_MISMATCH: 4403,
  TIMEOUT: 4408,
  REPLACED: 4409,
  HOST_GONE: 4410,
} as const;

export const SIGN_PREFIX = "helm-relay-v1|host|";

type HostAttachment = {
  role: "host";
  hostId: string;
  authed: boolean;
  nonce: string;
  since: number;
  last: number;
  dead?: true;
};
type PhoneAttachment = { role: "phone"; ch: number; last: number; dead?: true };
type Attachment = HostAttachment | PhoneAttachment;

/** Activity timestamps are persisted to the socket attachment at most this often. */
const TOUCH_PERSIST_MS = 1000;

const OPEN = 1; // WebSocket.READY_STATE_OPEN

/**
 * One instance per host_id. Holds the PC agent's socket and its phone sockets, using the
 * WebSocket Hibernation API so idle tunnels cost nothing. Payload bytes are only forwarded:
 * never parsed, stored or logged.
 */
export class HostRelay extends DurableObject<Env> {
  private readonly lim: Limits;
  private readonly cache = new WeakMap<WebSocket, { att: Attachment; saved: number }>();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.lim = limits(env);
    const sql = ctx.storage.sql;
    sql.exec(`CREATE TABLE IF NOT EXISTS host_key (
      id INTEGER PRIMARY KEY CHECK (id = 1),
      point TEXT NOT NULL,
      registered_at INTEGER NOT NULL,
      last_auth_at INTEGER NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS meta (
      id INTEGER PRIMARY KEY CHECK (id = 1),
      next_channel INTEGER NOT NULL)`);
    // Keep-alives are answered by the runtime without waking the object.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  // ---------------------------------------------------------------- HTTP entry (from the Worker)

  override async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const hostId = request.headers.get("x-helm-host-id") ?? "";
    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
      return new Response("expected websocket", { status: 426 });
    }
    if (url.pathname === "/host") return this.acceptHost(hostId);
    if (url.pathname === "/connect") return this.acceptPhone();
    return new Response("not found", { status: 404 });
  }

  /** RPC: is an authenticated host connected right now? */
  isHostOnline(): boolean {
    return this.currentHost() !== null;
  }

  private async acceptHost(hostId: string): Promise<Response> {
    const pending = this.live("host").filter((ws) => !(this.att(ws) as HostAttachment).authed);
    if (pending.length >= this.lim.maxPendingHosts) {
      return new Response("too many pending host connections", { status: 429 });
    }
    const [client, server] = Object.values(new WebSocketPair()) as [WebSocket, WebSocket];
    this.ctx.acceptWebSocket(server, ["host"]);
    const now = Date.now();
    const nonce = randomNonce();
    this.setAtt(server, { role: "host", hostId, authed: false, nonce, since: now, last: now });
    server.send(JSON.stringify({ type: "challenge", v: 1, nonce }));
    await this.scheduleAlarm(now + this.lim.authTimeoutMs);
    return new Response(null, { status: 101, webSocket: client });
  }

  private async acceptPhone(): Promise<Response> {
    const host = this.currentHost();
    if (!host) return new Response("host offline", { status: 404 });
    if (this.live("phone").length >= this.lim.maxPhones) {
      return new Response("too many phones for this host", { status: 503 });
    }
    const ch = this.nextChannel();
    const [client, server] = Object.values(new WebSocketPair()) as [WebSocket, WebSocket];
    this.ctx.acceptWebSocket(server, ["phone"]);
    const now = Date.now();
    this.setAtt(server, { role: "phone", ch, last: now });
    this.safeSend(host, encodeFrame(FRAME_OPEN, ch));
    await this.scheduleAlarm(now + this.lim.phoneIdleMs);
    return new Response(null, { status: 101, webSocket: client });
  }

  // ---------------------------------------------------------------- WebSocket events

  override async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const a = this.att(ws);
    if (a.dead) return;
    if (a.role === "phone") return this.onPhoneMessage(ws, a, message);
    if (!a.authed) return this.onHostAuth(ws, a, message);
    return this.onHostFrame(ws, message);
  }

  override async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    this.onGone(ws, code);
  }

  override async webSocketError(ws: WebSocket): Promise<void> {
    this.onGone(ws, 1011);
  }

  override async alarm(): Promise<void> {
    const now = Date.now();
    let next = Number.POSITIVE_INFINITY;
    for (const ws of this.ctx.getWebSockets()) {
      const a = this.att(ws);
      if (a.dead || ws.readyState !== OPEN) continue;
      const deadline = this.deadline(ws, a);
      if (deadline > now) {
        next = Math.min(next, deadline);
        continue;
      }
      if (a.role === "phone") this.closePhone(ws, Close.TIMEOUT, "idle timeout", true);
      else if (a.authed) this.dropHost(ws, Close.TIMEOUT, "idle timeout");
      else this.failAuth(ws, "timeout", Close.TIMEOUT);
    }
    if (Number.isFinite(next)) await this.ctx.storage.setAlarm(next);
  }

  // ---------------------------------------------------------------- host side

  private async onHostAuth(ws: WebSocket, a: HostAttachment, message: string | ArrayBuffer) {
    if (typeof message !== "string") return this.failAuth(ws, "bad_message", Close.PROTOCOL);
    let msg: unknown;
    try {
      msg = JSON.parse(message);
    } catch {
      return this.failAuth(ws, "bad_message", Close.PROTOCOL);
    }
    const m = msg as { type?: unknown; public_key?: unknown; signature?: unknown };
    if (m?.type !== "auth" || typeof m.public_key !== "string" || typeof m.signature !== "string") {
      return this.failAuth(ws, "bad_message", Close.PROTOCOL);
    }
    const key = await verifyHostSignature(m.public_key, m.signature, SIGN_PREFIX + a.hostId + "|" + a.nonce);
    if (ws.readyState !== OPEN || this.att(ws).dead) return;
    if (!key) return this.failAuth(ws, "bad_signature", Close.BAD_SIGNATURE);

    // Trust on first use. All SQL below runs synchronously, so concurrent auths cannot race.
    const now = Date.now();
    const sql = this.ctx.storage.sql;
    const rows = sql.exec<{ point: string }>("SELECT point FROM host_key WHERE id = 1").toArray();
    let registered = false;
    if (rows.length === 0) {
      sql.exec(
        "INSERT INTO host_key (id, point, registered_at, last_auth_at) VALUES (1, ?, ?, ?)",
        key.pointHex,
        now,
        now,
      );
      registered = true;
    } else if (rows[0]!.point !== key.pointHex) {
      return this.failAuth(ws, "key_mismatch", Close.KEY_MISMATCH);
    } else {
      sql.exec("UPDATE host_key SET last_auth_at = ? WHERE id = 1", now);
    }

    // A newly authenticated host replaces the old one and everything tunneled through it.
    for (const other of this.live("host")) {
      if (other !== ws && (this.att(other) as HostAttachment).authed) {
        this.dropHost(other, Close.REPLACED, "replaced by a newer host connection");
      }
    }
    for (const phone of this.live("phone")) this.closePhone(phone, Close.HOST_GONE, "host reconnected", false);

    this.setAtt(ws, { ...a, authed: true, last: now });
    this.safeSend(
      ws,
      JSON.stringify({ type: "ready", v: 1, registered, max_phones: this.lim.maxPhones, max_frame: this.lim.maxFrame }),
    );
    await this.scheduleAlarm(now + this.lim.hostIdleMs);
  }

  private onHostFrame(ws: WebSocket, message: string | ArrayBuffer) {
    this.touch(ws);
    if (typeof message === "string") return; // keep-alive or unknown text: ignored
    const frame = decodeFrame(message);
    if (!frame) return this.dropHost(ws, Close.PROTOCOL, "bad frame");
    if (frame.payload.byteLength > this.lim.maxFrame) return this.dropHost(ws, Close.TOO_BIG, "frame too large");
    if (frame.type === FRAME_DATA) {
      if (frame.payload.byteLength === 0) return;
      const phone = this.findPhone(frame.channel);
      if (phone && this.safeSend(phone, frame.payload)) this.touch(phone);
    } else if (frame.type === FRAME_CLOSE) {
      const phone = this.findPhone(frame.channel);
      if (phone) this.closePhone(phone, Close.NORMAL, "closed by host", false);
    }
    // OPEN from the host and unknown types are ignored (forward compatibility).
  }

  private failAuth(ws: WebSocket, code: string, closeCode: number) {
    const a = this.att(ws);
    this.setAtt(ws, { ...a, dead: true });
    this.safeSend(ws, JSON.stringify({ type: "error", code }));
    this.safeClose(ws, closeCode, code);
  }

  /** Closes an authenticated host socket and every phone tunneled through it. */
  private dropHost(ws: WebSocket, code: number, reason: string) {
    const a = this.att(ws);
    this.setAtt(ws, { ...a, dead: true });
    this.safeClose(ws, code, reason);
    if (a.role === "host" && a.authed && this.currentHost() === null) {
      for (const phone of this.live("phone")) this.closePhone(phone, Close.HOST_GONE, "host went away", false);
    }
  }

  // ---------------------------------------------------------------- phone side

  private onPhoneMessage(ws: WebSocket, a: PhoneAttachment, message: string | ArrayBuffer) {
    if (typeof message === "string") return this.closePhone(ws, Close.PROTOCOL, "binary frames only", true);
    if (message.byteLength > this.lim.maxFrame) return this.closePhone(ws, Close.TOO_BIG, "frame too large", true);
    if (message.byteLength === 0) return;
    const host = this.currentHost();
    if (!host) return this.closePhone(ws, Close.HOST_GONE, "host went away", false);
    if (this.safeSend(host, encodeFrame(FRAME_DATA, a.ch, new Uint8Array(message)))) {
      this.touch(ws);
      this.touch(host);
    }
  }

  private closePhone(ws: WebSocket, code: number, reason: string, notifyHost: boolean) {
    const a = this.att(ws) as PhoneAttachment;
    if (a.dead) return;
    this.setAtt(ws, { ...a, dead: true });
    this.safeClose(ws, code, reason);
    if (notifyHost) {
      const host = this.currentHost();
      if (host) this.safeSend(host, encodeFrame(FRAME_CLOSE, a.ch));
    }
  }

  /** The remote end of a socket closed or failed. */
  private onGone(ws: WebSocket, code: number) {
    const a = this.att(ws);
    if (!a.dead) {
      if (a.role === "phone") this.closePhone(ws, Close.NORMAL, "", true);
      else if (a.authed) this.dropHost(ws, Close.NORMAL, "");
      else this.setAtt(ws, { ...a, dead: true });
    }
    this.safeClose(ws, code, "");
  }

  // ---------------------------------------------------------------- helpers

  private live(tag: "host" | "phone"): WebSocket[] {
    return this.ctx.getWebSockets(tag).filter((ws) => ws.readyState === OPEN && !this.att(ws).dead);
  }

  private currentHost(): WebSocket | null {
    return this.live("host").find((ws) => (this.att(ws) as HostAttachment).authed) ?? null;
  }

  private findPhone(ch: number): WebSocket | null {
    return this.live("phone").find((ws) => (this.att(ws) as PhoneAttachment).ch === ch) ?? null;
  }

  private nextChannel(): number {
    const row = this.ctx.storage.sql
      .exec<{ next_channel: number }>(
        `INSERT INTO meta (id, next_channel) VALUES (1, 2)
         ON CONFLICT (id) DO UPDATE SET next_channel = next_channel + 1
         RETURNING next_channel`,
      )
      .one();
    return row.next_channel - 1;
  }

  private deadline(ws: WebSocket, a: Attachment): number {
    if (a.role === "phone") return a.last + this.lim.phoneIdleMs;
    if (!a.authed) return a.since + this.lim.authTimeoutMs;
    const pinged = this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() ?? 0;
    return Math.max(a.last, pinged) + this.lim.hostIdleMs;
  }

  private async scheduleAlarm(at: number) {
    const current = await this.ctx.storage.getAlarm();
    if (current === null || current > at) await this.ctx.storage.setAlarm(at);
  }

  private att(ws: WebSocket): Attachment {
    let entry = this.cache.get(ws);
    if (!entry) {
      entry = { att: ws.deserializeAttachment() as Attachment, saved: Date.now() };
      this.cache.set(ws, entry);
    }
    return entry.att;
  }

  private setAtt(ws: WebSocket, att: Attachment) {
    try {
      ws.serializeAttachment(att);
    } catch {
      // socket already gone; the in-memory copy is enough
    }
    this.cache.set(ws, { att, saved: Date.now() });
  }

  private touch(ws: WebSocket) {
    const entry = this.cache.get(ws);
    const now = Date.now();
    if (!entry) return this.setAtt(ws, { ...this.att(ws), last: now });
    entry.att.last = now;
    if (now - entry.saved >= TOUCH_PERSIST_MS) this.setAtt(ws, entry.att);
  }

  private safeSend(ws: WebSocket, data: string | Uint8Array): boolean {
    try {
      ws.send(data);
      return true;
    } catch {
      return false;
    }
  }

  private safeClose(ws: WebSocket, code: number, reason: string) {
    // 1005/1006/1015 are reserved and cannot be sent.
    const sendable = code === 1000 || (code >= 3000 && code <= 4999) || (code >= 1007 && code <= 1014);
    try {
      ws.close(sendable ? code : 1000, reason.slice(0, 120));
    } catch {
      // already closed
    }
  }
}
