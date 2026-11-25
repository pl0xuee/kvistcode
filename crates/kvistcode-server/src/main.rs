//! `kvistcode-server`: Kvistcode without a window. Boots the same core the desktop app uses
//! (agents, queue, schedules, triggers), serves the phone web app from `kvistcode-remote`,
//! and is paired and controlled from any browser. No tray, hotkeys, notifications, voice or
//! preview panes: those belong to the desktop shell.

mod admin;
mod args;
mod banner;
mod lock;
mod maintenance;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use kvistcode_core::{Core, EventSink, KvistcodeEvent};
use kvistcode_remote::Remote;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tracing::{info, warn};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Total time allowed for a clean shutdown before the process exits anyway.
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(20);

fn main() {
    let args = match args::parse(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("kvistcode-server: {e}");
            std::process::exit(2);
        }
    };
    if args.help {
        print!("{}", args::HELP);
        return;
    }
    if args.version {
        println!("kvistcode-server {VERSION}");
        return;
    }
    let data_dir = match args.data_dir.clone().or_else(default_data_dir) {
        Some(d) => d,
        None => fail("couldn't find a data folder; pass --data-dir"),
    };

    if args.pair {
        match admin::request(&data_dir) {
            Ok(link) => print!(
                "{}",
                banner::pairing(&link.url, link.expires_at, args.data_dir.as_deref())
            ),
            Err(e) => fail(&e),
        }
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        fail(&format!("couldn't create {}: {e}", data_dir.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Holds the database, secrets, pairing tokens and the admin socket.
        let _ = std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700));
    }
    let _lock = match lock::DataDirLock::take(&data_dir) {
        Ok(l) => l,
        Err(e) => fail(&e),
    };

    // PATH is process-global and changing it is only sound before any other thread exists
    // (the runtime and the log writer start some), so it comes first, as in the desktop app.
    // (--no-tool-updates skips the login shell probe: it runs the user's shell profile.)
    let path_cached =
        !args.no_tool_updates && kvistcode_core::providers::augment_path_cached(Some(&data_dir));
    kvistcode_core::toolchain::add_managed_paths(&data_dir);
    if path_cached {
        kvistcode_core::providers::spawn_login_path_refresh(&data_dir);
    }

    let _log_guard = init_logging(&data_dir.join("logs"));
    info!(version = VERSION, data_dir = %data_dir.display(), "starting kvistcode-server");

    // No OS keychain on most servers: keep secrets in a private file in the data folder.
    // (`KVISTCODE_SECRETS=memory` still wins; that is for tests.)
    if std::env::var_os("KVISTCODE_SECRETS").is_none_or(|v| v != "memory") {
        if let Err(e) = kvistcode_core::use_file_secrets(data_dir.join("secrets.json")) {
            fail(&format!("{e:#}"));
        }
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => fail(&format!("couldn't start the async runtime: {e}")),
    };
    let code = match runtime.block_on(run(args, data_dir)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("kvistcode-server: {e:#}");
            1
        }
    };
    // Agent processes are already stopped; don't wait for stray runtime tasks.
    runtime.shutdown_timeout(Duration::from_secs(2));
    std::process::exit(code);
}

fn fail(msg: &str) -> ! {
    eprintln!("kvistcode-server: {msg}");
    std::process::exit(1);
}

/// `kvistcode-server` in the user's data folder: never the desktop app's own folder, so the
/// two never share a database.
fn default_data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("kvistcode-server"))
}

struct ChannelSink(UnboundedSender<KvistcodeEvent>);

impl EventSink for ChannelSink {
    fn emit(&self, event: KvistcodeEvent) {
        // Only fails once the pump is gone (shutting down).
        let _ = self.0.send(event);
    }
}

