//! Live toast. Ignored by default because it shows one real "Helm test" toast:
//! `cargo test -- --ignored live_toast`

#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use helm_win_notify::{AppRegistration, Notifier, ReplyBox, Toast, ToastAction, ToastEvent, unregister_app};

#[test]
#[ignore = "shows a real toast notification"]
fn live_toast() {
    let reg = AppRegistration { aumid: "Helm.Test".into(), display_name: "Helm (test)".into(), icon_path: None };
    let notifier = Notifier::new(&reg).expect("register + notifier");
    let toast = Toast {
        title: "Helm test".into(),
        body: "Toast from the helm-win-notify live test.".into(),
        app_name: Some("helm-win-notify".into()),
        actions: vec![ToastAction { id: "ok".into(), label: "OK".into() }],
        reply: Some(ReplyBox { id: "reply".into(), placeholder: "Reply…".into(), button_label: "Send".into() }),
        tag: Some("live".into()),
        group: Some("test".into()),
        silent: true,
        ..Default::default()
    };
    let events: Arc<Mutex<Vec<ToastEvent>>> = Arc::default();
    let ev = events.clone();
    let shown = notifier.show(&toast, move |e| ev.lock().unwrap().push(e)).expect("show");
    std::thread::sleep(Duration::from_secs(4));
    let got = events.lock().unwrap().clone();
    println!("events: {got:?}");
    assert!(!got.iter().any(|e| matches!(e, ToastEvent::Failed { .. })), "toast failed: {got:?}");
    shown.hide().ok();
    notifier.clear().ok();
    unregister_app("Helm.Test").unwrap();
}
