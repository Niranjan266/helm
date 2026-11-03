# Pairing security — hello feature `g4`

Extends PROTOCOL.md §4–§5. An agent that speaks this adds `"g4"` to `hello.features`.
Phones must not send `pair_code` to an agent without `g4` (they show "update the PC" instead).

Four parts:

1. **Short-code pairing** — a 6-digit code instead of the QR, when the phone camera can't scan.
2. **Approve on PC** — optional "ask before a new phone pairs".
3. **Phone lock timeout** — phone-only (no wire change).
4. **USB pairing** — the USB tethering address goes first in the pairing link.

---

## 1. Short-code pairing

The PC shows a code and its name, e.g. `482 913` · `DESKTOP-1` (console `c` + Enter, tray
**Show pairing code**, or a PC-side UI via `AppState::new_code_pairing()`). The phone finds the
PC with mDNS (§3: address, port, `host_id`, name), the user picks the PC with that name and
types the code.

Problem: the phone has **no pinned fingerprint** yet (that is what the QR carries), so it must
accept whatever TLS certificate answers. The code is used to prove, afterwards, that the
certificate the phone saw is the PC's — without the code ever crossing the network and
without letting anyone test guesses offline.

### Code

- 6 decimal digits, uniformly random (`000000`–`999999`), shown as `123 456`.
- Valid **5 minutes**, **one attempt**: the code is used up as soon as an attempt reaches the
  code exchange (success or not). A new code replaces the old one.
- **Lockout:** 3 failed attempts within 10 minutes lock code pairing for **10 minutes**
  (`pairing_locked`; no new codes can be shown). QR pairing has its own lockout (§5.3).
- The phone refuses to try the same code with the same PC twice (remembered in memory, so a fake
  PC cannot narrow a code down over retries); the user asks the PC for a new one.

### Values

```
fp       = lowercase hex SHA-256 of the leaf certificate DER
           agent: its own; phone: the one its TLS handshake saw
nonce_a  = hello.nonce (base64 text, exactly as sent)
nonce_p  = 32 random phone bytes, base64 (pair_code.nonce)
h        = SHA-256( "helm-g4-code-v1|" host_id "|" nonce_a "|" nonce_p "|" fp "|"
                    device_id "|" public_key )          (UTF-8; public_key = base64 SPKI text)
r(R,i)   = 32 random bytes, fresh per attempt, for role R ∈ {A (agent), P (phone)}, digit i = 1..6
C(R,i)   = HMAC-SHA256( key = r(R,i),
                        "helm-g4-commit|" R "|" i "|" d_i "|" hex(h) )   d_i = i-th code digit
```

`C` hides `d_i` until `r` is revealed (HMAC as a PRF with a secret 256-bit key), and binds the
committer to `d_i`, to its role and round, and — through `h` — to the certificate it sees, both
nonces and the phone's key.

### Messages (after `hello`)

| # | direction | message |
|---|---|---|
| 1 | phone → PC | `{"type":"pair_code","device_id","device_name","public_key","nonce":nonce_p,"commits":[C(P,1)…C(P,6)],"signature"}` |
| 2 | PC → phone | `{"type":"code_commit","commits":[C(A,1)…C(A,6)]}` |
| 3 | PC → phone | `{"type":"code_open","i":1,"r":"<base64 r(A,1)>"}` |
| 4 | phone → PC | `{"type":"code_open","i":1,"r":"<base64 r(P,1)>"}` |
| … | | rounds 2…6 alternate the same way |
| 15 | PC → phone | (`pair_pending`, see §2, then) `ready` — or `error` |

- `signature` = SHA256withECDSA over `helm-v1|pair_code|<host_id>|<hello nonce>` (proof of the
  device key; checked before the code is used up). `commits` are base64 of 32 bytes each.
- **Order is the security**: in each round the PC opens first; the phone checks
  `C(A,i)` with *its* digit and *its* `fp` and only then opens its own; the PC checks `C(P,i)`
  before opening round i+1. Nobody opens round i+1 before the other side proved round i.
