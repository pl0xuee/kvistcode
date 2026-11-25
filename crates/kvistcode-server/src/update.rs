//! Self-update from GitHub Releases.
//!
//! Each release carries `kvistcode-server-<version>-linux-<arch>.tar.gz` and a `.sig` next to
//! it, signed in CI with the same key as the desktop app's updates. The server asks GitHub for
//! the latest release, downloads the tarball for its own architecture, refuses it unless the
//! signature checks out against the public key built into this binary, checks that the new
//! binary runs and reports the expected version, and then renames it over its own file. The
//! running process keeps going until no agent is working, then restarts itself in place
//! (same process id, so systemd and friends don't notice).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use kvistcode_core::{AgentStatus, Core};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::VERSION;

const REPO: &str = "pl0xuee/kvistcode";
/// The desktop updater's public key (`plugins.updater.pubkey` in tauri.conf.json); a test
/// keeps the two in step.
pub const PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEJGQzk1MUQ4N0I0RUUyNDQKUldSRTRrNTcyRkhKdjlaM0VPM2NmWDdVQTErdktGOE4rQkx5RE1rNTNQdjRaRWVyNTRQLzdvTW0K";
const MAX_TARBALL: u64 = 256 * 1024 * 1024;
const POLL_BUSY: Duration = Duration::from_secs(5);

/// A newer release this machine can install.
#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    tarball: String,
    signature: String,
}

/// The download for this machine in release `version`, if there is one.
pub fn asset_name(version: &str) -> Option<String> {
    let arch = match std::env::consts::ARCH {
        a @ ("x86_64" | "aarch64") => a,
        _ => return None,
    };
    (std::env::consts::OS == "linux")
        .then(|| format!("kvistcode-server-{version}-linux-{arch}.tar.gz"))
}

fn parse_version(v: &str) -> Option<(u64, u64, u64, bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, _)) => (c, true),
        None => (v, false),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
        pre,
    ))
}

/// `candidate` > `current`, a prerelease counting as older than its release.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some((a, b, c, pa)), Some((x, y, z, px))) => {
            (a, b, c) > (x, y, z) || ((a, b, c) == (x, y, z) && px && !pa)
        }
        _ => false,
    }
}

fn http() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .https_only(true)
            .tls_config(kvistcode_core::toolchain::platform_tls())
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(Duration::from_secs(10 * 60)))
            .user_agent(concat!("kvistcode-server/", env!("CARGO_PKG_VERSION")))
            .build(),
    )
}

/// The latest release if it is newer than this one. `Err` is a message for the person.
pub fn latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = http()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .and_then(|r| {
            r.into_body()
                .with_config()
                .limit(4 * 1024 * 1024)
                .read_to_string()
        })
        .map_err(|e| format!("couldn't reach GitHub to look for updates: {e}"))?;
    let rel: Value = serde_json::from_str(&body)
        .map_err(|_| "GitHub sent an answer that isn't understood".to_string())?;
    let version = rel["tag_name"]
        .as_str()
        .unwrap_or_default()
        .trim_start_matches('v')
        .to_string();
    if !is_newer(&version, VERSION) {
        return Ok(None);
    }
    let Some(name) = asset_name(&version) else {
        return Err(format!(
            "Kvistcode {version} is out, but there is no ready-made download for this system. Build it from source (docs/HEADLESS.md)."
        ));
    };
    let url_of = |n: &str| {
        rel["assets"].as_array().and_then(|a| {
            a.iter()
                .find(|x| x["name"] == n)
                .and_then(|x| x["browser_download_url"].as_str())
                .map(str::to_string)
        })
    };
    let (Some(tarball), Some(signature)) = (url_of(&name), url_of(&format!("{name}.sig"))) else {
        return Err(format!(
            "Kvistcode {version} is out, but its download for this machine isn't there yet. Try again later."
        ));
    };
    Ok(Some(Release {
        page: rel["html_url"].as_str().unwrap_or_default().to_string(),
        notes: rel["body"].as_str().unwrap_or_default().to_string(),
        version,
        tarball,
        signature,
    }))
}

