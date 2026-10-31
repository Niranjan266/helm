# helm-win-terminal

Interactive terminal sessions for the Helm PC agent (Windows 10 1809+ / Windows 11, MSVC), built on
the Windows pseudo console (ConPTY): `CreatePseudoConsole` + two anonymous pipes + `CreateProcessW`
with `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`. The shell runs inside a kill-on-close Job object, so
`kill()` / drop end the shell and everything it started.

Only Win32 APIs through the `windows` crate (0.62). All code is original, written from the
Microsoft Learn documentation (clean-room rule). On non-Windows targets the crate builds; `spawn`
returns `Error::Unsupported`.

## Quick start

```rust
use helm_win_terminal::{Shell, Terminal};

let term = Terminal::spawn_with_exit(
    Shell::PowerShell, 120, 30, None,
    |vt: &[u8]| { /* raw VT bytes -> phone terminal emulator */ },
    |code| { /* shell ended, all output already delivered */ },
)?;
term.write(b"dir\r")?;        // keys: "\r" Enter, "\x1b[A" Up, "\x03" Ctrl+C
term.resize(100, 40)?;
term.kill()?;                  // or just drop it
let code = term.wait(None);
```

## API

| Item | Purpose |
|---|---|
| `Terminal::spawn(shell, cols, rows, cwd, on_output)` | start a session; `on_output(&[u8])` gets raw VT/UTF-8 bytes (split anywhere) |
| `Terminal::spawn_with_exit(..., on_exit)` | plus `on_exit(code)` once the shell ended and output was drained |
| `write(bytes)` / `resize(cols, rows)` / `size()` | input, `ResizePseudoConsole` |
| `wait(timeout)` / `try_wait()` / `exit_code()` / `is_running()` | exit code after shell exit **and** output drain |
| `kill()` / `was_killed()` | `TerminateJobObject` (exit code `KILLED_EXIT_CODE` = 1) |
| `pid()` | shell process id |
| `Shell` | `Cmd`, `PowerShell` (5.1), `Pwsh` (7, via PATH), `Custom(cmdline)`; system shells use absolute `%SystemRoot%` paths |
| `vt::strip` | remove escape sequences (tests, logs, search) |

## Threading

* `helm-pty-out` reads the output pipe and calls `on_output`. It never stops reading (a full pipe
  would block the console). Keep the callback non-blocking (e.g. `try_send` into a bounded channel).
* `helm-pty-wait` waits for the shell, closes the pseudo console (flushes + breaks the pipe), joins
  the reader, records the exit code, notifies `wait()` and calls `on_exit`.
* `Terminal` is `Send + Sync`. Drop kills the job and joins the worker threads (except when dropped
  from one of its own callbacks).

## Agent usage

One `Terminal` per phone terminal tab. Stream `on_output` bytes over the session channel, forward
keystrokes with `write`, phone viewport changes with `resize`, and on disconnect drop it. Only allow
it for paired, authorised devices (it is a full shell as the logged-in user).

## Tests

```text
cargo test                     # pure (shell lines, sizes, VT strip) + live (cmd/PowerShell)
cargo clippy --all-targets
```

Live tests are side-effect free: spawn cmd.exe, `echo helm-^ok` (only the executed echo prints
`helm-ok`), resize to 100x30 and check `mode con`, `exit 7` -> exit code 7, cwd honoured, kill fires
`on_exit`, drop does not hang, bad arguments rejected, PowerShell evaluates an expression.

## Notes

* Child std handles: `STARTF_USESTDHANDLES` with null handles so a parent with redirected stdio
  (service, CI) does not leak its handles into the shell.
* Output that the shell produces in the last instant before exit is flushed by
  `ClosePseudoConsole`; `wait()` returns only after it was delivered.
