//! `helm-agent` command line.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, bail};
use helm_agent::devices::DeviceStore;
use helm_agent::{Config, ServerHandle, audit::AuditLog, qr};

const USAGE: &str = "\
helm-agent — Helm PC agent

USAGE:
    helm-agent [OPTIONS]                 run the agent
    helm-agent devices [OPTIONS]         list paired devices and exit
    helm-agent revoke <device_id> [OPTIONS]
                                         remove a paired device and exit
    helm-agent check-update              check GitHub Releases for a newer agent and exit

OPTIONS:
    --data-dir <path>   data folder (default %APPDATA%\\Helm)
    --port <n>          TCP port (default 47820; 0 = any free port)
    --no-mdns           do not advertise on the local network
    --pair              show a pairing QR at start even if devices are paired
    --no-tray           console only: no notification-area icon
    --background        no console window (tray only; logs to agent.log in the data
                        folder; used by Start with Windows and helm-service)
    -h, --help          show this help
    -V, --version       show the version

While running, type  p + Enter  for a fresh pairing QR,  c + Enter  for a 6-digit
pairing code (when the phone camera can't scan),  y / n + Enter  to allow / deny a
waiting phone,  ask on / ask off  to ask before new phones pair,  q + Enter  to quit.
The tray icon menu offers the same (plus Paired devices, Start with Windows and
Check for updates). Updates are only checked and verified, never installed silently.";

enum Command {
    Run,
    Devices,
    Revoke(String),
    CheckUpdate,
}

struct Args {
    command: Command,
    data_dir: PathBuf,
    port: u16,
    mdns: bool,
    pair: bool,
    tray: bool,
    background: bool,
}

fn parse_args() -> anyhow::Result<Option<Args>> {
    let mut args = Args {
        command: Command::Run,
        data_dir: helm_agent::default_data_dir(),
        port: helm_agent::DEFAULT_PORT,
        mdns: true,
        pair: false,
        tray: cfg!(windows),
        background: false,
    };
    let mut positional: Vec<String> = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("helm-agent {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--data-dir" => {
                args.data_dir = PathBuf::from(it.next().context("--data-dir needs a path")?);
            }
            "--port" => {
                let v = it.next().context("--port needs a number")?;
                args.port = v.parse().with_context(|| format!("bad port {v:?}"))?;
            }
            "--no-mdns" => args.mdns = false,
            "--pair" => args.pair = true,
            "--no-tray" => args.tray = false,
            "--background" => args.background = true,
            other if other.starts_with('-') => bail!("unknown option {other}\n\n{USAGE}"),
            other => positional.push(other.to_string()),
        }
    }
    if args.background && !args.tray {
        bail!("--background needs the tray icon (drop --no-tray)");
    }
    args.command = match positional.as_slice() {
        [] => Command::Run,
        [cmd] if cmd == "devices" => Command::Devices,
        [cmd] if cmd == "check-update" => Command::CheckUpdate,
        [cmd, id] if cmd == "revoke" => Command::Revoke(id.clone()),
        [cmd] if cmd == "revoke" => bail!("revoke needs a device_id (see `helm-agent devices`)"),
        _ => bail!("unexpected arguments {positional:?}\n\n{USAGE}"),
    };
    Ok(Some(args))
}

fn format_time(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(ms as i128 * 1_000_000)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| ms.to_string())
}

fn list_devices(args: &Args) -> anyhow::Result<()> {
    let devices = DeviceStore::new(&args.data_dir).list()?;
    if devices.is_empty() {
        println!("No paired devices (data dir {}).", args.data_dir.display());
        return Ok(());
    }
    println!("Paired devices ({}):", args.data_dir.display());
    for d in devices {
        println!(
            "  {}  {:<24} tier={:<6} paired={} last_seen={}",
            d.device_id,
            d.device_name,
            d.tier.as_str(),
            format_time(d.paired_at_ms),
            format_time(d.last_seen_ms)
        );
    }
    Ok(())
}

fn revoke_device(args: &Args, device_id: &str) -> anyhow::Result<()> {
    let store = DeviceStore::new(&args.data_dir);
    if store.revoke(device_id)? {
        AuditLog::new(&args.data_dir).record("revoke", Some(device_id), Some("cli"), "by=cli");
        println!("Revoked {device_id}. It must pair again to connect.");
        Ok(())
    } else {
        bail!("no paired device with id {device_id}")
    }
}

