# helm-win-apps

Windows app inventory, app icons, launching, and top-level window control for the Helm PC
agent. MIT licensed, clean-room, written from Microsoft Learn documentation.

## API

| Function | What it does |
|---|---|
| `list_apps() -> Result<Vec<AppEntry>>` | Enumerates `shell:AppsFolder` (Start menu "All apps": Win32 shortcuts and packaged apps), deduplicated by id, sorted by name. |
| `app_icon_png(app_id, size_px) -> Result<Vec<u8>>` | App icon as PNG, at most `size_px` square (clamped to 16..=256). Uses `IShellItemImageFactory::GetImage`, then converts premultiplied BGRA to straight RGBA. |
| `IconCache::new(dir)` | On-disk PNG cache keyed by app id + size: `icon_png`, `get`, `put`, `get_or_insert_with`, `path_for`, `clear`. |
| `launch_app(app_id) -> Result<()>` | Packaged: `IApplicationActivationManager::ActivateApplication`, falling back to the shell. Desktop: invokes the AppsFolder item through `ShellExecuteExW` (`SEE_MASK_INVOKEIDLIST`). |
| `open_target(target) -> Result<()>` | Opens a file, folder or URL with its default handler. URLs: `http`, `https`, `mailto` only. Paths: absolute drive-letter paths that exist. UNC and device paths are rejected. |
| `list_windows() -> Result<Vec<WindowInfo>>` | Visible, unowned, non-tool, non-cloaked top-level windows with a title. |
| `focus_window(hwnd) -> Result<FocusResult>` | Restores the window if it is minimised, then tries `SetForegroundWindow`, then `AttachThreadInput` with the foreground thread, then an injected Alt press (`SendInput`). Reports `{ focused, method }` after re-checking `GetForegroundWindow`. |
| `minimize_window` / `maximize_window` / `restore_window(hwnd)` | `ShowWindowAsync`, so a hung app can't block the call. |
| `close_window(hwnd)` | Posts `WM_CLOSE`. The app may still ask to save changes. |

Types (`serde`, snake_case fields):

- `AppEntry { app_id, name, kind: "packaged" | "desktop", target_path }`. `app_id` is the
  item's parsing name in `shell:AppsFolder`: an AUMID for packaged apps, and an explicit
  AUMID or a `{KnownFolderGuid}\path.exe` path for desktop apps. `target_path` is what the
  shell reports for desktop apps. It is usually a file path, but can be a URL such as
  `steam://…`.
- `WindowInfo { hwnd, title, pid, exe_path, app_id, is_minimized, is_foreground }`.
  `app_id` comes from the window's `System.AppUserModel.ID`, then the packaged process's
  AUMID, then a match of the exe path against `list_apps()` targets.
- `FocusResult { focused, method }`. `method` is one of `already_foreground`,
  `set_foreground_window`, `attach_thread_input`, `alt_key_input` or `none`.
- `Error`: `Unsupported`, `InvalidArgument`, `NotFound`, `Windows { context, code, message }`,
  `Io`, `Png` or `Worker`. Bad ids return errors and never panic.

## Safety checks on input

- `launch_app` and `app_icon_png` accept only ids that `list_apps()` returns. The app list is
  cached for 2 minutes and re-enumerated once on a miss.
- The window functions accept only handles that `list_windows()` would return. A caller
  can't post `WM_CLOSE` to hidden or system windows.
- `open_target` on an executable file runs it, just as double-clicking it in Explorer would.
  Any policy about that belongs in the agent.

## Threading

Every function is **blocking** and can be called from any thread. All COM, shell and window
work runs on one internal worker thread. That thread is a COM STA, pumps messages while idle,
and is restarted if it dies. Callers don't need to initialise COM. From tokio, use
`tokio::task::spawn_blocking`. Calls are serialised.

On non-Windows targets every function returns `Error::Unsupported`.

## Development

```
cargo test                                      # unit tests + read-only live tests
cargo test -- --ignored --test-threads=1        # opens/closes a throwaway Notepad and Calculator
cargo run --example demo                        # prints apps and windows, writes one icon
cargo clippy --all-targets
```
