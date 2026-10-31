import { SELF, env, evictDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { FRAME_CLOSE, FRAME_DATA, FRAME_OPEN } from "../src/frames";
import { turnCredentials } from "../src/turn";
import { bytesToBase64 } from "../src/crypto";
import {
  Client,
  HostKey,
  bytes,
  connectHost,
  frame,
  newHostId,
  open,
  parse,
  sleep,
  str,
  uniqueIp,
  upgrade,
} from "./helpers";

async function hostWithPhone() {
  const id = newHostId();
  const { host } = await connectHost(id, await HostKey.generate());
  const phone = await open(`/connect/${id}`);
  const opened = parse(await host.nextBinary());
  expect(opened.type).toBe(FRAME_OPEN);
  expect(opened.payload.length).toBe(0);
  return { id, host, phone, ch: opened.channel };
}

describe("forwarding", () => {
  it("forwards bytes both ways without changing them", async () => {
    const { host, phone, ch } = await hostWithPhone();
    expect(ch).toBe(1);

    const blob = crypto.getRandomValues(new Uint8Array(1024)); // exactly the cap
    phone.send(blob);
    const up = parse(await host.nextBinary());
    expect(up.type).toBe(FRAME_DATA);
    expect(up.channel).toBe(ch);
    expect(Array.from(up.payload)).toEqual(Array.from(blob));

    host.send(frame(FRAME_DATA, ch, bytes("\x16\x03\x03 server hello")));
    expect(str(await phone.nextBinary())).toBe("\x16\x03\x03 server hello");

    host.close();
  });

  it("tells the host when a phone leaves", async () => {
    const { host, phone, ch } = await hostWithPhone();
    phone.close();
    const closed = parse(await host.nextBinary());
    expect(closed).toMatchObject({ type: FRAME_CLOSE, channel: ch });
    host.close();
  });

  it("closes the phone when the host sends CLOSE", async () => {
    const { host, phone, ch } = await hostWithPhone();
    host.send(frame(FRAME_CLOSE, ch));
    expect(await phone.closeCode()).toBe(1000);
    expect(await host.quiet()).toBe(true); // no CLOSE echoed back
    host.close();
  });

  it("closes all phones when the host goes away", async () => {
    const { host, phone } = await hostWithPhone();
    host.close();
    expect(await phone.closeCode()).toBe(4410);
  });

  it("closes old phones when the host reconnects", async () => {
    const id = newHostId();
    const key = await HostKey.generate();
    const { host } = await connectHost(id, key);
    const phone = await open(`/connect/${id}`);
    await host.nextBinary();
    const again = await connectHost(id, key);
    expect(await host.closeCode()).toBe(4409);
    expect(await phone.closeCode()).toBe(4410);
    again.host.close();
  });

  it("drops frames for unknown channels and ignores unknown frame types", async () => {
    const { host, phone, ch } = await hostWithPhone();
    host.send(frame(FRAME_DATA, ch + 1000, bytes("lost")));
    host.send(frame(0x7f, ch, bytes("future")));
    host.send("some text the relay ignores");
    host.send(frame(FRAME_DATA, ch, bytes("ok")));
    expect(str(await phone.nextBinary())).toBe("ok");
    host.close();
  });

  it("keeps working after the Durable Object hibernates", async () => {
    const { id, host, phone, ch } = await hostWithPhone();
    const stub = env.HOST_RELAY.get(env.HOST_RELAY.idFromName(id));
    await evictDurableObject(stub);

    phone.send(bytes("after sleep"));
    const up = parse(await host.nextBinary());
    expect(up.channel).toBe(ch);
    expect(str(up.payload)).toBe("after sleep");
    host.send(frame(FRAME_DATA, ch, bytes("still here")));
    expect(str(await phone.nextBinary())).toBe("still here");
    host.close();
  });
});

describe("multiplexing", () => {
  it("keeps several phones on separate channels", async () => {
    const id = newHostId();
    const { host } = await connectHost(id, await HostKey.generate());
    const p1 = await open(`/connect/${id}`);
    const ch1 = parse(await host.nextBinary()).channel;
    const p2 = await open(`/connect/${id}`);
    const ch2 = parse(await host.nextBinary()).channel;
    expect(ch1).not.toBe(ch2);

    p1.send(bytes("from one"));
    p2.send(bytes("from two"));
    const a = parse(await host.nextBinary());
    const b = parse(await host.nextBinary());
    expect(new Map([[a.channel, str(a.payload)], [b.channel, str(b.payload)]])).toEqual(
      new Map([[ch1, "from one"], [ch2, "from two"]]),
    );

    host.send(frame(FRAME_DATA, ch2, bytes("to two")));
    host.send(frame(FRAME_DATA, ch1, bytes("to one")));
    expect(str(await p1.nextBinary())).toBe("to one");
    expect(str(await p2.nextBinary())).toBe("to two");
    expect(await p1.quiet()).toBe(true);

    // Closing one channel leaves the other alone.
    host.send(frame(FRAME_CLOSE, ch1));
    expect(await p1.closeCode()).toBe(1000);
    p2.send(bytes("still open"));
    expect(str(parse(await host.nextBinary()).payload)).toBe("still open");
    host.close();
  });

  it("never reuses a channel id", async () => {
    const id = newHostId();
    const { host } = await connectHost(id, await HostKey.generate());
    const seen = new Set<number>();
    for (let i = 0; i < 4; i++) {
      const p = await open(`/connect/${id}`);
      const opened = parse(await host.nextBinary());
      expect(seen.has(opened.channel)).toBe(false);
      seen.add(opened.channel);
      p.close();
      expect(parse(await host.nextBinary()).type).toBe(FRAME_CLOSE);
    }
    host.close();
  });
});

describe("limits", () => {
  it("caps phones per host", async () => {
    const id = newHostId();
    const { host } = await connectHost(id, await HostKey.generate());
    const p1 = await open(`/connect/${id}`);
    const p2 = await open(`/connect/${id}`);
    expect((await upgrade(`/connect/${id}`)).status).toBe(503);
    p1.close();
    await host.nextBinary(); // OPEN 1
    await host.nextBinary(); // OPEN 2
    expect(parse(await host.nextBinary()).type).toBe(FRAME_CLOSE);
    const p3 = await upgrade(`/connect/${id}`);
    expect(p3.status).toBe(101);
    new Client(p3.webSocket!).close();
    p2.close();
    host.close();
  });

  it("closes a phone that sends an oversized frame", async () => {
    const { host, phone, ch } = await hostWithPhone();
    phone.send(new Uint8Array(1025));
    expect(await phone.closeCode()).toBe(1009);
    expect(parse(await host.nextBinary())).toMatchObject({ type: FRAME_CLOSE, channel: ch });
    host.close();
  });

  it("closes a host that sends an oversized frame", async () => {
    const { host, phone, ch } = await hostWithPhone();
    host.send(frame(FRAME_DATA, ch, new Uint8Array(1025)));
    expect(await host.closeCode()).toBe(1009);
    expect(await phone.closeCode()).toBe(4410);
  });

  it("closes a host that sends a truncated frame", async () => {
    const { host, phone } = await hostWithPhone();
    host.send(new Uint8Array([2, 0, 0]));
    expect(await host.closeCode()).toBe(4400);
    expect(await phone.closeCode()).toBe(4410);
  });

  it("closes a phone that sends text", async () => {
    const { host, phone, ch } = await hostWithPhone();
    phone.send("hello?");
    expect(await phone.closeCode()).toBe(4400);
    expect(parse(await host.nextBinary())).toMatchObject({ type: FRAME_CLOSE, channel: ch });
    host.close();
  });

  it("closes an idle phone", async () => {
    const { host, phone, ch } = await hostWithPhone();
    // Keep the host alive while the phone idles.
    const keepAlive = setInterval(() => host.send("ping"), 300);
    try {
      expect(await phone.closeCode(5000)).toBe(4408);
    } finally {
      clearInterval(keepAlive);
    }
    let msg = await host.next();
    while (msg === "pong") msg = await host.next();
    expect(parse(new Uint8Array(msg as ArrayBuffer))).toMatchObject({ type: FRAME_CLOSE, channel: ch });
    host.close();
  });

  it("traffic keeps a phone alive past the idle timeout", async () => {
    const { host, phone, ch } = await hostWithPhone();
    for (let i = 0; i < 8; i++) {
      await sleep(300);
      phone.send(bytes(`tick ${i}`));
      expect(str(parse(await host.nextBinary()).payload)).toBe(`tick ${i}`);
      host.send(frame(FRAME_DATA, ch, bytes(`tock ${i}`)));
      expect(str(await phone.nextBinary())).toBe(`tock ${i}`);
    }
    host.close();
  });

  it("rate-limits connects per IP", async () => {
    const ip = uniqueIp();
    const id = newHostId();
    const statuses: number[] = [];
    for (let i = 0; i < 6; i++) statuses.push((await upgrade(`/connect/${id}`, ip)).status);
    expect(statuses.slice(0, 5)).toEqual([404, 404, 404, 404, 404]);
    expect(statuses[5]).toBe(429);
    const limited = await upgrade(`/connect/${id}`, ip);
    expect(Number(limited.headers.get("retry-after"))).toBeGreaterThan(0);
    // Another IP is unaffected.
    expect((await upgrade(`/connect/${id}`, uniqueIp())).status).toBe(404);
  });
});

describe("turn stub", () => {
  const turn = (id: string) =>
    SELF.fetch(`https://relay.test/turn?host_id=${id}`, { headers: { "CF-Connecting-IP": uniqueIp() } });

  it("only hands out credentials for online hosts", async () => {
    const id = newHostId();
    expect((await turn(id)).status).toBe(404);
    expect((await turn("nope")).status).toBe(400);

    const { host } = await connectHost(id, await HostKey.generate());
    const res = await turn(id);
    expect(res.status).toBe(200);
    const body = (await res.json()) as { urls: string[]; username: string; credential: string; ttl: number };
    expect(body.urls).toEqual(["turn:turn.test:3478?transport=udp", "turns:turn.test:5349"]);
    expect(body.ttl).toBe(3600);
    const [expiry, user] = body.username.split(":");
    expect(user).toBe(id);
    expect(Number(expiry)).toBeGreaterThan(Date.now() / 1000);

    const key = await crypto.subtle.importKey("raw", bytes("test-only-secret"), { name: "HMAC", hash: "SHA-1" }, false, [
      "sign",
    ]);
    const mac = new Uint8Array(await crypto.subtle.sign("HMAC", key, bytes(body.username)));
    expect(body.credential).toBe(bytesToBase64(mac));
    host.close();
  });

  it("reports not configured without a provider", async () => {
    expect(await turnCredentials({ ...env, TURN_URLS: "", TURN_SHARED_SECRET: "x" }, newHostId())).toBeNull();
    expect(await turnCredentials({ ...env, TURN_SHARED_SECRET: undefined }, newHostId())).toBeNull();
  });
});
