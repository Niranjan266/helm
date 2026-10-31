use helm_plugins::trigger::{
    DEFAULT_DEBOUNCE_MS, Event, FakeClock, Firing, Trigger, TriggerEngine, TriggerError, Weekday,
    When,
};
use serde_json::{Value, json};

fn trig(id: &str, when: When, action: Value) -> Trigger {
    Trigger {
        id: id.into(),
        when,
        action,
        enabled: true,
        debounce_ms: None,
    }
}

fn ids(f: &[Firing]) -> Vec<&str> {
    f.iter().map(|f| f.trigger_id.as_str()).collect()
}

fn app(exe: &str) -> Event {
    Event::AppStarted { exe: exe.into() }
}

#[test]
fn time_trigger_fires_once_on_matching_days() {
    let clock = FakeClock::at(Weekday::Mon, 7, 29);
    let mut e = TriggerEngine::new(
        clock.clone(),
        vec![trig(
            "work",
            When::Time("07:30 mon-fri".into()),
            json!({"launch":"Outlook"}),
        )],
    )
    .unwrap();
    assert!(e.tick().is_empty());
    clock.advance_ms(30_000);
    assert!(e.tick().is_empty()); // 07:29:30
    clock.advance_ms(30_000);
    let f = e.tick(); // 07:30:00
    assert_eq!(ids(&f), ["work"]);
    assert_eq!(f[0].action, json!({"launch":"Outlook"}));
    for _ in 0..3 {
        clock.advance_ms(10_000);
        assert!(e.tick().is_empty(), "same minute must not re-fire");
    }
    // Every day through Sunday at 07:30: fires Tue-Fri, not Sat/Sun.
    let mut fired_days = Vec::new();
    for _ in 0..6 {
        clock.advance_min(24 * 60);
        // Tick around the target minute like a 30 s poller would.
        for _ in 0..4 {
            if !e.tick().is_empty() {
                fired_days.push(e.clock().clone());
            }
            clock.advance_ms(30_000);
        }
        clock.shift_local_min(-2);
    }
    assert_eq!(fired_days.len(), 4);
}

#[test]
fn late_ticks_catch_up_within_window_only() {
    let clock = FakeClock::at(Weekday::Sat, 21, 55);
    let mut e = TriggerEngine::new(
        clock.clone(),
        vec![
            trig("a", When::Time("21:58".into()), json!({"text":"a"})),
            trig("b", When::Time("22:30".into()), json!({"text":"b"})),
        ],
    )
    .unwrap();
    assert!(e.tick().is_empty());
    clock.advance_min(4); // 21:59 — 21:58 is within the 5-minute catch-up
    assert_eq!(ids(&e.tick()), ["a"]);
    clock.advance_min(40); // 22:39 — 22:30 is 9 minutes late: skipped
    assert!(e.tick().is_empty());
}

#[test]
fn dst_fall_back_does_not_refire_but_big_jump_resets() {
    let clock = FakeClock::at(Weekday::Sun, 1, 30);
    let mut e = TriggerEngine::new(
        clock.clone(),
        vec![trig("t", When::Time("01:30".into()), json!({"text":"x"}))],
    )
    .unwrap();
    assert_eq!(ids(&e.tick()), ["t"]);
    clock.advance_min(60); // 02:30
    assert!(e.tick().is_empty());
    clock.shift_local_min(-60); // DST: back to 01:30
    assert!(e.tick().is_empty());
    clock.advance_min(1);
    assert!(e.tick().is_empty());

    // A >2 h backward jump (user changed the clock) resets the schedule.
    clock.shift_local_min(-24 * 60 - 1); // previous day 01:30
    assert_eq!(ids(&e.tick()), ["t"]);
}

#[test]
fn app_start_is_edge_triggered_and_debounced() {
    let clock = FakeClock::at(Weekday::Wed, 12, 0);
    let mut e = TriggerEngine::new(
        clock.clone(),
        vec![trig(
            "obs",
            When::AppStart {
                exe: "OBS64.exe".into(),
            },
            json!({"remote":"obs.toggle_record"}),
        )],
    )
    .unwrap();
    assert_eq!(
        ids(&e.handle(&app(r"C:\Program Files\obs-studio\bin\64bit\obs64.exe"))),
        ["obs"]
    );
    // A second instance while the first runs: no fire.
    assert!(e.handle(&app("obs64.exe")).is_empty());
    e.handle(&Event::AppExited {
        exe: "obs64.exe".into(),
    });
    assert!(
        e.handle(&app("obs64")).is_empty(),
        "still one instance running"
    );
    e.handle(&Event::AppExited {
        exe: "obs64".into(),
    });
    e.handle(&Event::AppExited {
        exe: "obs64".into(),
    });
    // Restart within the debounce window: suppressed.
    clock.advance_ms(500);
    assert!(e.handle(&app("obs64.exe")).is_empty());
    e.handle(&Event::AppExited {
        exe: "obs64.exe".into(),
    });
    clock.advance_ms(DEFAULT_DEBOUNCE_MS);
    assert_eq!(ids(&e.handle(&app("obs64.exe"))), ["obs"]);
    assert!(e.handle(&app("notepad.exe")).is_empty());
}