/// Checks `data` against a `.sig` file as written by `tauri signer sign` (a base64-wrapped
/// minisign signature).
pub fn verify(data: &[u8], sig_file: &str) -> Result<(), String> {
    verify_with(PUBKEY, data, sig_file)
}

fn verify_with(pubkey: &str, data: &[u8], sig_file: &str) -> Result<(), String> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let text = |s: &str| {
        b64.decode(s.trim())
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    };
    let key = text(pubkey)
        .and_then(|k| minisign_verify::PublicKey::decode(&k).ok())
        .ok_or("the built-in update key is unreadable")?;
    let sig = text(sig_file)
        .and_then(|s| minisign_verify::Signature::decode(&s).ok())
        .ok_or("the update's signature is unreadable")?;
    key.verify(data, &sig, true)
        .map_err(|_| "the update's signature doesn't match; it was not installed".to_string())
}

fn download(rel: &Release) -> Result<Vec<u8>, String> {
    let agent = http();
    let get = |url: &str, limit: u64| {
        agent
            .get(url)
            .call()
            .and_then(|r| r.into_body().with_config().limit(limit).read_to_vec())
            .map_err(|e| format!("couldn't download the update: {e}"))
    };
    let sig = String::from_utf8(get(&rel.signature, 16 * 1024)?)
        .map_err(|_| "the update's signature is unreadable".to_string())?;
    let data = get(&rel.tarball, MAX_TARBALL)?;
    verify(&data, &sig)?;
    Ok(data)
}

/// The binary this process runs from (what an update replaces).
pub fn current_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // Linux names a replaced file "<path> (deleted)".
    let s = exe.to_string_lossy();
    Some(match s.strip_suffix(" (deleted)") {
        Some(p) => PathBuf::from(p),
        None => exe,
    })
}

/// Whether this user may replace `exe`; `Err` says what to do instead.
pub fn can_replace(exe: &Path) -> Result<(), String> {
    let dir = exe.parent().ok_or("unknown install folder")?;
    let probe = dir.join(format!(".kvistcode-server.probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(_) => Err(format!(
            "Kvistcode can't update itself because this user can't change {}. Reinstall with the one-line installer (docs/HEADLESS.md), which puts it in ~/.local/bin; updates then install by themselves.",
            dir.display()
        )),
    }
}

/// Unpacks the binary from `tarball`, checks it runs as `version`, and renames it over `exe`.
pub fn install(exe: &Path, tarball: &[u8], version: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let staged = exe.with_file_name(".kvistcode-server.new");
    let _ = std::fs::remove_file(&staged);
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tarball));
    let mut found = false;
    for entry in archive
        .entries()
        .map_err(|_| "the update is damaged".to_string())?
    {
        let mut entry = entry.map_err(|_| "the update is damaged".to_string())?;
        let path = entry
            .path()
            .map_err(|_| "the update is damaged".to_string())?;
        let mut parts = path.components();
        let (Some(_), Some(file), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if file.as_os_str() != "kvistcode-server" || !entry.header().entry_type().is_file() {
            continue;
        }
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|_| "the update is damaged".to_string())?;
        std::fs::write(&staged, &bytes)
            .and_then(|_| std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)))
            .map_err(|e| format!("couldn't write the update next to {}: {e}", exe.display()))?;
        found = true;
        break;
    }
    if !found {
        return Err("the update doesn't contain kvistcode-server".into());
    }
    // ETXTBSY: another thread forking (an agent starting) can briefly hold the file we just
    // wrote open in its child.
    let mut out = std::process::Command::new(&staged)
        .arg("--version")
        .output();
    for _ in 0..20 {
        match &out {
            Err(e) if e.raw_os_error() == Some(26) => {
                std::thread::sleep(Duration::from_millis(50));
                out = std::process::Command::new(&staged)
                    .arg("--version")
                    .output();
            }
            _ => break,
        }
    }
    let out = out.map_err(|e| format!("the new version doesn't start on this machine: {e}"));
    let ok = out.as_ref().is_ok_and(|o| {
        String::from_utf8_lossy(&o.stdout).trim() == format!("kvistcode-server {version}")
    });
    if !ok {
        let _ = std::fs::remove_file(&staged);
        return Err(out.err().unwrap_or_else(|| {
            format!("the downloaded file isn't kvistcode-server {version}; nothing was changed")
        }));
    }
    std::fs::rename(&staged, exe).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("couldn't replace {}: {e}", exe.display())
    })
}

