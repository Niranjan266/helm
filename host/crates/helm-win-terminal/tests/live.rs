//! Live ConPTY tests. Safe: they only start cmd.exe, echo text, query the console size and exit.
#![cfg(windows)]

use helm_win_terminal::{KILLED_EXIT_CODE, Shell, Terminal, vt};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Captured = Arc<Mutex<Vec<u8>>>;

fn spawn_cmd(cols: u16, rows: u16, cwd: Option<&std::path::Path>) -> (Terminal, Captured) {
    let out: Captured = Arc::default();
    let sink = out.clone();
    let t = Terminal::spawn(Shell::Cmd, cols, rows, cwd, move |b: &[u8]| sink.lock().unwrap().extend_from_slice(b))
        .expect("spawn cmd.exe in a pseudo console");
    (t, out)
}

fn text(out: &Captured) -> String {
    vt::strip(&out.lock().unwrap())
}

/// Waits until the plain-text output contains `needle`.
fn wait_for(out: &Captured, needle: &str, timeout: Duration) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if text(out).contains(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn cmd_echo_resize_and_exit_code() {
    let (t, out) = spawn_cmd(80, 25, None);
    assert!(t.pid() != 0);
    assert!(t.is_running());

    // `^o` is an escaped `o`: the typed line shows "helm-^ok", only the executed echo prints
    // "helm-ok", so finding it proves the command ran.
    t.write(b"echo helm-^ok\r").unwrap();
    assert!(wait_for(&out, "helm-ok", Duration::from_secs(10)), "no echo output; got: {:?}", text(&out));

    t.resize(100, 30).unwrap();
    assert_eq!(t.size(), (100, 30));
    t.write(b"mode con\r").unwrap();
    assert!(
        wait_for(&out, "Columns:", Duration::from_secs(10)),
        "no `mode con` output; got: {:?}",
        text(&out)
    );
    let s = text(&out);
    let cols_line = s.lines().find(|l| l.contains("Columns:")).unwrap_or_default().to_string();
    assert!(cols_line.contains("100"), "console did not see the resize: {cols_line:?}");

    t.write(b"exit 7\r").unwrap();
    assert_eq!(t.wait(Some(Duration::from_secs(10))), Some(7));
    assert!(!t.is_running());
    assert!(!t.was_killed());
    assert!(t.write(b"echo late\r").is_err(), "writes after exit must fail");
    assert!(t.resize(80, 25).is_err());
}

#[test]
fn working_directory_is_honoured() {
    let dir = std::env::temp_dir();
    let (t, out) = spawn_cmd(120, 25, Some(&dir));
    t.write(b"cd\r").unwrap();
    let want = dir.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && !text(&out).to_lowercase().replace("\r\n", "\n").contains(&want) {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(text(&out).to_lowercase().contains(&want), "cwd {want:?} not in {:?}", text(&out));
    t.write(b"exit\r").unwrap();
    assert_eq!(t.wait(Some(Duration::from_secs(10))), Some(0));
}

#[test]
fn kill_ends_session_and_on_exit_fires() {
    let fired = Arc::new(Mutex::new(None));
    let f2 = fired.clone();
    let t = Terminal::spawn_with_exit(Shell::Cmd, 80, 25, None, |_b: &[u8]| {}, move |c| *f2.lock().unwrap() = Some(c))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    t.kill().unwrap();
    assert_eq!(t.wait(Some(Duration::from_secs(10))), Some(KILLED_EXIT_CODE));
    assert!(t.was_killed());
    assert_eq!(*fired.lock().unwrap(), Some(KILLED_EXIT_CODE));
}

#[test]
fn drop_while_running_does_not_hang() {
    let (t, out) = spawn_cmd(80, 25, None);
    t.write(b"echo helm-^ok\r").unwrap();
    assert!(wait_for(&out, "helm-ok", Duration::from_secs(10)));
    let start = Instant::now();
    drop(t);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn bad_arguments_rejected() {
    assert!(Terminal::spawn(Shell::Cmd, 0, 25, None, |_b: &[u8]| {}).is_err());
    let missing = std::env::temp_dir().join("helm-win-terminal-does-not-exist-9f3a");
    assert!(Terminal::spawn(Shell::Cmd, 80, 25, Some(&missing), |_b: &[u8]| {}).is_err());
    assert!(Terminal::spawn(Shell::Custom("helm-no-such-program-9f3a.exe".into()), 80, 25, None, |_b: &[u8]| {}).is_err());
}

#[test]
fn powershell_runs() {
    let out: Captured = Arc::default();
    let sink = out.clone();
    let t = Terminal::spawn(Shell::PowerShell, 120, 30, None, move |b: &[u8]| sink.lock().unwrap().extend_from_slice(b))
        .unwrap();
    // String concatenation: only the evaluated result contains "helm-ps-ok".
    t.write(b"'helm-ps-' + 'ok'\r").unwrap();
    assert!(wait_for(&out, "helm-ps-ok", Duration::from_secs(30)), "got: {:?}", text(&out));
    t.write(b"exit 3\r").unwrap();
    assert_eq!(t.wait(Some(Duration::from_secs(15))), Some(3));
}