- Phone mismatch → phone sends `{"type":"error","code":"bad_code"}` and closes; the PC answers
  `error bad_code`. PC mismatch → `error bad_code`. Every message has the 10 s handshake limit.
- On success the phone pins `fp` exactly like a QR fingerprint; later connections use
  normal `auth` (§5.2). `pair_code` is refused over the relay (QR `pair` is allowed there, §12) and on `/stream`.

Errors: `bad_code` (wrong code, no code shown / expired / used, or a mismatch),
`pairing_locked`, `bad_signature`, `bad_message`, `timeout`, `denied` (§2).

### Threat model

Assets: the device key the PC stores (whoever pairs controls the PC at `admin`), and the
fingerprint the phone pins (whoever holds that key can impersonate the PC to the phone).

| Attacker | Result |
|---|---|
| **Passive** on the LAN (sees TLS ciphertext) | Learns nothing: everything after `hello` is inside TLS; the code is never sent. |
| **Active MITM** (answers mDNS / ARP, terminates the phone's TLS with its own certificate) | The phone's `h` contains the attacker's `fp`, the PC's contains the real one, so commitments cannot be relayed. **Against the PC** (getting a key stored while sitting in the middle) the attacker's six commitments go out in `pair_code` before the PC reveals anything: **1 in 1 000 000** per code. **Against the phone** (the phone pins the attacker's certificate) the attacker may delay its `code_commit` and first run a sacrificial attempt against the real PC, which opens digit 1 before noticing the attacker's bad opening (and digit k+1 after k lucky guesses). Summed over those cases: **≈ 1 in 18 000 per code** (5.5 × 10⁻⁵), and every such try burns a real PC code, is shown on the PC (`PairingFailed`; console/desktop app) and audited (`pair_fail … code bad_code`); 3 in 10 minutes lock code pairing for 10 minutes. A phone fooled this way talks to a fake PC that cannot reach the real one (the PC never stored the phone's key). The ordering deliberately puts the weaker side on the phone: the PC — what an attacker really wants — keeps the full 10⁻⁶. |
| **Offline guessing** from a captured exchange | Not possible: openings are only revealed after the other side committed, and a wrong round stops the exchange; `r` values are 256-bit random and never reused. |
| **Attacker pairs its own phone** (no MITM, guesses the code) | 1 in 1 000 000 per code, one try per code, lockout as above. With **Approve on PC** (§2) the owner also has to press Allow. |
| **Shoulder-surfing** the code | Equivalent to scanning the QR: a code is a 5-minute single-use credential; turn on Approve on PC where that matters. |
| **Relay / internet** attacker | Code pairing is refused over the relay. QR pairing is allowed there: it needs the one-time 256-bit token, and the phone pins the fingerprint from the QR, so the relay sees only ciphertext (§12). |
| **Downgrade** (old phone or old agent) | The phone only sends `pair_code` when `hello` has `g4`; an agent without `g4` answers `bad_message`. There is no fallback that skips the proof. |

Not a goal: protecting a PC whose user account is compromised, or a rooted phone.

Why not a full PAKE (SPAKE2/CPace)? It would give 1 in 1 000 000 against a MITM too, but needs
elliptic-curve point arithmetic that Android's platform crypto does not expose; this design
uses only SHA-256, HMAC and the existing ECDSA key, and the one-digit leak is documented above.
(Same structure as Bluetooth passkey entry, digit-wise instead of bit-wise.)

### Interop vector

Both implementations check these bytes (`host/agent/src/g4/code.rs` `interop_vector`,
`android/.../CodePairingTest.interopVector`):