/// Checks for, downloads and installs the latest release into `exe`. `Ok(None)`: up to date.
pub fn update_file(exe: &Path) -> Result<Option<String>, String> {
    can_replace(exe)?;
    let Some(rel) = latest()? else {
        return Ok(None);
    };
    let data = download(&rel)?;
    install(exe, &data, &rel.version)?;
    Ok(Some(rel.version))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Idle,
    Checking,
    Downloading,
    /// Installed; restarts once no agent is working.
    Waiting,
    Restarting,
}

impl Phase {
    fn word(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Checking => "checking",
            Phase::Downloading => "downloading",
            Phase::Waiting => "waiting",
            Phase::Restarting => "restarting",
        }
    }
}

struct State {
    phase: Phase,
    latest: Option<Release>,
    checked_at: Option<i64>,
    error: Option<String>,
    installed: Option<String>,
    now: bool,
}

/// The running server's updater: what the web app and `--update` talk to.
pub struct Updater {
    core: Core,
    exe: Option<PathBuf>,
    pub auto: bool,
    state: Mutex<State>,
    /// Woken when the server should restart into the new version.
    pub restart: Arc<Notify>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

impl Updater {
    pub fn new(core: Core, auto: bool) -> Arc<Self> {
        Arc::new(Self {
            core,
            exe: current_exe(),
            auto,
            state: Mutex::new(State {
                phase: Phase::Idle,
                latest: None,
                checked_at: None,
                error: None,
                installed: None,
                now: false,
            }),
            restart: Arc::new(Notify::new()),
        })
    }

