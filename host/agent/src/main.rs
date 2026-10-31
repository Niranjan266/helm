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

OPTIONS:
    --data-dir <path>   data folder (default %APPDATA%\\Helm)
    --port <n>          TCP port (default 47820; 0 = any free port)
    --no-mdns           do not advertise on the local network
    --pair              show a pairing QR at start even if devices are paired
    --no-tray           console only: no notification-area icon
    --background        no console window (tray only; used by Start with Windows)
    -h, --help          show this help
    -V, --version       show the version

While running, type  p + Enter  for a fresh pairing QR,  q + Enter  to quit.
The tray icon menu offers the same (plus Paired devices and Start with Windows).";

enum Command {
    Run,
    Devices,
    Revoke(String),
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
fn start_tray(
    tx: tokio::sync::mpsc::UnboundedSender<helm_agent::tray::TrayCommand>,
) -> Option<TrayHandle> {
    match helm_agent::tray::Tray::start(tx) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("tray icon unavailable ({e:#}); console mode only");
            None
        }
    }
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
    println!("  Commands:     p + Enter = new pairing QR, q + Enter = quit, Ctrl+C = quit");

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
    let tray: Option<TrayHandle> = if args.tray { start_tray(tray_tx) } else { None };
    #[cfg(not(windows))]
    let tray: Option<TrayHandle> = None;
    if args.background && tray.is_none() {
        bail!("--background needs the tray icon, which could not be created");
    }

    let mut stdin_open = true;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            line = line_rx.recv(), if stdin_open => match line.as_deref() {
                Some("p") | Some("pair") => {
                    show_pairing(&server);
                }
                Some("q") | Some("quit") | Some("exit") => break,
                Some("") => {}
                Some(other) => println!("unknown command {other:?} (p = pair, q = quit)"),
                // stdin closed (no console): keep serving until Ctrl+C or tray Quit.
                None => stdin_open = false,
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
                        TrayCommand::PairedDevices => {
                            open_with_shell(server.data_dir().to_path_buf()).await;
                        }
                        TrayCommand::ToggleAutostart => toggle_autostart(),
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
        Command::Run => {}
    }
    #[cfg(windows)]
    if args.background {
        // Started by "Start with Windows": no console window, the tray is the UI.
        // SAFETY: detaching from our own console has no preconditions.
        unsafe {
            let _ = windows::Win32::System::Console::FreeConsole();
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}
