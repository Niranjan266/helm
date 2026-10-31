# Contributing to Helm

Thanks for helping! Helm is fully open source under the MIT license.

## The clean-room rule (most important)

Helm is **our own work**. Other projects — WinDock, KDE Connect, Unified Remote, Macro Deck,
Sunshine, Moonlight, RustDesk, LocalSend and others — are great **references for ideas**:
features, UX, which Windows APIs exist, what problems to expect.

But:

- **Never copy code** from another project into this repo — not functions, not files, not
  "small snippets", not translated/ported versions of their code. This applies to every
  license, including MIT and GPL.
- Write your implementation from the platform documentation (Microsoft Learn, Android
  developers, RFCs, crate docs) and from your own understanding.
- Assets (icons, images, sounds) must be made by us or come from a source whose license
  allows use (e.g. CC0), with the source noted in the PR.
- Normal open-source **libraries used as dependencies** (crates, Maven packages) are fine —
  that is using them, not copying them. Prefer permissive licenses (MIT/Apache/BSD).

## No AI features

Helm has no AI features — no AI APIs and no local models. Please don't add any.

## Before opening a pull request

- `cargo test` in `host/agent` passes.
- `.\gradlew.bat testDebugUnitTest assembleDebug` in `android/` passes.
- Update `Docs/PROTOCOL.md` first if you change any message.
- Never commit pairing tokens, device keys, or anything from `%APPDATA%\Helm`.
