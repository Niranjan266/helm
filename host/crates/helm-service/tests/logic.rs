//! Pure-logic tests. Nothing here touches the SCM or creates processes.

use std::time::Duration;

use helm_service::backoff::{Backoff, BackoffPolicy};
use helm_service::cli::{self, Command, StartMode};
use helm_service::cmdline;
use helm_service::supervisor::{Action, Event, NO_USER_POLL, Phase, Supervisor};

const fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn policy() -> BackoffPolicy {
    BackoffPolicy {
        base: s(1),
        max: s(60),
        stable_after: s(30),
    }
}

// ---------------------------------------------------------------- backoff

#[test]
fn backoff_doubles_and_caps() {
    let mut b = Backoff::new(policy());
    let delays: Vec<u64> = (0..9).map(|_| b.on_exit(s(2)).as_secs()).collect();
    assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
    assert_eq!(b.failures(), 9);
}

#[test]
fn backoff_resets_after_stable_run() {
    let mut b = Backoff::new(policy());
    for _ in 0..5 {
        b.on_exit(Duration::ZERO);
    }
    assert_eq!(b.on_exit(s(30)), s(1), "a 30 s run counts as healthy");
    assert_eq!(b.on_exit(s(1)), s(2));
    b.reset();
    assert_eq!(b.on_launch_failure(), s(1));
}

#[test]
fn backoff_survives_huge_streaks() {
    let mut b = Backoff::new(policy());
    for _ in 0..10_000 {
        assert!(b.on_exit(Duration::ZERO) <= s(60));
    }
}

// ---------------------------------------------------------------- supervisor

fn sup() -> Supervisor {
    Supervisor::new(policy())
}

fn session(active: Option<u32>) -> Event {
    Event::SessionChanged {
        active,
        logon: false,
    }
}

#[test]
fn launches_in_active_session_at_start() {
    let mut m = sup();
    assert_eq!(
        m.handle(s(0), session(Some(1))),
        [Action::Launch { session: 1 }]
    );
    assert_eq!(m.running_session(), Some(1));
}

#[test]
fn no_console_session_waits() {
    let mut m = sup();
    assert!(m.handle(s(0), session(None)).is_empty());
    assert!(m.handle(s(100), Event::Tick).is_empty());
    assert_eq!(
        m.handle(s(101), session(Some(2))),
        [Action::Launch { session: 2 }]
    );
}

#[test]
fn crash_restarts_with_backoff() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    assert!(
        m.handle(s(1), Event::ChildExited { ran_for: s(1) })
            .is_empty()
    );
    assert_eq!(
        m.phase(),
        Phase::Backoff {
            session: 1,
            until: s(2)
        }
    );
    assert!(m.handle(s(1), Event::Tick).is_empty(), "too early");
    assert_eq!(m.handle(s(2), Event::Tick), [Action::Launch { session: 1 }]);

    // Second quick crash: 2 s delay.
    m.handle(s(3), Event::ChildExited { ran_for: s(1) });
    assert_eq!(
        m.phase(),
        Phase::Backoff {
            session: 1,
            until: s(5)
        }
    );
    assert!(m.handle(s(4), Event::Tick).is_empty());
    assert_eq!(m.handle(s(5), Event::Tick), [Action::Launch { session: 1 }]);

    // A long healthy run resets the streak.
    m.handle(s(500), Event::ChildExited { ran_for: s(495) });
    assert_eq!(
        m.phase(),
        Phase::Backoff {
            session: 1,
            until: s(501)
        }
    );
}

#[test]
fn lock_and_unlock_do_not_restart() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    assert!(m.handle(s(5), session(Some(1))).is_empty());
    assert!(
        m.handle(
            s(6),
            Event::SessionChanged {
                active: Some(1),
                logon: true
            }
        )
        .is_empty()
    );
    assert_eq!(m.running_session(), Some(1));
}