/// Writes a fresh pairing QR and returns the path of `pair-qr.svg`.
fn show_pairing(server: &ServerHandle) -> std::path::PathBuf {
    let pairing = server.new_pairing();
    let svg_path = server.data_dir().join("pair-qr.svg");
    match qr::to_unicode(&pairing.uri) {
        Ok(text) => println!("\n{text}"),
        Err(e) => eprintln!("could not draw QR: {e:#}"),
    }
    match qr::to_svg(&pairing.uri).and_then(|svg| {
        helm_agent::util::atomic_write(&svg_path, svg.as_bytes())?;
        Ok(())
    }) {
        Ok(()) => println!("QR image:     {}", svg_path.display()),
        Err(e) => eprintln!("could not write {}: {e:#}", svg_path.display()),
    }
    let minutes = pairing
        .expires_at
        .saturating_duration_since(Instant::now())
        .as_secs()
        .div_ceil(60);
    println!("Pairing link: {}", pairing.uri);
    println!(
        "Scan it with the Helm app. Valid for {minutes} min, one use. Type p + Enter for a new one.\n"
    );
    svg_path
}

/// Show a 6-digit pairing code (g4) and return the text shown, or why there is none.
fn show_code(server: &ServerHandle) -> String {
    match server.new_code_pairing() {
        Ok(c) => {
            let text = format!(
                "Pairing code for {}:  {}\nOn the phone: Nearby PCs -> {} -> type the code. \
                 Valid 5 min, one try.",
                c.host_name,
                helm_agent::g4::code::display_code(&c.code),
                c.host_name
            );
            println!("\n{text}\n");
            text
        }
        Err(left) => {
            let text = format!(
                "Code pairing is locked after wrong codes; try again in {} min (or use the QR).",
                left.as_secs().div_ceil(60)
            );
            println!("{text}");
            text
        }
    }
}

/// Console answer to a waiting pairing request (oldest first).
fn answer_pairing(server: &ServerHandle, allow: bool) {
    let g4 = &server.state().g4;
    match g4.pending().first() {
        Some(p) => {
            g4.resolve(p.request_id, allow);
            println!(
                "{} \"{}\".",
                if allow { "Allowed" } else { "Denied" },
                p.device_name
            );
        }
        None => println!("No phone is waiting for approval."),
    }
}

/// Print host events (g4) that need the console user's attention.
fn print_host_event(ev: &helm_agent::g4::HostEvent) {
    use helm_agent::g4::HostEvent;
    match ev {
        HostEvent::PairingRequest(p) => println!(
            "\nPhone \"{}\" ({}) wants to pair. Type y + Enter to allow, n + Enter to deny \
             ({} s).",
            p.device_name,
            p.peer,
            ((p.expires_unix_ms - helm_agent::util::now_ms()).max(0) / 1000)
        ),
        HostEvent::PairingResolved { outcome, .. } => {
            println!("Pairing request: {}", outcome.as_str())
        }
        HostEvent::PairingFailed { method, reason, .. } => {
            println!("Pairing ({method}) failed: {reason}")
        }
        HostEvent::PairingLocked { .. } => {
            println!("Code pairing locked for 10 minutes after repeated wrong codes.")
        }
        HostEvent::Paired {
            device_name,
            method,
            ..
        } => println!("Paired \"{device_name}\" ({method})."),
        _ => {}
    }
}

/// Open a file or folder with its default app (browser for the QR SVG, Explorer for folders).
async fn open_with_shell(path: std::path::PathBuf) {
    let shown = path.display().to_string();
    let r =
        tokio::task::spawn_blocking(move || helm_win_apps::open_target(&path.to_string_lossy()))
            .await;
    if !matches!(r, Ok(Ok(()))) {
        eprintln!("could not open {shown}");
    }
}

#[cfg(windows)]
type TrayHandle = helm_agent::tray::Tray;
#[cfg(not(windows))]
type TrayHandle = ();
#[cfg(not(windows))]
enum NoTrayCommand {}