#[test]
fn phone_connected_any_or_specific_device() {
    let clock = FakeClock::at(Weekday::Thu, 9, 0);
    let mut e = TriggerEngine::new(
        clock.clone(),
        vec![
            trig(
                "any",
                When::PhoneConnected { device_id: None },
                json!({"media":"pause"}),
            ),
            trig(
                "pixel",
                When::PhoneConnected {
                    device_id: Some("dev-pixel".into()),
                },
                json!({"text":"hi pixel"}),
            ),
        ],
    )
    .unwrap();
    let conn = |d: &str| Event::PhoneConnected {
        device_id: d.into(),
    };
    assert_eq!(ids(&e.handle(&conn("dev-other"))), ["any"]);
    clock.advance_ms(10_000);
    assert_eq!(ids(&e.handle(&conn("dev-pixel"))), ["any", "pixel"]);
    // Duplicate connect events (e.g. a second socket) are deduped.
    clock.advance_ms(10_000);
    assert!(e.handle(&conn("dev-pixel")).is_empty());
    e.handle(&Event::PhoneDisconnected {
        device_id: "dev-pixel".into(),
    });
    clock.advance_ms(10_000);
    assert_eq!(ids(&e.handle(&conn("dev-pixel"))), ["any", "pixel"]);
}

#[test]
fn wifi_fires_on_change_to_ssid_with_debounce_for_flapping() {
    let clock = FakeClock::at(Weekday::Fri, 18, 0);
    let mut home = trig(
        "home",
        When::Wifi {
            ssid: "HomeNet".into(),
        },
        json!({"volume_step":-0.2}),
    );
    home.debounce_ms = Some(60_000);
    let mut e = TriggerEngine::new(clock.clone(), vec![home]).unwrap();
    let wifi = |s: &str| Event::WifiConnected { ssid: s.into() };
    assert_eq!(ids(&e.handle(&wifi("HomeNet"))), ["home"]);
    assert!(e.handle(&wifi("HomeNet")).is_empty(), "same SSID again");
    assert!(
        e.handle(&wifi("homenet")).is_empty(),
        "SSID match is case-sensitive"
    );
    // Flap: disconnect/reconnect within the debounce window.
    e.handle(&Event::WifiDisconnected);
    clock.advance_ms(5_000);
    assert!(e.handle(&wifi("HomeNet")).is_empty());
    e.handle(&wifi("Cafe"));
    clock.advance_ms(60_000);
    assert_eq!(ids(&e.handle(&wifi("HomeNet"))), ["home"]);
}

#[test]
fn disabled_triggers_and_identical_actions_in_one_batch() {
    let clock = FakeClock::at(Weekday::Tue, 8, 0);
    let mut off = trig("off", When::Time("08:00".into()), json!({"text":"off"}));
    off.enabled = false;
    let mut e = TriggerEngine::new(
        clock,
        vec![
            off,
            trig("a", When::Time("08:00".into()), json!({"hotkey":"win+d"})),
            trig(
                "b",
                When::Time("08:00 tue".into()),
                json!({"hotkey":"win+d"}),
            ),
            trig("c", When::Time("08:00".into()), json!({"text":"c"})),
        ],
    )
    .unwrap();
    assert_eq!(ids(&e.tick()), ["a", "c"]);
}

