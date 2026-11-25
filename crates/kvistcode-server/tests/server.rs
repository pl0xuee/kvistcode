//! Runs the built `kvistcode-server` against a scratch data folder and talks to it over HTTP.
//! The server gets a scratch HOME, in-memory secrets, no session bus, no display and
//! `--no-tool-updates`, so it can't touch the real keychain, provider folders or desktop.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

static N: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "kvistcode-server-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        Scratch(dir)
    }
    fn data(&self) -> PathBuf {
        self.0.join("data")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn server_command(scratch: &Scratch) -> Command {
    let home = scratch.0.join("home");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_kvistcode-server"));
    cmd.env_remove("BROWSER")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("KVISTCODE_SECRETS", "memory")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
        .arg("--no-tool-updates")
        .arg("--data-dir")
        .arg(scratch.data());
    cmd
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

struct Server {
    child: Child,
    port: u16,
    /// The pairing link printed at startup.
    link: String,
}

impl Server {
    fn start(scratch: &Scratch) -> Server {
        let port = free_port();
        let mut child = server_command(scratch)
            .arg("--port")
            .arg(port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("starting kvistcode-server");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line.contains("#pair=") {
                    let _ = tx.send(line.trim().to_string());
                }
            }
        });
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(link) => Server { child, port, link },
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the server never printed a pairing link");
            }
        }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Ask it to stop the way a service manager would, and wait for it.
    fn stop(&mut self) -> std::process::ExitStatus {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status();
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if started.elapsed() > Duration::from_secs(30) {
                let _ = self.child.kill();
                panic!("the server did not stop on SIGTERM");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build(),
    )
}

fn pair(base: &str, link: &str) -> (u16, String) {
    let code = link.rsplit("#pair=").next().unwrap();
    let body = serde_json::to_vec(&json!({ "code": code, "deviceName": "Test browser" })).unwrap();
    let mut res = http()
        .post(&format!("{base}/api/pair"))
        .header("Content-Type", "application/json")
        .send(&body[..])
        .unwrap();
    (
        res.status().as_u16(),
        res.body_mut().read_to_string().unwrap_or_default(),
    )
}

fn get(base: &str, path: &str, token: Option<&str>) -> (u16, String) {
    let agent = http();
    let mut req = agent.get(&format!("{base}{path}"));
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut res = req.call().unwrap();
    (
        res.status().as_u16(),
        res.body_mut().read_to_string().unwrap_or_default(),
    )
}

fn token_from(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap();
    v["token"].as_str().expect("a device token").to_string()
}

fn pair_command(scratch: &Scratch) -> std::process::Output {
    server_command(scratch)
        .arg("--pair")
        .output()
        .expect("running --pair")
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn pairs_a_browser_and_serves_state() {
    let scratch = Scratch::new();
    let mut server = Server::start(&scratch);
    let base = server.base();

    // The page itself is served without a token; the API is not.
    assert_eq!(get(&base, "/", None).0, 200);
    assert_eq!(get(&base, "/api/state", None).0, 401);

    // The printed link pairs once.
    let (code, body) = pair(&base, &server.link);
    assert_eq!(code, 200, "{body}");
    let token = token_from(&body);
    assert_eq!(
        pair(&base, &server.link).0,
        401,
        "a pairing link works once"
    );

    let (code, body) = get(&base, "/api/state", Some(&token));
    assert_eq!(code, 200, "{body}");
    let state: Value = serde_json::from_str(&body).unwrap();
    assert!(state.is_object());

    // The session says there is no desktop window behind this.
    let (code, body) = get(&base, "/api/session", Some(&token));
    assert_eq!(code, 200, "{body}");
    let session: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(session["headless"], json!(true));

    // The data folder and what is in it are private.
    assert_eq!(mode(&scratch.data()), 0o700);
    assert_eq!(mode(&scratch.data().join("remote.json")), 0o600);

    // SIGTERM is a clean stop that leaves nothing for the next start to trip over.
    let status = server.stop();
    assert!(status.success(), "{status:?}");
    assert!(!scratch.data().join("admin.sock").exists());
}

#[test]
fn pair_flag_gives_a_new_link_while_the_server_runs() {
    let scratch = Scratch::new();
    let server = Server::start(&scratch);
    let base = server.base();

    let out = pair_command(&scratch);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("expires in"), "{text}");
    let link = text
        .lines()
        .find(|l| l.contains("#pair="))
        .expect("a pairing link")
        .trim()
        .to_string();
    assert!(link.starts_with(&base), "{link} should point at {base}");

    let (code, body) = pair(&base, &link);
    assert_eq!(code, 200, "{body}");
    let token = token_from(&body);
    assert_eq!(get(&base, "/api/state", Some(&token)).0, 200);
}

#[test]
fn pair_flag_says_so_when_no_server_is_running() {
    let scratch = Scratch::new();
    let out = pair_command(&scratch);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("kvistcode-server"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn a_second_server_on_the_same_data_folder_is_refused() {
    let scratch = Scratch::new();
    let _server = Server::start(&scratch);

    let out = server_command(&scratch)
        .arg("--port")
        .arg(free_port().to_string())
        .output()
        .expect("running a second server");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("already using"), "{err}");
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn unpair_all_signs_every_browser_out() {
    let scratch = Scratch::new();
    let server = Server::start(&scratch);
    let base = server.base();
    let (code, body) = pair(&base, &server.link);
    assert_eq!(code, 200, "{body}");
    let token = token_from(&body);
    assert_eq!(get(&base, "/api/state", Some(&token)).0, 200);

    let out = server_command(&scratch)
        .arg("--unpair-all")
        .output()
        .expect("running --unpair-all");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(get(&base, "/api/state", Some(&token)).0, 401);

    // A new link still pairs afterwards.
    let out = pair_command(&scratch);
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    let link = text.lines().find(|l| l.contains("#pair=")).unwrap().trim();
    assert_eq!(pair(&base, link).0, 200);
}

#[test]
fn an_open_data_folder_is_made_private() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new();
    std::fs::create_dir_all(scratch.data()).unwrap();
    std::fs::set_permissions(scratch.data(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let _server = Server::start(&scratch);
    assert_eq!(mode(&scratch.data()), 0o700);
}