#[cfg(windows)]
async fn start_tray(
    tx: tokio::sync::mpsc::UnboundedSender<helm_agent::tray::TrayCommand>,
    background: bool,
) -> Option<TrayHandle> {
    // Right after sign-in (Start with Windows / helm-service) the taskbar may not exist yet:
    // in background mode keep trying for about a minute.
    let attempts = if background { 30 } else { 1 };
    for attempt in 1..=attempts {
        match helm_agent::tray::Tray::start(tx.clone()) {
            Ok(t) => return Some(t),
            Err(e) if attempt == attempts => {
                eprintln!("tray icon unavailable ({e:#}); console mode only");
                tracing::warn!("tray icon unavailable: {e:#}");
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
        }
    }
    None
}

/// Tray "Check for updates": check, and offer to download + verify (never install).
#[cfg(windows)]
async fn check_updates_interactive() {
    use helm_agent::update::{self, UpdateStatus};
    let status = tokio::task::spawn_blocking(update::check)
        .await
        .unwrap_or_else(|e| UpdateStatus::Failed(e.to_string()));
    println!("{}", status.summary());
    let _ = tokio::task::spawn_blocking(move || {
        let title = "Helm updates";
        let UpdateStatus::Available(c) = &status else {
            helm_agent::desktop::message_box(title, &status.summary(), false);
            return;
        };
        let ask = format!(
            "{}\n\nDownload the installer and verify its signature now? \
             It will not be run automatically.",
            status.summary()
        );
        if !helm_agent::desktop::message_box(title, &ask, true) {
            return;
        }
        match update::download(c) {
            Ok(path) => {
                let folder = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                helm_agent::desktop::message_box(
                    title,
                    &format!(
                        "Verified installer saved to\n{}\n\nRun it to update Helm.",
                        path.display()
                    ),
                    false,
                );
                let _ = helm_win_apps::open_target(&folder.to_string_lossy());
            }
            Err(e) => {
                helm_agent::desktop::message_box(title, &format!("Download failed: {e}"), false);
            }
        }
    })
    .await;
}

/// Startup check: log only (no dialogs at sign-in). Skipped when this build has no key.
fn startup_update_check() {
    if helm_update::compiled_public_key().is_none() {
        tracing::info!("update checks disabled: no release key compiled in");
        return;
    }
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        if let Ok(status) = tokio::task::spawn_blocking(helm_agent::update::check).await {
            println!("{}", status.summary());
            tracing::info!("{}", status.summary());
        }
    });
}

#[cfg(windows)]
fn toggle_autostart() {
    let want = !helm_agent::tray::autostart_enabled();
    match helm_agent::tray::set_autostart(want) {
        Ok(()) => println!("Start with Windows: {}", if want { "on" } else { "off" }),
        Err(e) => eprintln!("could not change Start with Windows: {e:#}"),
    }
}

