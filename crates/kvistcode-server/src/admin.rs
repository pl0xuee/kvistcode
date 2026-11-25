//! How `kvistcode-server --pair` asks the running server for a pairing link.
//!
//! The running process owns the pairing codes (they live in its memory), so the command
//! has to ask it. The simplest safe channel is a Unix socket in the data folder: no network
//! port, nothing for a browser or another machine to reach, and only the folder's owner can
//! open it (the folder is `0700`, the socket `0600`). Connect, write one command line
//! (`pair` or `unpair-all`), read one JSON line back, done. `pair` answers
//! `{"url": ..., "expiresAt": ...}`, `unpair-all` answers `{"ok": true}`, and either can
//! answer `{"error": ...}`.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;

use kvistcode_remote::Remote;

#[cfg(unix)]
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("admin.sock")
}

/// What `kvistcode-server` asks the running server for.
#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    /// A fresh pairing link.
    Pair,
    /// Sign out every paired browser and phone.
    UnpairAll,
}

#[cfg(unix)]
impl Command {
    fn word(self) -> &'static str {
        match self {
            Command::Pair => "pair",
            Command::UnpairAll => "unpair-all",
        }
    }
}

/// The reply of one admin request (only ever built on Unix, where the socket exists).
#[cfg_attr(not(unix), allow(dead_code))]
pub struct PairingLink {
    pub url: String,
    pub expires_at: i64,
}

#[cfg(unix)]
pub fn serve(data_dir: &Path, remote: Arc<Remote>) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let path = socket_path(data_dir);
    // The data folder lock is ours, so a socket still there is left over from a crash.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)
        .map_err(|e| anyhow::anyhow!("couldn't create {}: {e}", path.display()))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    Ok(tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let remote = remote.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                let mut reader = BufReader::new(read.take(64));
                let got = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    reader.read_line(&mut line),
                )
                .await;
                if !matches!(got, Ok(Ok(_))) {
                    return;
                }
                let reply = match line.trim() {
                    "pair" => match remote.pairing_link() {
                        Some((url, expires_at)) => {
                            serde_json::json!({ "url": url, "expiresAt": expires_at })
                        }
                        None => serde_json::json!({
                            "error": "The server isn't ready to pair yet (is Tailscale still starting?). Try again in a moment."
                        }),
                    },
                    "unpair-all" => {
                        // Read-only share links too: this is the "I lost a device" switch.
                        for share in remote.status().shares {
                            remote.revoke_share(&share.id);
                        }
                        remote.unpair_all();
                        tracing::info!("every paired device was signed out (--unpair-all)");
                        serde_json::json!({ "ok": true })
                    }
                    _ => serde_json::json!({ "error": "unknown request" }),
                };
                let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
            });
        }
    }))
}

/// Not available off Unix: the server runs, but a new link needs a restart.
#[cfg(not(unix))]
pub fn serve(_: &Path, _: Arc<Remote>) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    anyhow::bail!("`--pair` isn't supported on this system")
}

/// Sends `command` to the server running on `data_dir` and returns its JSON answer.
#[cfg(unix)]
fn ask(data_dir: &Path, command: Command) -> Result<serde_json::Value, String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let path = socket_path(data_dir);
    let mut stream = UnixStream::connect(&path).map_err(|_| {
        format!(
            "no kvistcode-server is running on {}. Start it first (it prints a pairing link).",
            data_dir.display()
        )
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    stream
        .write_all(format!("{}\n", command.word()).as_bytes())
        .map_err(|e| format!("couldn't reach the server: {e}"))?;
    let mut line = String::new();
    stream
        .read_to_string(&mut line)
        .map_err(|e| format!("the server didn't answer: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|_| "the server sent an answer that isn't understood".to_string())?;
    if let Some(e) = v["error"].as_str() {
        return Err(e.to_string());
    }
    Ok(v)
}

/// Asks the server running on `data_dir` for a pairing link.
#[cfg(unix)]
pub fn request(data_dir: &Path) -> Result<PairingLink, String> {
    let v = ask(data_dir, Command::Pair)?;
    match (v["url"].as_str(), v["expiresAt"].as_i64()) {
        (Some(url), Some(expires_at)) => Ok(PairingLink {
            url: url.to_string(),
            expires_at,
        }),
        _ => Err("the server sent an answer that isn't understood".into()),
    }
}

#[cfg(not(unix))]
pub fn request(_: &Path) -> Result<PairingLink, String> {
    Err("`--pair` isn't supported on this system. Restart the server to get a new link.".into())
}

/// Signs out every device paired with the server running on `data_dir`.
#[cfg(unix)]
pub fn unpair_all(data_dir: &Path) -> Result<(), String> {
    ask(data_dir, Command::UnpairAll).map(|_| ())
}

#[cfg(not(unix))]
pub fn unpair_all(_: &Path) -> Result<(), String> {
    Err("`--unpair-all` isn't supported on this system. Stop the server and delete remote.json in its data folder.".into())
}
