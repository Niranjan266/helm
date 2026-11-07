//! HelmSignIn (pre-login service) pure logic. Nothing here touches the SCM.

use std::path::PathBuf;
use std::time::Duration;

use helm_service::prelogin::{Handover, RunArgs, SIGN_OUT_GRACE, Step, valid_sid};

const SID: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";

const fn s(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn sids() {
    assert!(valid_sid(SID));
    assert!(valid_sid("S-1-5-18"));
    assert!(!valid_sid("S-1-5"));
    assert!(!valid_sid("S-1-5-21-x"));
    assert!(!valid_sid("S-1-5--1"));
    assert!(!valid_sid("s-1-5-18 --agent x"));
    assert!(!valid_sid(""));
}

#[test]
fn run_args_round_trip_and_validation() {
    let a = RunArgs {
        data_dir: PathBuf::from(r"C:\Users\Me Too\AppData\Roaming\Helm"),
        owner_sid: SID.into(),
    };
    assert_eq!(RunArgs::parse(&a.to_args()), Ok(a.clone()));
    let bad =
        |v: &[&str]| RunArgs::parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err();
    assert!(bad(&["--data-dir", r"C:\x"]));
    assert!(bad(&["--owner", SID]));
    assert!(bad(&["--data-dir", r"relative\Helm", "--owner", SID]));
    assert!(bad(&["--data-dir", r#"C:\a"b"#, "--owner", SID]));
    assert!(bad(&["--data-dir", r"C:\x", "--owner", "nobody"]));
    assert!(bad(&["--data-dir", r"C:\x", "--owner", SID, "--extra"]));
}

#[test]
fn serves_at_boot_and_hands_over_at_sign_in() {
    let mut h = Handover::new();
    // Boot: nobody signed in -> serve straight away.
    assert_eq!(h.step(s(0), Some(false)), Some(Step::Start));
    assert!(h.serving());
    assert_eq!(h.step(s(2), Some(false)), None);
    // Owner signs in -> stop at once.
    assert_eq!(h.step(s(4), Some(true)), Some(Step::Stop));
    assert_eq!(h.step(s(6), Some(true)), None);
    // Signs out: wait out the grace period, then serve again.
    assert_eq!(h.step(s(7), Some(false)), None);
    assert_eq!(
        h.step(s(6) + SIGN_OUT_GRACE, Some(false)),
        Some(Step::Start)
    );
}

#[test]
fn unknown_presence_changes_nothing() {
    let mut h = Handover::new();
    assert_eq!(h.step(s(0), None), None);
    assert!(!h.serving());
    assert_eq!(h.step(s(1), Some(false)), Some(Step::Start));
    assert_eq!(h.step(s(2), None), None);
    assert!(h.serving());
}

#[test]
fn failed_start_backs_off() {
    let mut h = Handover::new();
    assert_eq!(h.step(s(0), Some(false)), Some(Step::Start));
    h.agent_failed(s(0), s(0)); // 1 s
    assert_eq!(h.step(s(0), Some(false)), None);
    assert_eq!(h.step(s(1), Some(false)), Some(Step::Start));
    h.agent_failed(s(1), s(0)); // 2 s
    assert_eq!(h.step(s(2), Some(false)), None);
    assert_eq!(h.step(s(3), Some(false)), Some(Step::Start));
}