#[test]
fn fast_user_switch_moves_agent() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    // User B switches in: console now belongs to session 3.
    assert_eq!(m.handle(s(10), session(Some(3))), [Action::Terminate]);
    assert_eq!(m.phase(), Phase::Switching { next: Some(3) });
    assert_eq!(
        m.handle(s(10), Event::ChildExited { ran_for: s(10) }),
        [Action::Launch { session: 3 }]
    );
    // Switch back to session 1.
    assert_eq!(m.handle(s(20), session(Some(1))), [Action::Terminate]);
    assert_eq!(
        m.handle(s(20), Event::ChildExited { ran_for: s(10) }),
        [Action::Launch { session: 1 }]
    );
}

#[test]
fn switching_follows_latest_session() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    m.handle(s(1), session(Some(2)));
    assert!(
        m.handle(s(1), session(Some(4))).is_empty(),
        "still waiting for the kill"
    );
    assert_eq!(
        m.handle(s(2), Event::ChildExited { ran_for: s(2) }),
        [Action::Launch { session: 4 }]
    );
}

#[test]
fn console_disconnect_parks_the_agent() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    assert_eq!(m.handle(s(5), session(None)), [Action::Terminate]);
    assert!(
        m.handle(s(5), Event::ChildExited { ran_for: s(5) })
            .is_empty()
    );
    assert!(matches!(
        m.phase(),
        Phase::WaitingForUser { session: None, .. }
    ));
    assert!(m.handle(s(1000), Event::Tick).is_empty());
}

#[test]
fn logoff_then_logon_same_session() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    // Logoff kills the agent; relaunch attempt finds no user.
    m.handle(s(100), Event::ChildExited { ran_for: s(100) });
    assert_eq!(
        m.handle(s(101), Event::Tick),
        [Action::Launch { session: 1 }]
    );
    assert!(
        m.handle(s(101), Event::LaunchFailed { no_user: true })
            .is_empty()
    );
    assert_eq!(
        m.phase(),
        Phase::WaitingForUser {
            session: Some(1),
            retry_at: s(101) + NO_USER_POLL
        }
    );
    // Logon notice launches immediately, before the poll fires.
    assert_eq!(
        m.handle(
            s(103),
            Event::SessionChanged {
                active: Some(1),
                logon: true
            }
        ),
        [Action::Launch { session: 1 }]
    );
}

#[test]
fn no_user_is_polled() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    m.handle(s(0), Event::LaunchFailed { no_user: true });
    assert!(m.handle(s(9), Event::Tick).is_empty());
    assert_eq!(
        m.handle(s(10), Event::Tick),
        [Action::Launch { session: 1 }]
    );
}

#[test]
fn launch_error_backs_off() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    assert!(
        m.handle(s(0), Event::LaunchFailed { no_user: false })
            .is_empty()
    );
    assert_eq!(
        m.phase(),
        Phase::Backoff {
            session: 1,
            until: s(1)
        }
    );
    // Same-session notices do not defeat the backoff ...
    assert!(m.handle(s(0), session(Some(1))).is_empty());
    // ... but a logon or a different session starts fresh.
    assert_eq!(
        m.handle(
            s(0),
            Event::SessionChanged {
                active: Some(1),
                logon: true
            }
        ),
        [Action::Launch { session: 1 }]
    );
    m.handle(s(0), Event::LaunchFailed { no_user: false });
    assert_eq!(
        m.handle(s(0), session(Some(2))),
        [Action::Launch { session: 2 }]
    );
}

#[test]
fn stop_while_running_terminates_then_exits() {
    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    assert_eq!(m.handle(s(1), Event::StopRequested), [Action::Terminate]);
    assert_eq!(m.phase(), Phase::Stopping);
    assert!(
        m.handle(s(1), session(Some(2))).is_empty(),
        "no relaunch while stopping"
    );
    assert!(m.handle(s(1), Event::Tick).is_empty());
    assert_eq!(
        m.handle(s(2), Event::ChildExited { ran_for: s(2) }),
        [Action::Exit]
    );
    assert!(m.is_stopped());
    assert!(
        m.handle(s(3), session(Some(1))).is_empty(),
        "stopped is final"
    );
}

