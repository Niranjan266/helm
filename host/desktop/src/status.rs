//! Turning the agent's state into the words the window and the tray show.

use serde::Serialize;

/// Lifecycle of the in-process agent as the desktop app sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Starting,
    Running,
    /// Stopped on purpose (tray / Settings "Pause connections").
    Paused,
    Stopping,
    /// `helm_agent::start` failed.
    Failed,
    /// The console `helm-agent.exe` already serves this data folder.
    Conflict,
}

/// What the header pill and the speech bubble say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Headline {
    /// CSS hook: `ok`, `wait`, `paused`, `error`.
    pub tone: &'static str,
    /// Header pill text.
    pub label: &'static str,
    /// Small badge above the PC name.
    pub badge: &'static str,
    /// Speech-bubble sentence.
    pub message: String,
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} and {b}"),
        many => format!("{} phones", many.len()),
    }
}

pub fn headline(run: RunState, online: &[String], paired: usize, error: Option<&str>) -> Headline {
    match run {
        RunState::Starting => Headline {
            tone: "wait",
            label: "Starting",
            badge: "Warming up",
            message: "Starting Helm…".into(),
        },
        RunState::Stopping => Headline {
            tone: "wait",
            label: "Stopping",
            badge: "Stopping",
            message: "Closing connections…".into(),
        },
        RunState::Paused => Headline {
            tone: "paused",
            label: "Paused",
            badge: "Paused",
            message: "Connections are paused. Resume to let your phones in.".into(),
        },
        RunState::Failed => Headline {
            tone: "error",
            label: "Error",
            badge: "Needs attention",
            message: format!(
                "Helm could not start: {}",
                error.unwrap_or("unknown error")
            ),
        },
        RunState::Conflict => Headline {
            tone: "error",
            label: "Already running",
            badge: "Needs attention",
            message: "The Helm console agent (helm-agent.exe) is already running. Quit it from \
                      its tray icon, then press Resume."
                .into(),
        },
        RunState::Running if !online.is_empty() => Headline {
            tone: "ok",
            label: "Running",
            badge: "Connected",
            message: format!(
                "{} {} connected. Go steer!",
                join_names(online),
                if online.len() == 1 { "is" } else { "are" }
            ),
        },
        RunState::Running if paired == 0 => Headline {
            tone: "ok",
            label: "Running",
            badge: "Ready for your phone",
            message: "No phones yet. Scan the code with the Helm app.".into(),
        },
        RunState::Running => Headline {
            tone: "ok",
            label: "Running",
            badge: "Ready for your phone",
            message: "Waiting for your phone. Open Helm on it to connect.".into(),
        },
    }
}

/// "Connect from anywhere" as the window shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RelayLine {
    /// CSS hook and short label source: `off`, `paused`, `connecting`, `connected`, `error`.
    pub state: &'static str,
    /// One word for tight places (Home's details table, the Settings card head).
    pub label: &'static str,
    /// One plain sentence: what is happening and, on error, why.
    pub text: String,
}

/// Never a bare "ERROR": an error always carries the agent's reason.
pub fn relay_line(run: RunState, enabled: bool, connected: bool, error: Option<&str>) -> RelayLine {
    let line = |state, label, text: String| RelayLine { state, label, text };
    if !enabled {
        return line("off", "Off", "Off. Phones reach this PC on the same network only.".into());
    }
    if run != RunState::Running {
        return line("paused", "Paused", "On, and waits until Helm is running.".into());
    }
    if connected {
        return line(
            "connected",
            "Connected",
            "Connected. Paired phones can reach this PC from anywhere.".into(),
        );
    }
    match error {
        Some(why) => line("error", "Not connected", format!("Not connected: {why}. Retrying…")),
        None => line("connecting", "Connecting", "Connecting to the relay…".into()),
    }
}

/// The relay address to show: the saved one, else the pre-filled default.
pub fn relay_url_shown(saved: &str) -> String {
    if saved.is_empty() {
        helm_agent::relay::DEFAULT_RELAY_URL.to_string()
    } else {
        saved.to_string()
    }
}

/// "1 of 2 online".
pub fn phones_summary(online: usize, paired: usize) -> String {
    if paired == 0 {
        "None paired".into()
    } else {
        format!("{online} of {paired} online")
    }
}

/// Tray tooltip, e.g. "Helm · running · 1 phone connected".
pub fn tooltip(run: RunState, online: usize) -> String {
    let state = match run {
        RunState::Running => "running",
        RunState::Starting => "starting",
        RunState::Stopping => "stopping",
        RunState::Paused => "paused",
        RunState::Failed | RunState::Conflict => "not running",
    };
    match (run, online) {
        (RunState::Running, 0) => format!("Helm · {state}"),
        (RunState::Running, 1) => format!("Helm · {state} · 1 phone connected"),
        (RunState::Running, n) => format!("Helm · {state} · {n} phones connected"),
        _ => format!("Helm · {state}"),
    }
}