async fn run(args: Args) -> anyhow::Result<()> {
    // One agent per data folder and session (service-started + Start with Windows).
    let Some(_instance) = helm_agent::desktop::single_instance(&args.data_dir) else {
        println!(
            "Helm agent is already running for {}.",
            args.data_dir.display()
        );
        tracing::info!("another agent already runs for this data folder; exiting");
        return Ok(());
    };
    let mut config = Config::new(&args.data_dir);
    config.port = args.port;
    config.enable_mdns = args.mdns;
    let server = helm_agent::start(config).await?;

    println!();
    println!("Helm agent {} is running.", env!("CARGO_PKG_VERSION"));
    println!("  PC name:      {}", server.host_name());
    println!("  Host id:      {}", server.host_id());
    println!("  Port:         {}", server.port());
    let addrs: Vec<String> = server.addresses().iter().map(|a| a.to_string()).collect();
    println!(
        "  Addresses:    {}",
        if addrs.is_empty() {
            "(none found)".to_string()
        } else {
            addrs.join(", ")
        }
    );
    println!("  Fingerprint:  {}", server.fingerprint());
    println!("  Data folder:  {}", server.data_dir().display());
    println!(
        "  Commands:     p + Enter = new pairing QR, c + Enter = pairing code, q + Enter = quit"
    );
    if server.state().g4.settings().require_approval {
        println!("  New phones:   must be allowed on this PC (ask off + Enter to stop asking)");
    }

    let paired = server.devices().map(|d| d.len()).unwrap_or(0);
    if paired == 0 || args.pair {
        if paired == 0 {
            println!("\nNo devices paired yet.");
        }
        let svg = show_pairing(&server);
        if args.background {
            open_with_shell(svg).await;
        }
    } else {
        println!("\n{paired} paired device(s). Type p + Enter to pair another phone.\n");
    }

    // Console commands come from a blocking reader thread.
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line_tx.send(line.trim().to_ascii_lowercase()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    #[cfg(windows)]
    let (tray_tx, mut tray_rx) = tokio::sync::mpsc::unbounded_channel();
    #[cfg(not(windows))]
    let (_tray_tx, mut tray_rx) = tokio::sync::mpsc::unbounded_channel::<NoTrayCommand>();
    #[cfg(windows)]
    let tray: Option<TrayHandle> = if args.tray {
        start_tray(tray_tx, args.background).await
    } else {
        None
    };
    #[cfg(not(windows))]
    let tray: Option<TrayHandle> = None;
    if args.background && tray.is_none() {
        bail!("--background needs the tray icon, which could not be created");
    }

    startup_update_check();

    let mut host_events = server.state().g4.subscribe();
    let mut stdin_open = true;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            line = line_rx.recv(), if stdin_open => match line.as_deref() {
                Some("p") | Some("pair") => {
                    show_pairing(&server);
                }
                Some("c") | Some("code") => {
                    show_code(&server);
                }
                Some("y") | Some("allow") => answer_pairing(&server, true),
                Some("n") | Some("deny") => answer_pairing(&server, false),
                Some(cmd @ ("ask on" | "ask off")) => {
                    match server.state().g4.set_require_approval(cmd == "ask on") {
                        Ok(s) => println!("Ask before a new phone pairs: {}", if s.require_approval { "on" } else { "off" }),
                        Err(e) => eprintln!("could not save the setting: {e:#}"),
                    }
                }
                Some("q") | Some("quit") | Some("exit") => break,
                Some("") => {}
                Some(other) => println!("unknown command {other:?} (p = pair, c = code, q = quit)"),
                // stdin closed (no console): keep serving until Ctrl+C or tray Quit.
                None => stdin_open = false,
            },
            ev = host_events.recv() => match ev {
                Ok(ev) => print_host_event(&ev),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
            },
            Some(cmd) = tray_rx.recv() => {
                #[cfg(windows)]
                {
                    use helm_agent::tray::TrayCommand;
                    match cmd {
                        TrayCommand::ShowPairing => {
                            let svg = show_pairing(&server);
                            open_with_shell(svg).await;
                        }
                        TrayCommand::ShowPairingCode => {
                            let text = show_code(&server);
                            tokio::task::spawn_blocking(move || {
                                helm_agent::desktop::message_box("Helm pairing code", &text, false)
                            });
                        }
                        TrayCommand::PairedDevices => {
                            open_with_shell(server.data_dir().to_path_buf()).await;
                        }
                        TrayCommand::ToggleAutostart => toggle_autostart(),
                        TrayCommand::CheckUpdates => {
                            tokio::spawn(check_updates_interactive());
                        }
                        TrayCommand::Quit => break,
                    }
                }
                #[cfg(not(windows))]
                let _ = cmd;
            }
        }
    }
    drop(tray);

    println!("Stopping…");
    let svg_path = server.data_dir().join("pair-qr.svg");
    server.shutdown().await;
    // The QR holds a pairing token; don't leave it lying around.
    let _ = std::fs::remove_file(svg_path);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    // Physical pixels everywhere (capture, cursor, absolute input); see helm-win-capture README.
    helm_agent::screen::make_process_dpi_aware();
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    match &args.command {
        Command::Devices => return list_devices(&args),
        Command::Revoke(id) => return revoke_device(&args, id),
        Command::CheckUpdate => {
            println!("{}", helm_agent::update::check().summary());
            return Ok(());
        }
        Command::Run => {}
    }
    if helm_agent::desktop::current_session_id() == Some(0) {
        // Session 0 has no desktop: input, capture and the tray cannot work there.
        bail!(
            "helm-agent is running in session 0 (the services session). Install it with \
             helm-service, which starts the agent in the signed-in user's session."
        );
    }
    #[cfg(windows)]
    if args.background {
        // Started by "Start with Windows": no console window, the tray is the UI.
        // SAFETY: detaching from our own console has no preconditions.
        unsafe {
            let _ = windows::Win32::System::Console::FreeConsole();
        }
    }
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
    };
    // No console in background mode: log to <data dir>\agent.log instead.
    match args
        .background
        .then(|| helm_agent::desktop::open_log(&args.data_dir).ok())
        .flatten()
    {
        Some((file, _)) => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_target(false)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        None => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_target(false)
            .init(),
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}
