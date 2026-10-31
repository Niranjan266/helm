# Helm relay

A Cloudflare Worker + Durable Objects that lets a phone that is **not** on the home network reach
its PC (Docs/PLAN.md §3.1, Docs/ROADMAP.md Phase 4, feature A8).

The relay is a **blind byte pipe**. The phone and the PC agent run the normal, certificate-pinned
Helm session (Docs/PROTOCOL.md) *inside* the tunnel, so the relay only ever sees TLS ciphertext.
It never parses, stores or logs tunneled payloads.

```
 phone ──wss /connect/<host_id>──┐                       ┌──wss /host/<host_id>── PC agent
 phone ──wss /connect/<host_id>──┤  HostRelay DO         │  (outgoing, signed in
                                 └─ (one per host_id) ───┘   with the host key)
         raw tunnel bytes            adds/strips a 5-byte      mux frames
                                     channel header
```

## Components

| Part | What it does |
|---|---|
| Worker (`src/index.ts`) | Routes requests, validates `host_id`, applies the per-IP rate limit, forwards WebSocket upgrades to the host's Durable Object. |
| `HostRelay` DO (`src/host-relay.ts`) | One per `host_id` (`idFromName(host_id)`). Holds the host socket and up to N phone sockets (WebSocket Hibernation API), checks the host signature, stores the host public key (trust on first use), multiplexes phones over the host socket, enforces limits and idle timeouts. |
| `RateLimiter` DO (`src/rate-limiter.ts`) | One per client IP. In-memory token bucket for new connections. |

Both DO classes use the SQLite storage backend (`new_sqlite_classes`). The only persisted data is
the host's public key, timestamps and a channel counter. Nothing about phones is persisted.

## Endpoints

| Method + path | Who | Result |
|---|---|---|
| `GET /health` | anyone | `200 ok` |
| `GET /host/<host_id>` + WebSocket upgrade | PC agent | host control socket (see below) |
| `GET /connect/<host_id>` + WebSocket upgrade | phone | tunnel socket |
| `GET /turn?host_id=<host_id>` | phone / agent | TURN credentials (stub, see below) |

`host_id` is the agent's UUID v4 (PROTOCOL.md §2). It is case-insensitive and normalised to
lowercase. Anything else is `400`.

HTTP rejections happen **before** the upgrade, so clients see a plain status code:

| Status | Meaning |
|---|---|
| `400` | malformed `host_id` |
| `404` | (`/connect`, `/turn`) the host is not connected right now |
| `426` | upgrade header missing on a WebSocket route |
| `429` | per-IP connect rate limit hit (`Retry-After` header set), or (`/host`) too many unauthenticated host sockets for this `host_id` |
| `503` | (`/connect`) the host already has the maximum number of phones |
| `501` | (`/turn`) no TURN provider configured |

---

## Tunnel protocol v1

This is the contract the PC agent (`host/agent`) and the Android app must implement to use the
relay. All multi-byte integers are big-endian.

### 1. Host connection (PC agent → relay)

The agent keeps **one** outgoing WebSocket to `wss://<relay>/host/<host_id>` while it runs, and
reconnects with exponential backoff (1 s → 60 s, with jitter) when it drops.

**1.1 Challenge.** Right after the upgrade the relay sends one text frame:

```json
{"type":"challenge","v":1,"nonce":"<base64 of 32 random bytes>"}
```

**1.2 Auth.** Within **10 s** the agent answers with one text frame:

```json
{"type":"auth","public_key":"<base64 SPKI DER>","signature":"<base64 DER ECDSA sig>"}
```

- Key: the agent's existing **ECDSA P-256 certificate key** (the one in `identity.json`). One
  identity, no new secret to store. `public_key` = base64 (standard, padded) X.509
  SubjectPublicKeyInfo DER, exactly as in PROTOCOL.md §2.
- Signature: SHA256withECDSA, ASN.1 DER, base64 (standard), over the UTF-8 string

  ```
  helm-relay-v1|host|<host_id>|<nonce>
  ```

  where `<host_id>` is lowercase and `<nonce>` is the exact string from the challenge. The
  `helm-relay-v1|host` prefix keeps these signatures apart from every other use of the key.

**1.3 Trust on first use.** The first successful auth for a `host_id` registers that public key
in the Durable Object. Every later auth must use the **same** key (compared as the uncompressed EC
point, so SPKI encoding details do not matter); a different key is rejected with close code
`4403`. There is no API to reset a registration; a PC that loses its key also gets a new
`host_id` (both live in `identity.json`) and must be re-paired anyway.

**1.4 Ready.** On success the relay replies:

```json
{"type":"ready","v":1,"registered":true,"max_phones":4,"max_frame":65536}
```

`registered` is `true` only on the very first (TOFU) registration. From now on the socket carries
**binary mux frames** (§3) plus keep-alive text.

If a second host socket authenticates for the same `host_id`, it **replaces** the old one: the
old host socket is closed with `4409` and all its phone channels are closed with `4410`. An
unauthenticated socket never disturbs the current host.