```
host_id    1b4e28ba-2fa1-11d2-883f-0016d3cca427
nonce_a    bm9uY2UtYWdlbnQtMDEyMzQ1Njc4OWFiY2RlZjAxMjM=
nonce_p    bm9uY2UtcGhvbmUtMDEyMzQ1Njc4OWFiY2RlZjAxMjM=
fp         abab…ab (32 × "ab")
device_id  9f8e7d6c-0000-4000-8000-123456789abc
public_key MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEexample
h          a16fbf9b52ce476d4c6c20ab349e56b7d9cd8ec271c959d63cb0a842e71aea4c
C(A,1) digit 4, r = 32 × 0x11   e7cd6b49d48eb85513216cdb6904f2ee3279ef9dec51b8629327f57d3b2fcaf7
C(P,6) digit 9, r = 32 × 0x26   c114c93a81becd531ae0455c5a7255f8ccb62e67e837ff4fad8875eb0faf5b80
```

---

## 2. Approve on PC ("ask before a new phone pairs")

Setting `require_approval` (default **off**), saved in `pairing.json` in the data folder.

- When on, after a `pair` (token + signature valid) or a successful code exchange, the agent
  sends `{"type":"pair_pending","timeout_s":60}` and waits up to **60 s** for the PC owner.
  The phone keeps waiting `timeout_s + 10` s and shows "Waiting for approval on the PC".
- The owner answers on the **Windows toast** (Allow / Deny buttons), in a PC-side UI
  (`HostEvent::PairingRequest` → `G4State::resolve(request_id, allow)`), or in the console
  (`y` / `n` + Enter).
- Deny, no answer within 60 s, or no way to show the request → `error denied`; the device is
  not stored; the QR token / code stays used up.
- A phone that is already paired with the **same** key (re-pair) is not asked about again.
- Every decision is audited: `pair_approval … method=qr|code outcome=allowed|denied|timeout|cancelled`.

Methods (admin):

| method | params → result |
|---|---|
| `pairing.settings.get` | → `{require_approval}` |
| `pairing.settings.set` | `{require_approval}` → `{require_approval}` (audited) |

Older phones ignore `pair_pending` and keep their 20 s ready timeout — enabling the setting
with old phones means they time out unless the owner is quick.

### Host events (agent library, for PC-side UIs — not sent to phones)

`AppState::g4.subscribe()` → `HostEvent` (serde: `{"type": "<snake_case>", …}`):
`pairing_code{code,host_name,expires_unix_ms}`, `pairing_code_ended{reason: used|replaced}`,
`pairing_request{request_id,device_id,device_name,peer,method,expires_unix_ms}`,
`pairing_resolved{request_id,outcome}`, `pairing_failed{method,peer,reason}`,
`pairing_locked{until_unix_ms}`, `paired{device_id,device_name,method}`.
Calls: `new_code_pairing()`, `g4.current_code()`, `g4.cancel_code()`, `g4.pending()`,
`g4.resolve(id, allow)`, `g4.settings()`, `g4.set_require_approval(on)`.

---

## 3. Phone lock timeout (Android only)

Setting **Lock Helm after: Never / 1 / 5 / 15 / 30 min** in the background (default Never).
When Helm comes back to the front after that long it shows a lock screen until
BiometricPrompt (fingerprint / face, or the device PIN/pattern/password) succeeds. A phone with
no screen lock cannot be locked. The separate "Confirm admin actions with fingerprint or
screen lock" (per PC, §11) still applies after unlocking.

---

## 4. USB pairing

USB needs no protocol change: with **USB tethering** on, the phone and PC share a small
network over the cable (Windows sees a "Remote NDIS based Internet Sharing Device" or similar).

- The agent lists the addresses of adapters that look like USB tethering (description or name
  contains `Remote NDIS`, `RNDIS`, `USB Ethernet`, `NCM`, `Android`, `Apple Mobile Device
  Ethernet`) **first** in the pairing link `a=` list, then the usual LAN order. Addresses are
  re-read for every new pairing link, so plugging the phone in after the agent started works.
- Steps: connect the cable → phone Settings → Hotspot & tethering → **USB tethering** → on the
  PC show a pairing QR (or code) → scan. The phone tries the USB address first.
- After pairing, the phone shows the **USB** badge whenever the PC answers on the tethering
  subnet (§12). Windows may ask once whether the new network is Private — choose Private.
- Code pairing over USB works too if the phone's mDNS sees the tethering link; otherwise use
  the QR (or paste the link).
