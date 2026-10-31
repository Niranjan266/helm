# Helm — Android app

The phone side of Helm (Phase 0): find PCs on the LAN, pair by QR code, connect over a
pinned-TLS WebSocket, remember trusted PCs, and call a few test methods. The wire contract is
[`Docs/PROTOCOL.md`](../Docs/PROTOCOL.md).

## Layout

```
app/src/main/java/com/niranjan/helm/
  core/     plain Kotlin/JVM, no android.* imports (unit-tested on the JVM)
    PairingUri.kt       helm://pair?... parser + validation (§4)
    Identity.kt         DeviceSigner interface, software signer, signed-string format (§5)
    PinnedTls.kt        fingerprint pinning trust manager, OkHttp transport (§1)
    HelmConnection.kt   WebSocket client: hello -> pair/auth -> ready, req/res, state flow
    SavedPc.kt          saved-PC model + JSON codec, address ordering
  device/   Android pieces: Keystore signer, SharedPreferences stores, NSD discovery
  ui/       Compose (Material 3, dark): Home + Connected screens, MainActivity, ViewModel
  HelmApp.kt            app-scoped holder for identity, stores, discovery and the connection
```

## Build

Requirements: Android SDK (platform 34), and **JDK 17 or 21**. AGP does not run on newer JDKs,
so point `JAVA_HOME` at Android Studio's bundled JBR (Java 21):

```powershell
$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"
cd android
.\gradlew.bat testDebugUnitTest assembleDebug
```

`local.properties` must contain `sdk.dir=...` (Android Studio writes it; it is git-ignored).

The debug APK is `app/build/outputs/apk/debug/app-debug.apk`. Install with
`adb install -r app\build\outputs\apk\debug\app-debug.apk`.

You can start pairing without a camera by sending the deep link:

```powershell
adb shell am start -a android.intent.action.VIEW -d "'helm://pair?v=1&id=...&t=...'"
```

## Tests

`.\gradlew.bat testDebugUnitTest` runs JVM tests only (no device needed):

- `PairingUriTest` — parsing, URL-encoded names, bad input.
- `SigningTest` — exact `helm-v1|<purpose>|<host_id>|<nonce>` string, SPKI key, DER signature.
- `PinnedTlsTest` — SHA-256 pinning trust manager, real TLS against MockWebServer.
- `HelmConnectionTest` — a fake agent on MockWebServer: pair success, `bad_token`, auth,
  wrong key, host mismatch, address fallback, req/res (incl. out-of-order), timeouts, and the
  whole handshake over `wss://` with pinning.
- `SavedPcTest` — saved-PC JSON and address order.
- `E2eRealAgentTest` — **opt-in**, skipped unless `HELM_E2E_URI` is set (below).

## Opt-in end-to-end test against the real PC agent

1. Start the Rust agent (`host/agent`) and copy the `helm://pair?...` link it prints
   (the token is single-use and valid for 5 minutes).
2. Run, from `android/`:

```powershell
$env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr"
$env:HELM_E2E_URI = 'helm://pair?v=1&id=...&n=...&a=...&p=47820&fp=...&t=...'
.\gradlew.bat testDebugUnitTest --tests "com.niranjan.helm.core.E2eRealAgentTest" --rerun
Remove-Item Env:HELM_E2E_URI
```

Use single quotes so PowerShell does not touch the `&` characters. The test pairs with a fresh
software key over the real pinned TLS client, calls `system.ping` and `system.info`,
disconnects, reconnects with `auth`, calls `devices.list`, revokes itself with
`devices.revoke`, and checks that a further `auth` gets `unknown_device`. Results are printed
with a `[helm-e2e]` prefix.