**1.5 Keep-alive.** The agent sends the text frame `ping` every **30 s**; the relay answers `pong`
(the runtime answers without waking the DO). A host socket with no frames at all for **90 s** is
closed with `4408`.

**1.6 Errors.** On a failed handshake the relay sends `{"type":"error","code":"…"}` and closes:

| code | close code | meaning |
|---|---|---|
| `bad_message` | 4400 | not JSON / wrong type / missing field / binary before auth |
| `bad_signature` | 4401 | key does not parse as P-256 or the signature does not verify |
| `key_mismatch` | 4403 | valid signature, but not the key registered for this `host_id` |
| `timeout` | 4408 | no `auth` within 10 s |

The agent should back off for longer (≥ 10 min) after `4403`: retrying will not help.

### 2. Phone connection (phone → relay)

The phone opens `wss://<relay>/connect/<host_id>`. The relay does **not** authenticate the phone;
the pinned TLS session inside the tunnel does that (§4). The relay accepts the upgrade only if the
host is connected and authenticated and the phone limit is not reached (`404` / `503` otherwise).

On accept the relay assigns the phone a **channel id** and sends `OPEN` to the host. Then:

- Each **binary** WebSocket message from the phone is the next chunk of the phone→PC byte
  stream, 1 … 65 536 bytes. Message boundaries carry no meaning.
- Each **binary** message the phone receives is the next chunk of the PC→phone byte stream.
- The phone may send the text frame `ping` (answered with `pong`). Any other text frame closes
  the socket with `4400`. A message over 65 536 bytes closes it with `1009`.
- The tunnel ends when either WebSocket closes. A phone socket with no data in either direction
  for **120 s** is closed with `4408` (the inner Helm WebSocket already pings every 15 s, so a
  live session never hits this).

### 3. Mux frames (host ↔ relay, binary WebSocket messages)

```
 0        1                5
 +--------+----------------+---------------------------+
 |  type  |  channel (u32) |  payload (0 … 65 536 B)   |
 +--------+----------------+---------------------------+
```

| type | name | direction | payload |
|---|---|---|---|
| `0x01` | `OPEN` | relay → host | empty (reserved; the host ignores any bytes) |
| `0x02` | `DATA` | both | next chunk of that channel's byte stream (1 … 65 536 B) |
| `0x03` | `CLOSE` | both | empty (reserved). relay→host: the phone left. host→relay: drop this phone. Not acknowledged. |

Rules:

- Channel ids are assigned by the relay, start at 1, and are **never reused** for a given
  `host_id` (the counter is persisted).
- The relay sends `OPEN` for a channel before any `DATA` for it. Frames of one channel are
  delivered in order.
- `DATA`/`CLOSE` for an unknown or already-closed channel are silently dropped (races are normal).
- Unknown `type` values are ignored (forward compatibility). A frame shorter than 5 bytes closes
  the host socket with `4400`; a payload over 65 536 bytes closes it with `1009`.
- Text frames other than `ping` from an authenticated host are ignored.
- When the host socket goes away, every phone socket is closed with `4410`.

### 4. The Helm session inside the tunnel

Each channel is treated by both ends **exactly like a fresh TCP connection to the agent's
`/ws` port** (PROTOCOL.md §1). Nothing in the Helm protocol changes:

1. The phone starts a **TLS 1.3 client handshake** over the tunnel byte stream and pins the
   agent's certificate: `SHA-256(leaf DER)` must equal the fingerprint saved at pairing. No
   hostname check, no CA validation (same rules as on the LAN).
2. Inside TLS the phone sends the normal HTTP/1.1 WebSocket upgrade `GET /ws`. The `Host`
   header is `<host_id>.relay.helm.invalid`; the agent must not depend on it.
3. Then the usual handshake: agent `hello` (fresh nonce) → phone `auth` → `ready`, followed by
   `req`/`res`/`event` and binary file frames. Inner WebSocket frames may be split across or
   merged into tunnel chunks arbitrarily.
4. Closing the inner session (TLS `close_notify` + TCP-like EOF) is signalled by closing the
   tunnel: the phone closes its relay WebSocket; the agent sends `CLOSE`.

Because TLS is end-to-end between phone and agent, the relay sees only: `host_id`, client IPs,
connection times, and the size/timing of ciphertext chunks. It cannot read or alter the session
without breaking the pinned TLS.

**PC agent implementation notes.**
- On `OPEN ch`: create an in-memory duplex pipe (e.g. `tokio::io::duplex(256 KiB)`), and run the
  **same** TLS acceptor + WebSocket handler used for LAN connections on one end. Feed `DATA`
  payloads into the pipe; read the other side in chunks of ≤ 65 536 bytes and send them as
  `DATA ch`. On `CLOSE ch` or pipe EOF/error: tear down and (if not already) send `CLOSE ch`.