async fn run(args: args::Args, data_dir: PathBuf) -> anyhow::Result<()> {
    let handle = tokio::runtime::Handle::current();
    let (tx, mut rx) = unbounded_channel::<KvistcodeEvent>();
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));
    let core = Core::open(data_dir.clone(), sink)?;
    let remote = Remote::new(
        core.clone(),
        VERSION.to_string(),
        data_dir.clone(),
        handle.clone(),
    );
    remote.set_headless(true);

    // Core events to connected browsers, merged like the desktop does for its window.
    let pump = {
        let remote = remote.clone();
        tokio::spawn(async move {
            while let Some(first) = rx.recv().await {
                let mut batch = vec![first];
                while batch.len() < 512 {
                    match rx.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                let streaming = batch.iter().any(|e| {
                    matches!(
                        e,
                        KvistcodeEvent::TextDelta { .. } | KvistcodeEvent::TerminalOutput { .. }
                    )
                });
                for event in kvistcode_remote::coalesce::coalesce(batch) {
                    remote.publish(&event);
                }
                if streaming {
                    tokio::time::sleep(Duration::from_millis(16)).await;
                }
            }
        })
    };

    kvistcode_remote::triggers::start(&core, &handle);

    // The flags decide how the server is reached on every start; the port falls back to the
    // saved one.
    let settings = core.update_settings(|s| {
        s.remote.enabled = true;
        s.remote.lan = args.lan;
        s.remote.tailscale = args.tailscale;
        if let Some(port) = args.port {
            s.remote.port = port;
        }
    })?;
    remote.apply(&settings.remote).await;
    if let Some(error) = remote.status().error {
        remote.shutdown().await;
        anyhow::bail!("{error}");
    }

    let mut background = vec![pump];
    {
        let core = core.clone();
        background.push(tokio::spawn(async move { core.startup().await }));
    }
    if args.no_tool_updates {
        info!("--no-tool-updates: not installing tools or checking sign-ins");
    } else {
        let core = core.clone();
        background.push(tokio::spawn(async move {
            for account in core.accounts().unwrap_or_default() {
                let core = core.clone();
                tokio::spawn(async move {
                    if let Err(e) = core.check_account(&account.id).await {
                        warn!(account = %account.id, "account check failed: {e:#}");
                    }
                });
            }
        }));
    }
    if !args.no_tool_updates {
        background.push(maintenance::spawn(core.clone(), data_dir.clone()));
        let (core, dir) = (core.clone(), data_dir.clone());
        background.push(tokio::spawn(async move {
            maintenance::refresh(&core, &dir).await
        }));
    }
    {
        // Follow network changes (new LAN address, Tailscale coming up).
        let remote = remote.clone();
        background.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                remote.refresh().await;
            }
        }));
    }
    match admin::serve(&data_dir, remote.clone()) {
        Ok(task) => background.push(task),
        Err(e) => warn!("`kvistcode-server --pair` won't work: {e:#}"),
    }

    print_startup(&remote, &args, &settings.remote.port).await;

    wait_for_stop().await;
    info!("stopping");
    println!("Stopping: ending agents and closing the server.");
    let stopped = tokio::time::timeout(SHUTDOWN_DEADLINE, async {
        for task in &background {
            task.abort();
        }
        #[cfg(unix)]
        let _ = std::fs::remove_file(admin::socket_path(&data_dir));
        remote.shutdown().await;
        info!("shutting down agents");
        if tokio::time::timeout(Duration::from_secs(10), core.shutdown())
            .await
            .is_err()
        {
            warn!("core shutdown timed out");
        }
    })
    .await;
    if stopped.is_err() {
        warn!("shutdown took too long; exiting anyway");
    }
    // Anything still alive (a stuck handshake): end its whole process tree.
    let _ = tokio::task::spawn_blocking(kvistcode_core::kill_all_children).await;
    Ok(())
}

async fn print_startup(remote: &Arc<Remote>, args: &args::Args, port: &u16) {
    // Over Tailscale the link only exists once Tailscale Serve is up.
    let mut link = remote.pairing_link();
    if link.is_none() && args.tailscale {
        println!("Waiting for Tailscale Serve to come up...");
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            remote.refresh().await;
            link = remote.pairing_link();
            if link.is_some() {
                break;
            }
        }
    }
    let status = remote.status();
    println!("Kvistcode server {VERSION} is running.");
    match status.url.as_deref() {
        Some(url) => println!("Address: {url}"),
        None => println!("Address: not available yet (port {port})"),
    }
    if let Some(ts) = status.tailscale.as_ref().filter(|t| t.url.is_none()) {
        if let Some(message) = ts.message.as_deref().filter(|m| !m.is_empty()) {
            println!("Tailscale: {message}");
        }
        if let Some(command) = &ts.command {
            println!("  Try: {command}");
        }
    }
    println!();
    match link {
        Some((url, expires_at)) => print!(
            "{}",
            banner::pairing(&url, expires_at, args.data_dir.as_deref())
        ),
        None => {
            println!("No pairing link yet. Run `kvistcode-server --pair` in a moment to get one.")
        }
    }
    println!("\nPress Ctrl+C to stop.");
}

/// Returns on SIGINT, SIGTERM or SIGHUP (Ctrl+C elsewhere).
async fn wait_for_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
            signal(SignalKind::hangup()),
        ) else {
            warn!("could not install signal handlers");
            std::future::pending::<()>().await;
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
            _ = hup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

/// Logs to stderr and to a private, size-capped file in `<data>/logs`. Agent output stays out
/// of the default level (see the desktop app's note on `KVISTCODE_LOG`).
fn init_logging(log_dir: &Path) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    let _ = std::fs::create_dir_all(log_dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(log_dir, std::fs::Permissions::from_mode(0o700));
    }
    let (file_writer, guard) =
        match kvistcode_remote::log_writer::CappedLogWriter::new(log_dir.to_path_buf()) {
            Ok(file) => tracing_appender::non_blocking(file),
            Err(e) => {
                eprintln!("could not open the log file: {e}");
                tracing_appender::non_blocking(std::io::sink())
            }
        };
    let filter = EnvFilter::try_from_env("KVISTCODE_LOG").unwrap_or_else(|_| {
        EnvFilter::new(
            "info,kvistcode_core=debug,kvistcode_remote=debug,kvistcode::agent_stderr=warn,kvistcode::rpc=warn",
        )
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(fmt::layer().with_ansi(false).with_writer(file_writer))
        .try_init();
    Some(guard)
}
