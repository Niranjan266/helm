# helm-win-notify

Windows toast notifications for the Helm PC agent (mirrored phone notifications): title, body,
source-app attribution, optional circle-cropped image, up to 5 buttons and an inline reply box,
with a callback for activation, dismissal and failure. WinRT `ToastNotificationManager`, for an
**unpackaged** app. Clean-room, from the Microsoft Learn documentation only.

```rust
use helm_win_notify::*;
let n = Notifier::new(&AppRegistration { aumid: "Helm.Agent".into(), display_name: "Helm".into(), icon_path: None })?;
let toast = Toast {
    title: "Alice".into(), body: "Lunch?".into(), app_name: Some("WhatsApp".into()),
    image: Some(r"C:\...\avatar.png".into()),
    actions: vec![ToastAction { id: "read".into(), label: "Mark read".into() }],
    reply: Some(ReplyBox { id: "reply".into(), placeholder: "Reply…".into(), button_label: "Send".into() }),
    tag: Some("msg-42".into()), group: Some("whatsapp".into()), silent: false,
};
let shown = n.show(&toast, |ev| match ev {
    ToastEvent::Activated { action_id, reply } => { /* None = body click; reply = typed text */ }
    ToastEvent::Dismissed { reason } => {}
    ToastEvent::Failed { message } => {}
})?;
shown.hide()?; n.remove("msg-42", Some("whatsapp"))?; n.clear()?;
```

* **Registration:** `Notifier::new` / `register_app` writes `DisplayName` (+ `IconUri`) under
  `HKCU\Software\Classes\AppUserModelId\<aumid>` (no admin), then `CreateToastNotifierWithId`.
  `unregister_app` removes the key.
* **Threading:** `Notifier` and `ShownToast` are `Send + Sync`; the process MTA is kept alive with
  `CoIncrementMTAUsage`, so any thread may call. Callbacks run on a system thread.
* **Activation** is delivered through the in-process `Activated` event: only while the agent runs.
  No COM activator is registered, so clicking an old toast in Action Center after the agent exits
  does nothing.
* `toast_xml` (pure) builds the ToastGeneric XML with escaping; button arguments are
  `helm:action:<id>`, the body is `helm:body`. Images must be absolute paths (`file:///` URI).
* Non-Windows builds: `Notifier::new` returns `Error::Unsupported`.

## Tests

```text
cargo test                     # pure: XML, escaping, file URIs, arguments, validation, serde
cargo test -- --ignored        # live: registers Helm.Test, shows one "Helm test" toast, hides it, unregisters
cargo clippy --all-targets
```

Live test run once on the dev PC: shown without a `Failed` event.

`windows` 0.62 features: `Data_Xml_Dom`, `Foundation`, `Foundation_Collections`,
`UI_Notifications`, `Win32_Foundation`, `Win32_Security`, `Win32_System_Com`, `Win32_System_Registry`.