    fn busy_agents(&self) -> usize {
        self.core
            .agents()
            .map(|agents| {
                agents
                    .iter()
                    .filter(|a| {
                        matches!(
                            a.status,
                            AgentStatus::Starting
                                | AgentStatus::Working
                                | AgentStatus::NeedsPermission
                        )
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    fn replace_problem(&self) -> Option<String> {
        match &self.exe {
            None => Some(
                "Kvistcode can't tell where it is installed, so it can't update itself.".into(),
            ),
            Some(exe) => can_replace(exe).err(),
        }
    }

    /// Looks for a newer release, blocking. Returns whether one is there.
    pub fn check_blocking(&self) -> bool {
        {
            let mut s = self.state.lock();
            if s.phase != Phase::Idle {
                return s.latest.is_some();
            }
            s.phase = Phase::Checking;
        }
        let found = latest();
        let mut s = self.state.lock();
        s.phase = Phase::Idle;
        s.checked_at = Some(now_ms());
        match found {
            Ok(rel) => {
                if let Some(r) = &rel {
                    info!(version = %r.version, "an update is available");
                }
                s.latest = rel;
                s.error = None;
            }
            Err(e) => {
                warn!("update check failed: {e}");
                s.error = Some(e);
            }
        }
        s.latest.is_some()
    }

    /// Downloads and installs, blocking, then waits for the agents and asks for a restart.
    /// `Ok(None)`: already up to date.
    pub fn install_blocking(self: &Arc<Self>, now: bool) -> Result<Option<String>, String> {
        {
            let mut s = self.state.lock();
            match s.phase {
                Phase::Waiting | Phase::Restarting => {
                    s.now |= now;
                    return Ok(s.installed.clone());
                }
                Phase::Checking | Phase::Downloading => {
                    return Err("An update is already being checked or downloaded.".into())
                }
                Phase::Idle => {}
            }
            if let Some(problem) = self.replace_problem() {
                return Err(problem);
            }
            s.phase = Phase::Downloading;
            s.error = None;
        }
        let exe = self.exe.clone().unwrap_or_default();
        let result = latest().and_then(|rel| match rel {
            None => Ok(None),
            Some(rel) => {
                let data = download(&rel)?;
                install(&exe, &data, &rel.version)?;
                Ok(Some(rel.version))
            }
        });
        let mut s = self.state.lock();
        match &result {
            Ok(Some(version)) => {
                info!(%version, "update installed; restarting once no agent is working");
                s.phase = Phase::Waiting;
                s.installed = Some(version.clone());
                s.now = now;
                drop(s);
                let me = self.clone();
                std::thread::spawn(move || me.wait_then_restart());
            }
            Ok(None) => {
                s.phase = Phase::Idle;
                s.latest = None;
                s.checked_at = Some(now_ms());
            }
            Err(e) => {
                warn!("update failed: {e}");
                s.phase = Phase::Idle;
                s.error = Some(e.clone());
            }
        }
        result
    }

    fn wait_then_restart(&self) {
        loop {
            let now = self.state.lock().now;
            if now || self.busy_agents() == 0 {
                break;
            }
            std::thread::sleep(POLL_BUSY);
        }
        self.state.lock().phase = Phase::Restarting;
        info!("restarting into the new version");
        self.restart.notify_one();
    }

    /// Checks every few hours; with `auto`, installs what it finds.
    pub fn spawn_background(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            loop {
                let u = me.clone();
                let found = tokio::task::spawn_blocking(move || u.check_blocking())
                    .await
                    .unwrap_or(false);
                if found && me.auto && me.replace_problem().is_none() {
                    let u = me.clone();
                    let _ = tokio::task::spawn_blocking(move || u.install_blocking(false)).await;
                }
                tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
            }
        })
    }
}

/// The updater as the web app sees it.
pub struct WebHandle(pub Arc<Updater>);

impl kvistcode_remote::ServerUpdate for WebHandle {
    fn status(&self) -> Value {
        let problem = self.0.replace_problem();
        let s = self.0.state.lock();
        json!({
            "current": VERSION,
            "latest": s.latest.as_ref().map(|r| json!({
                "version": r.version,
                "notes": r.notes,
                "url": r.page,
            })),
            "installed": s.installed,
            "checkedAt": s.checked_at,
            "phase": s.phase.word(),
            "waitingOn": if s.phase == Phase::Waiting { self.0.busy_agents() } else { 0 },
            "error": s.error,
            "canInstall": problem.is_none(),
            "reason": problem,
            "auto": self.0.auto,
        })
    }

    fn check(&self) {
        let me = self.0.clone();
        std::thread::spawn(move || {
            me.check_blocking();
        });
    }

    fn install(&self, now: bool) -> Result<(), String> {
        {
            let s = self.0.state.lock();
            if matches!(s.phase, Phase::Waiting | Phase::Restarting) {
                drop(s);
                self.0.install_blocking(now)?;
                return Ok(());
            }
            if matches!(s.phase, Phase::Checking | Phase::Downloading) {
                return Err("An update is already being checked or downloaded.".into());
            }
        }
        if let Some(problem) = self.0.replace_problem() {
            return Err(problem);
        }
        let me = self.0.clone();
        std::thread::spawn(move || {
            let _ = me.install_blocking(now);
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.9.1", "0.9.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.9.0", "0.9.0-beta.1"));
        assert!(!is_newer("0.9.0", "0.9.0"));
        assert!(!is_newer("0.8.9", "0.9.0"));
        assert!(!is_newer("0.9.1-rc.1", "0.9.1"));
        assert!(!is_newer("", "0.9.0"));
    }

    #[test]
    fn names_the_download_for_this_machine() {
        let name = asset_name("1.2.3");
        if cfg!(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )) {
            assert_eq!(
                name.unwrap(),
                format!(
                    "kvistcode-server-1.2.3-linux-{}.tar.gz",
                    std::env::consts::ARCH
                )
            );
        } else {
            assert!(name.is_none());
        }
    }

    #[test]
    fn the_key_matches_the_desktop_updater() {
        let conf = include_str!("../../../apps/desktop/src-tauri/tauri.conf.json");
        let conf: Value = serde_json::from_str(conf).unwrap();
        assert_eq!(conf["plugins"]["updater"]["pubkey"], PUBKEY);
    }

    #[test]
    fn refuses_unsigned_or_badly_signed_downloads() {
        assert!(verify(b"data", "").is_err());
        assert!(verify(b"data", "not base64!").is_err());
        // A well-formed minisign signature by some other key.
        let other = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1556193335\tfile:test\ny/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==\n";
        let wrapped = base64::engine::general_purpose::STANDARD.encode(other);
        assert!(verify(b"test", &wrapped).is_err());
    }

    #[test]
    fn accepts_what_the_tauri_signer_writes() {
        // Made with `tauri signer generate` (a throwaway key) and `tauri signer sign data.bin`,
        // the same commands CI uses on the release tarballs.
        let key = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IENBODU2QTczMUI2RkI3QzQKUldURXQyOGJjMnFGeXFqeHRrdmdFQ3BBcEx1UzQ3N1F4KzRrWkxBMncyUmtBQURiNGVIblluTzAK";
        let sig = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVURXQyOGJjMnFGeXRkYkphanlKUWVzdlFyNVpCa1VLTTVHNXFvWVVKaXJveWEyTDBlQlRuUllPK1JPc0VoMUJ6K0JHQnprUGtiYjAzZGZJaUo1cDMzb2RaNm85UXNMMWdjPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzk1NjQ0MDgxCWZpbGU6ZGF0YS5iaW4KTlhvMkRtY2lSNWNjZ0VnT0JtU1NCOUVsZmxGRU5Cc0NYWHZzK3NOdVJmWXUrZjFTUHdQRTZtZHpIN0lzOHJ5dGc4NFVpWlZYaFNjd3V3Y1VscEZRQlE9PQo=";
        let data = b"kvistcode update test\n";
        verify_with(key, data, sig).unwrap();
        // One byte off, or the real key: refused.
        assert!(verify_with(key, b"kvistcode update test!", sig).is_err());
        assert!(verify(data, sig).is_err());
    }

    /// A fake release: a script that answers `--version` like the real binary would.
    fn tarball(version: &str, file: &str) -> Vec<u8> {
        let script = format!("#!/bin/sh\necho kvistcode-server {version}\n");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_size(script.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(
                &mut header,
                format!("kvistcode-server-{version}-linux-x/{file}"),
                script.as_bytes(),
            )
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kvs-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn installs_a_good_binary_over_the_old_one() {
        let dir = scratch("good");
        let exe = dir.join("kvistcode-server");
        std::fs::write(&exe, "old").unwrap();
        can_replace(&exe).unwrap();
        install(&exe, &tarball("9.9.9", "kvistcode-server"), "9.9.9").unwrap();
        let out = std::process::Command::new(&exe)
            .arg("--version")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "kvistcode-server 9.9.9"
        );
        assert!(!dir.join(".kvistcode-server.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keeps_the_old_binary_when_the_new_one_is_wrong() {
        let dir = scratch("bad");
        let exe = dir.join("kvistcode-server");
        std::fs::write(&exe, "old").unwrap();
        // Reports another version.
        assert!(install(&exe, &tarball("9.9.8", "kvistcode-server"), "9.9.9").is_err());
        // Doesn't contain the binary.
        assert!(install(&exe, &tarball("9.9.9", "something-else"), "9.9.9").is_err());
        // Isn't a tarball at all.
        assert!(install(&exe, b"garbage", "9.9.9").is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert!(!dir.join(".kvistcode-server.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
