import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { derToRawP256 } from "../src/crypto";
import { Client, HostKey, connectHost, newHostId, open, relayMessage, upgrade } from "./helpers";

async function challenge(hostId: string): Promise<{ host: Client; nonce: string }> {
  const host = await open(`/host/${hostId}`);
  const c = await host.nextJson();
  expect(c.type).toBe("challenge");
  expect(c.v).toBe(1);
  expect(typeof c.nonce).toBe("string");
  return { host, nonce: c.nonce as string };
}

describe("host auth (trust on first use)", () => {
  it("registers the key on first connect and accepts the same key afterwards", async () => {
    const id = newHostId();
    const key = await HostKey.generate();

    const first = await connectHost(id, key);
    expect(first.ready).toMatchObject({ type: "ready", v: 1, registered: true, max_phones: 2, max_frame: 1024 });

    const second = await connectHost(id, key);
    expect(second.ready).toMatchObject({ type: "ready", registered: false });
    // The newer authenticated connection replaces the old one.
    expect(await first.host.closeCode()).toBe(4409);
    second.host.close();
  });

  it("accepts an uppercase host_id as the same host", async () => {
    const id = newHostId();
    const key = await HostKey.generate();
    const { host } = await connectHost(id, key);
    const res = await upgrade(`/connect/${id.toUpperCase()}`);
    expect(res.status).toBe(101);
    res.webSocket?.accept();
    res.webSocket?.close();
    host.close();
  });

  it("rejects a different key for a registered host_id", async () => {
    const id = newHostId();
    const owner = await HostKey.generate();
    const intruder = await HostKey.generate();
    const { host: ownerSocket } = await connectHost(id, owner);

    const { host, nonce } = await challenge(id);
    host.send(
      JSON.stringify({
        type: "auth",
        public_key: intruder.spkiB64,
        signature: await intruder.signDer(relayMessage(id, nonce)),
      }),
    );
    expect(await host.nextJson()).toEqual({ type: "error", code: "key_mismatch" });
    expect(await host.closeCode()).toBe(4403);

    // The rightful host was not disturbed.
    expect(await ownerSocket.quiet()).toBe(true);
    const phone = await upgrade(`/connect/${id}`);
    expect(phone.status).toBe(101);
    phone.webSocket?.accept();
    phone.webSocket?.close();
    ownerSocket.close();
  });

  it("rejects a key mismatch even while the owner is offline", async () => {
    const id = newHostId();
    const owner = await HostKey.generate();
    const { host: first } = await connectHost(id, owner);
    first.close();
    await first.closed;

    const intruder = await HostKey.generate();
    const { host, nonce } = await challenge(id);
    host.send(
      JSON.stringify({ type: "auth", public_key: intruder.spkiB64, signature: await intruder.signDer(relayMessage(id, nonce)) }),
    );
    expect(await host.nextJson()).toEqual({ type: "error", code: "key_mismatch" });
    expect(await host.closeCode()).toBe(4403);
  });

  it("rejects a signature over the wrong nonce", async () => {
    const id = newHostId();
    const key = await HostKey.generate();
    const { host } = await challenge(id);
    host.send(
      JSON.stringify({ type: "auth", public_key: key.spkiB64, signature: await key.signDer(relayMessage(id, "AAAA")) }),
    );
    expect(await host.nextJson()).toEqual({ type: "error", code: "bad_signature" });
    expect(await host.closeCode()).toBe(4401);
  });

  it("rejects a signature made for another host_id", async () => {
    const id = newHostId();
    const key = await HostKey.generate();
    const { host, nonce } = await challenge(id);
    host.send(
      JSON.stringify({ type: "auth", public_key: key.spkiB64, signature: await key.signDer(relayMessage(newHostId(), nonce)) }),
    );
    expect(await host.nextJson()).toEqual({ type: "error", code: "bad_signature" });
    expect(await host.closeCode()).toBe(4401);
  });

  it("rejects garbage keys and malformed messages", async () => {
    const id = newHostId();
    const a = await challenge(id);
    a.host.send(JSON.stringify({ type: "auth", public_key: "bm90IGEga2V5", signature: "MAA=" }));
    expect(await a.host.nextJson()).toEqual({ type: "error", code: "bad_signature" });
    expect(await a.host.closeCode()).toBe(4401);

    const b = await challenge(id);
    b.host.send("{not json");
    expect(await b.host.nextJson()).toEqual({ type: "error", code: "bad_message" });
    expect(await b.host.closeCode()).toBe(4400);

    const c = await challenge(id);
    c.host.send(new Uint8Array([2, 0, 0, 0, 1, 42]));
    expect(await c.host.nextJson()).toEqual({ type: "error", code: "bad_message" });
    expect(await c.host.closeCode()).toBe(4400);
  });

  it("a failed attempt does not register a key", async () => {
    const id = newHostId();
    const key = await HostKey.generate();
    const { host } = await challenge(id);
    host.send(JSON.stringify({ type: "auth", public_key: key.spkiB64, signature: await key.signDer("wrong") }));
    expect(await host.closeCode()).toBe(4401);

    const other = await HostKey.generate();
    const { ready } = await connectHost(id, other);
    expect(ready.registered).toBe(true);
  });

  it("closes a host that never answers the challenge", async () => {
    const { host } = await challenge(newHostId());
    expect(await host.nextJson()).toEqual({ type: "error", code: "timeout" });
    expect(await host.closeCode()).toBe(4408);
  });

  it("limits unauthenticated host sockets per host_id", async () => {
    const id = newHostId();
    const a = await challenge(id);
    const b = await challenge(id);
    const res = await upgrade(`/host/${id}`);
    expect(res.status).toBe(429);
    a.host.close();
    b.host.close();
  });

  it("answers keep-alive pings", async () => {
    const { host } = await connectHost(newHostId(), await HostKey.generate());
    host.send("ping");
    expect(await host.next()).toBe("pong");
    host.close();
  });

  it("closes an authenticated host that goes silent", async () => {
    const { host } = await connectHost(newHostId(), await HostKey.generate());
    expect(await host.closeCode(6000)).toBe(4408);
  });
});

describe("routing", () => {
  it("serves /health and rejects bad requests", async () => {
    expect((await SELF.fetch("https://relay.test/health")).status).toBe(200);
    expect((await upgrade("/host/not-a-uuid")).status).toBe(400);
    expect((await upgrade("/connect/../host")).status).not.toBe(101);
    expect((await SELF.fetch(`https://relay.test/connect/${newHostId()}`)).status).toBe(426);
    expect((await SELF.fetch("https://relay.test/nope")).status).toBe(404);
    expect((await SELF.fetch("https://relay.test/health", { method: "POST" })).status).toBe(405);
  });

  it("returns 404 when a phone connects to an offline host", async () => {
    expect((await upgrade(`/connect/${newHostId()}`)).status).toBe(404);
  });
});

describe("DER signature parsing", () => {
  it("rejects malformed encodings", () => {
    expect(derToRawP256(new Uint8Array([]))).toBeNull();
    expect(derToRawP256(new Uint8Array([0x30, 0x02, 0x02, 0x00]))).toBeNull();
    // trailing garbage
    expect(derToRawP256(new Uint8Array([0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01, 0x00]))).toBeNull();
    // negative integer
    expect(derToRawP256(new Uint8Array([0x30, 0x06, 0x02, 0x01, 0x81, 0x02, 0x01, 0x01]))).toBeNull();
    const ok = derToRawP256(new Uint8Array([0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x02]));
    expect(ok?.[31]).toBe(1);
    expect(ok?.[63]).toBe(2);
  });
});