#[test]
fn stop_while_idle_exits_immediately() {
    let mut m = sup();
    m.handle(s(0), session(None));
    assert_eq!(m.handle(s(1), Event::StopRequested), [Action::Exit]);

    let mut m = sup();
    m.handle(s(0), session(Some(1)));
    m.handle(s(1), Event::ChildExited { ran_for: s(1) });
    assert_eq!(m.handle(s(1), Event::StopRequested), [Action::Exit]);
}

#[test]
fn stray_events_are_ignored() {
    let mut m = sup();
    assert!(
        m.handle(s(0), Event::ChildExited { ran_for: s(1) })
            .is_empty()
    );
    assert!(
        m.handle(s(0), Event::LaunchFailed { no_user: false })
            .is_empty()
    );
    assert!(m.handle(s(0), Event::Tick).is_empty());
}

// ---------------------------------------------------------------- command line

#[test]
fn command_line_quoting() {
    let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert_eq!(
        cmdline::build(
            r"C:\Program Files\Helm\helm-agent.exe",
            &a(&["--port", "47820"])
        ),
        r#""C:\Program Files\Helm\helm-agent.exe" --port 47820"#
    );
    assert_eq!(
        cmdline::build(
            "a.exe",
            &a(&[
                "",
                "two words",
                r#"say "hi""#,
                r"C:\dir\",
                r"C:\my dir\",
                r"a\\b"
            ])
        ),
        r#""a.exe" "" "two words" "say \"hi\"" C:\dir\ "C:\my dir\\" a\\b"#
    );
    assert_eq!(
        cmdline::build("a.exe", &a(&[r#"x\"y"#])),
        r#""a.exe" "x\\\"y""#
    );
}

// ---------------------------------------------------------------- cli

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn cli_parsing() {
    assert_eq!(cli::parse(&[]).unwrap(), Command::Help);
    assert_eq!(
        cli::parse(&args(&["uninstall"])).unwrap(),
        Command::Uninstall
    );
    assert_eq!(cli::parse(&args(&["start"])).unwrap(), Command::Start);
    assert!(cli::parse(&args(&["stop", "now"])).is_err());
    assert!(cli::parse(&args(&["frobnicate"])).is_err());

    assert_eq!(
        cli::parse(&args(&[
            "install",
            "--agent",
            r"C:\h\helm-agent.exe",
            "--start",
            "demand",
            "--",
            "--port",
            "1"
        ]))
        .unwrap(),
        Command::Install {
            agent: Some(r"C:\h\helm-agent.exe".into()),
            start: StartMode::Demand,
            agent_args: args(&["--port", "1"]),
        }
    );
    assert_eq!(
        cli::parse(&args(&["install"])).unwrap(),
        Command::Install {
            agent: None,
            start: StartMode::Auto,
            agent_args: vec![]
        }
    );
    assert!(cli::parse(&args(&["install", "--start", "boot"])).is_err());
    assert!(
        cli::parse(&args(&["run", "--start", "auto"])).is_err(),
        "--start is install-only"
    );
    assert!(cli::parse(&args(&["install", "--agent"])).is_err());
}

#[test]
fn run_arguments_roundtrip() {
    let stored = cli::run_arguments(
        Some(std::path::Path::new(r"C:\h\helm-agent.exe")),
        &args(&["--port", "1"]),
    );
    assert_eq!(
        stored,
        args(&[
            "run",
            "--agent",
            r"C:\h\helm-agent.exe",
            "--",
            "--port",
            "1"
        ])
    );
    assert_eq!(
        cli::parse(&stored).unwrap(),
        Command::Run {
            agent: Some(r"C:\h\helm-agent.exe".into()),
            agent_args: args(&["--port", "1"])
        }
    );
    assert_eq!(cli::run_arguments(None, &[]), args(&["run"]));
}