- Apply the same per-connection limits as on the LAN, and bound the per-channel input buffer
  (close the channel if the phone outruns the TLS stack by more than ~1 MiB).
- `pair` messages SHOULD be refused on relayed channels (`bad_message`): pairing requires the
  QR code, which is a LAN/proximity action. Only `auth` from already-paired devices is accepted.
- Mark relayed sessions in `audit.log` (e.g. `via=relay`).

**Android implementation notes.**
- Wrap the relay WebSocket (OkHttp) as a `java.net.Socket` whose input/output streams are the
  tunnel byte stream, return it from a custom `SocketFactory`, and keep the existing OkHttp
  pinning setup (`sslSocketFactory` + pinned `TrustManager` + `hostnameVerifier { true }`). OkHttp
  then layers TLS and the Helm WebSocket on top unchanged. Use a custom `Dns` so the dummy host
  name resolves without a lookup.
- Try the LAN path first (mDNS / saved address); use the relay only when that fails.

### 5. Close codes (relay → client)

| code | meaning |
|---|---|
| 1000 | normal (host sent `CLOSE` for this channel) |
| 1009 | frame too large |
| 4400 | protocol error (bad frame / unexpected text / bad handshake message) |
| 4401 | bad signature or bad key |
| 4403 | key does not match the registered key |
| 4408 | auth or idle timeout |
| 4409 | host replaced by a newer authenticated connection |
| 4410 | host went away (sent to phones) |

---

## Limits

All configurable through `[vars]` in `wrangler.toml`:

| var | default | meaning |
|---|---|---|
| `MAX_PHONES_PER_HOST` | 4 | concurrent phone channels per host |
| `MAX_FRAME_BYTES` | 65536 | max tunnel payload per WebSocket message |
| `HOST_AUTH_TIMEOUT_MS` | 10000 | time to answer the challenge |
| `HOST_IDLE_TIMEOUT_MS` | 90000 | host socket with no frames (incl. `ping`) |
| `PHONE_IDLE_TIMEOUT_MS` | 120000 | phone socket with no data either way |
| `MAX_PENDING_HOSTS` | 2 | unauthenticated host sockets per `host_id` at once |
| `CONNECT_BURST` | 10 | per-IP token bucket size (all `/host`, `/connect`, `/turn` requests) |
| `CONNECT_PER_MINUTE` | 20 | per-IP refill rate |

Rate-limit state is in memory only; a restarted limiter starts full, which is acceptable for an
abuse brake. Use Cloudflare WAF rate-limit rules in front of it for anything stronger.

There is no back-pressure on Workers WebSockets; the frame cap and the agent's own bounded
buffers keep memory use per channel small.

## Privacy

- Payload bytes are never logged, persisted, inspected or copied beyond forwarding.
- The code contains no `console.log` calls. `[observability]` is disabled in `wrangler.toml`, so
  not even request metadata is kept by default.
- Persisted per host: public key (EC point), first-registration and last-auth time, channel
  counter. Nothing per phone. IPs only live in the rate limiter's memory.

## TURN (stub)

`GET /turn?host_id=<id>` exists for the later WebRTC media path (ROADMAP Phase 4). It returns
time-limited credentials in the common "TURN REST API" shared-secret scheme:

```json
{"urls":["turn:turn.example.com:3478?transport=udp"],"username":"<unix expiry>:<host_id>","credential":"<base64 HMAC-SHA1(secret, username)>","ttl":3600}
```

It needs `TURN_URLS` (comma-separated, a var) and `TURN_SHARED_SECRET` (set with
`wrangler secret put`); without them it answers `501`. It also answers `404` unless the host is
currently connected, so credentials are only handed out for live hosts. No provider is
configured in this repo. Before using it for real, gate it behind the inner Helm session (have
the agent hand credentials to authenticated phones) instead of this public endpoint.

## Security notes

- **TOFU squatting:** whoever authenticates first for a `host_id` owns it on this relay.
  `host_id`s are random UUIDs only shown in the pairing QR, so an attacker would have to know it
  before the PC first connects. Even then they cannot impersonate the PC to phones, because the
  inner TLS is pinned to the PC's certificate; the worst case is denial of service for that
  `host_id`.
- **Phones are not authenticated by the relay.** Anyone who knows a `host_id` can open a channel
  and attempt a TLS handshake with the agent. The agent's normal device auth (and its own rate
  limits) protects it; the relay adds the per-IP limit and the per-host phone cap.
- The relay never holds a private key and cannot decrypt anything.

## Development

```sh
npm install
npm test          # vitest 4 + @cloudflare/vitest-pool-workers (runs workerd locally)
npm run typecheck
npx wrangler dev  # local only
```

`@cloudflare/vitest-pool-workers` 0.23 is the last release under that name (upstream renamed it
`@cloudflare/vitest-plugin`); the config uses its `cloudflareTest()` plugin, so switching later is
a package rename.

Not deployed. Deploying needs a Cloudflare account (`npx wrangler deploy`) and, optionally, a
custom domain route.