#[test]
fn validation_errors() {
    let clock = FakeClock::at(Weekday::Mon, 0, 0);
    let ok = json!({"text":"x"});
    let cases = [
        (
            vec![trig("", When::Time("10:00".into()), ok.clone())],
            "BadId",
        ),
        (
            vec![
                trig("a", When::Time("10:00".into()), ok.clone()),
                trig("a", When::Time("11:00".into()), ok.clone()),
            ],
            "DuplicateId",
        ),
        (
            vec![trig("a", When::Time("25:00".into()), ok.clone())],
            "BadTime",
        ),
        (
            vec![trig("a", When::AppStart { exe: " ".into() }, ok.clone())],
            "BadCondition",
        ),
        (
            vec![trig("a", When::Wifi { ssid: "".into() }, ok.clone())],
            "BadCondition",
        ),
        (
            vec![trig(
                "a",
                When::Time("10:00".into()),
                json!({"format":"c:"}),
            )],
            "BadAction",
        ),
    ];
    for (ts, want) in cases {
        let err = TriggerEngine::new(clock.clone(), ts).unwrap_err();
        let got = match err {
            TriggerError::BadId => "BadId",
            TriggerError::DuplicateId(_) => "DuplicateId",
            TriggerError::BadTime(_) => "BadTime",
            TriggerError::BadCondition { .. } => "BadCondition",
            TriggerError::BadAction { .. } => "BadAction",
        };
        assert_eq!(got, want);
    }
    // set_triggers is all-or-nothing.
    let mut e = TriggerEngine::new(
        clock,
        vec![trig("keep", When::Time("00:00".into()), ok.clone())],
    )
    .unwrap();
    assert!(
        e.set_triggers(vec![trig("x", When::Time("nope".into()), ok)])
            .is_err()
    );
    assert_eq!(e.triggers().count(), 1);
}

#[test]
fn json_config_round_trip() {
    let cfg = r#"[
      {"id":"morning","when":{"time":"07:30 mon-fri"},"action":{"launch":"Outlook"}},
      {"id":"obs","when":{"app_start":{"exe":"obs64.exe"}},"action":{"remote":"obs.scene:Live"},"debounce_ms":10000},
      {"id":"phone","when":{"phone_connected":{}},"action":{"media":"pause"},"enabled":false},
      {"id":"home","when":{"wifi":{"ssid":"HomeNet"}},"action":{"macro":[{"volume_step":-0.2},{"delay_ms":100}]}}
    ]"#;
    let ts: Vec<Trigger> = serde_json::from_str(cfg).unwrap();
    assert_eq!(ts[1].debounce_ms, Some(10_000));
    assert!(!ts[2].enabled);
    assert_eq!(ts[2].when, When::PhoneConnected { device_id: None });
    let back: Vec<Trigger> = serde_json::from_str(&serde_json::to_string(&ts).unwrap()).unwrap();
    assert_eq!(back, ts);
    TriggerEngine::new(FakeClock::at(Weekday::Mon, 0, 0), ts).unwrap();
    let ev: Event = serde_json::from_str(r#"{"type":"wifi_connected","ssid":"x"}"#).unwrap();
    assert_eq!(ev, Event::WifiConnected { ssid: "x".into() });
}

/// A scripted day of events through a fake stream, checking the full output.
#[test]
fn fake_event_stream_scenario() {
    enum Step {
        Wait(u64),
        Ev(Event),
    }
    use Step::*;
    let clock = FakeClock::at(Weekday::Mon, 8, 58);
    let mut e = TriggerEngine::new(
        clock.clone(),
        vec![
            trig(
                "standup",
                When::Time("09:00 weekdays".into()),
                json!({"open":"https://meet.example/standup"}),
            ),
            trig(
                "game",
                When::AppStart {
                    exe: "game.exe".into(),
                },
                json!({"media":"pause"}),
            ),
            trig(
                "office",
                When::Wifi {
                    ssid: "Office".into(),
                },
                json!({"text":"at work"}),
            ),
        ],
    )
    .unwrap();
    let script = vec![
        Ev(Event::WifiConnected {
            ssid: "Office".into(),
        }),
        Wait(60_000),
        Wait(60_000),
        Ev(app("game.exe")),
        Ev(app("game.exe")),
        Wait(30_000),
        Ev(Event::WifiConnected {
            ssid: "Office".into(),
        }),
        Ev(Event::AppExited {
            exe: "game.exe".into(),
        }),
        Ev(Event::AppExited {
            exe: "game.exe".into(),
        }),
        Wait(5_000),
        Ev(app("GAME.EXE")),
    ];
    let mut log = Vec::new();
    for step in script {
        let fired = match step {
            Wait(ms) => {
                clock.advance_ms(ms);
                e.tick()
            }
            Ev(ev) => e.handle(&ev),
        };
        log.extend(fired.into_iter().map(|f| f.trigger_id));
    }
    assert_eq!(log, ["office", "standup", "game", "game"]);
}