/// Text of the tray's pause item.
pub fn pause_label(run: RunState) -> &'static str {
    match run {
        RunState::Running | RunState::Starting | RunState::Stopping => "Pause connections",
        _ => "Resume connections",
    }
}

/// Whether "pause" from the tray should stop (true) or start (false) the agent.
pub fn pause_stops(run: RunState) -> bool {
    matches!(run, RunState::Running | RunState::Starting)
}

/// Split a PC name for the big heading: "DESKTOP-" plain, the rest highlighted.
pub fn split_host_name(name: &str) -> (String, String) {
    match name.find('-') {
        Some(i) if i + 1 < name.len() => (name[..=i].to_string(), name[i + 1..].to_string()),
        _ => (String::new(), name.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn running_messages_follow_phones() {
        let h = headline(RunState::Running, &names(&["Pixel 8"]), 2, None);
        assert_eq!(h.message, "Pixel 8 is connected. Go steer!");
        assert_eq!((h.tone, h.label, h.badge), ("ok", "Running", "Connected"));
        let h = headline(RunState::Running, &names(&["Pixel 8", "Tab"]), 2, None);
        assert_eq!(h.message, "Pixel 8 and Tab are connected. Go steer!");
        let h = headline(RunState::Running, &names(&["a", "b", "c"]), 3, None);
        assert_eq!(h.message, "3 phones are connected. Go steer!");
        let h = headline(RunState::Running, &[], 0, None);
        assert!(h.message.starts_with("No phones yet"));
        assert_eq!(h.badge, "Ready for your phone");
        let h = headline(RunState::Running, &[], 1, None);
        assert!(h.message.starts_with("Waiting for your phone"));
    }

    #[test]
    fn other_states() {
        assert_eq!(headline(RunState::Paused, &[], 1, None).tone, "paused");
        let f = headline(RunState::Failed, &[], 0, Some("port busy"));
        assert_eq!(f.tone, "error");
        assert!(f.message.ends_with("port busy"));
        let c = headline(RunState::Conflict, &[], 0, None);
        assert_eq!(c.label, "Already running");
        assert!(c.message.contains("helm-agent.exe"));
        assert_eq!(headline(RunState::Starting, &[], 0, None).tone, "wait");
    }

    #[test]
    fn summaries_and_tray_texts() {
        assert_eq!(phones_summary(1, 2), "1 of 2 online");
        assert_eq!(phones_summary(0, 0), "None paired");
        assert_eq!(tooltip(RunState::Running, 0), "Helm · running");
        assert_eq!(
            tooltip(RunState::Running, 1),
            "Helm · running · 1 phone connected"
        );
        assert_eq!(
            tooltip(RunState::Running, 3),
            "Helm · running · 3 phones connected"
        );
        assert_eq!(tooltip(RunState::Conflict, 0), "Helm · not running");
        assert_eq!(pause_label(RunState::Running), "Pause connections");
        assert_eq!(pause_label(RunState::Paused), "Resume connections");
        assert_eq!(pause_label(RunState::Failed), "Resume connections");
        assert!(pause_stops(RunState::Running));
        assert!(!pause_stops(RunState::Paused));
        assert!(!pause_stops(RunState::Conflict));
    }

    #[test]
    fn relay_line_says_what_is_going_on() {
        let off = relay_line(RunState::Running, false, false, None);
        assert_eq!((off.state, off.label), ("off", "Off"));
        let on = relay_line(RunState::Running, true, true, None);
        assert_eq!((on.state, on.label), ("connected", "Connected"));
        let wait = relay_line(RunState::Running, true, false, None);
        assert_eq!((wait.state, wait.label), ("connecting", "Connecting"));
        let bad = relay_line(RunState::Running, true, false, Some("the relay knows a different key for this PC"));
        assert_eq!(bad.state, "error");
        assert!(bad.text.contains("different key"), "{}", bad.text);
        assert_ne!(bad.label, "ERROR");
        assert_eq!(relay_line(RunState::Paused, true, false, None).state, "paused");
    }

    #[test]
    fn relay_address_is_prefilled_until_one_is_saved() {
        let shown = relay_url_shown("");
        assert!(shown.starts_with("wss://"), "{shown}");
        assert_eq!(helm_agent::relay::normalize_url(&shown).unwrap(), shown);
        assert_eq!(relay_url_shown("wss://mine.example"), "wss://mine.example");
    }

    #[test]
    fn host_name_split() {
        assert_eq!(
            split_host_name("DESKTOP-NIRANJAN"),
            ("DESKTOP-".to_string(), "NIRANJAN".to_string())
        );
        assert_eq!(
            split_host_name("WORKSTATION"),
            (String::new(), "WORKSTATION".to_string())
        );
        assert_eq!(split_host_name("PC-"), (String::new(), "PC-".to_string()));
    }
}
