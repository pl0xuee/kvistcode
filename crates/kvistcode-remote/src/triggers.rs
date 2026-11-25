//! Inbound event triggers, the I/O half (the rules live in `kvistcode_core::triggers`).
//!
//! * GitHub triggers are polled through the `gh` sign-in Kvistcode already uses, every two
//!   minutes, only while an enabled trigger needs it; a rate-limit answer doubles the wait
//!   (up to half an hour).
//! * Webhook triggers are served by an opt-in listener on 127.0.0.1 only (off by default).
//!   To reach it from the internet, put a tunnel you control in front of that port. Each
//!   trigger has its own URL token plus a signature or bearer check; see the core module
//!   for the order of checks.
//! * Nothing here logs a URL token, a secret or event text.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State as AxState};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::Router;
use kvistcode_core::triggers::{
    self as core_triggers, Claim, HookOutcome, HookRequest, InboundEvent, Trigger, TriggerSource,
    TriggerStore, WebhookAuth,
};
use kvistcode_core::triggers_github::{
    is_rate_limited, iso_time, mention_since, parse_ci_failures, parse_mentions, CommentKind,
};
use kvistcode_core::{Agent, Core};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::Notify;
use tracing::{info, warn};

const POLL_EVERY: Duration = Duration::from_secs(120);
const MAX_POLL_WAIT: Duration = Duration::from_secs(30 * 60);

/// Headers a sender may put the signature in.
const SIGNATURE_HEADERS: [&str; 4] = [
    "x-kvistcode-signature",
    "x-hub-signature-256",
    "linear-signature",
    "sentry-hook-signature",
];
/// Headers a sender may put a delivery id in (a retry carries the same one).
const DELIVERY_HEADERS: [&str; 3] = [
    "x-kvistcode-delivery",
    "x-github-delivery",
    "linear-delivery",
];

#[derive(Default)]
struct Listener {
    task: Option<tokio::task::JoinHandle<()>>,
    port: Option<u16>,
    error: Option<String>,
}

pub struct Engine {
    core: Core,
    pub store: Arc<TriggerStore>,
    /// Wakes the poller (a trigger was saved, deleted or switched).
    pub wake: Notify,
    /// Project folder -> "owner/name", so `gh repo view` runs once per folder.
    repos: Mutex<HashMap<String, String>>,
    /// Triggers whose last poll failed (their error is cleared on the next good poll).
    poll_failed: Mutex<HashSet<String>>,
    /// Set by a rate-limit answer during the current round.
    limited: Mutex<bool>,
    listener: Mutex<Listener>,
}

static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();

/// The one engine of this process, made on first use.
pub fn engine(core: &Core) -> Arc<Engine> {
    ENGINE
        .get_or_init(|| Arc::new(Engine::new(core.clone())))
        .clone()
}

/// Starts the poller and, if the user turned it on, the webhook listener.
pub fn start(core: &Core, runtime: &tokio::runtime::Handle) {
    let eng = engine(core);
    let poller = eng.clone();
    runtime.spawn(async move { poller.poll_loop().await });
    runtime.spawn(async move {
        if let Err(e) = eng.apply_listener().await {
            warn!("trigger webhook listener: {e:#}");
        }
    });
}

impl Engine {
    pub fn new(core: Core) -> Self {
        let store = Arc::new(TriggerStore::open(core.data_dir()));
        Engine {
            core,
            store,
            wake: Notify::new(),
            repos: Mutex::default(),
            poll_failed: Mutex::default(),
            limited: Mutex::new(false),
            listener: Mutex::default(),
        }
    }

    // ------------------------------------------------------------ firing

    async fn launch(&self, trigger: &Trigger, event: &InboundEvent) -> anyhow::Result<Agent> {
        let result = self
            .core
            .create_agent(core_triggers::request_for(trigger, event))
            .await;
        match &result {
            Ok(agent) => {
                info!("trigger {} started agent {}", trigger.id, agent.id);
                self.store.record_outcome(&trigger.id, Ok(&agent.id));
            }
            Err(e) => {
                warn!("trigger {} could not start an agent: {e:#}", trigger.id);
                self.store
                    .record_outcome(&trigger.id, Err(&format!("{e:#}")));
            }
        }
        result
    }

    /// Runs `event` through the dedupe and daily cap and starts the agent if it passes.
    async fn fire(&self, id: &str, event: InboundEvent) {
        match self
            .store
            .claim(id, &event.id, &core_triggers::today(), now_ms(), false)
        {
            Claim::Fire(t) => {
                let _ = self.launch(&t, &event).await;
            }
            Claim::CapReached => self
                .store
                .set_error(id, Some("Daily limit reached; an event was skipped")),
            Claim::Duplicate | Claim::Disabled | Claim::Missing => {}
        }
    }

    /// The Test button: a made-up event, run now whether or not the trigger is enabled.
    pub async fn test(&self, id: &str) -> anyhow::Result<Agent> {
        let event = InboundEvent {
            id: format!("test:{}", uuid::Uuid::new_v4()),
            label: "test event".into(),
            text:
                "This is a test sent from the Test button in Kvistcode. Nothing happened elsewhere."
                    .into(),
        };
        match self
            .store
            .claim(id, &event.id, &core_triggers::today(), now_ms(), true)
        {
            Claim::Fire(t) => self.launch(&t, &event).await,
            Claim::CapReached => bail!("This trigger has reached its daily limit"),
            _ => bail!("Trigger not found"),
        }
    }

    // ------------------------------------------------------------ GitHub polling

    async fn poll_loop(self: Arc<Self>) {
        let mut wait = POLL_EVERY;
        loop {
            if self.core.is_shutting_down() {
                return;
            }
            if !self.store.any_github_enabled() {
                // Nothing to read: no `gh` calls until a trigger is saved.
                self.wake.notified().await;
                wait = POLL_EVERY;
                continue;
            }
            *self.limited.lock() = false;
            for t in self
                .store
                .list()
                .into_iter()
                .filter(|t| t.enabled && t.source.is_github())
            {
                match self.poll_one(&t).await {
                    Ok(()) => {
                        if self.poll_failed.lock().remove(&t.id) {
                            self.store.set_error(&t.id, None);
                        }
                    }
                    Err(e) => {
                        self.poll_failed.lock().insert(t.id.clone());
                        self.store.set_error(&t.id, Some(&format!("{e:#}")));
                    }
                }
            }
            wait = if std::mem::take(&mut *self.limited.lock()) {
                (wait * 2).min(MAX_POLL_WAIT)
            } else {
                POLL_EVERY
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.wake.notified() => {}
            }
        }
    }

    async fn gh(&self, cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
        match crate::pr_status::gh(cwd, args).await {
            Ok(out) => Ok(out),
            Err(raw) => {
                if is_rate_limited(&raw) {
                    *self.limited.lock() = true;
                }
                Err(crate::gh_errors::explain_gh_error(raw))
            }
        }
    }

    async fn repo_slug(&self, cwd: &Path) -> anyhow::Result<String> {
        let key = cwd.to_string_lossy().into_owned();
        if let Some(s) = self.repos.lock().get(&key) {
            return Ok(s.clone());
        }
        let out = self
            .gh(
                cwd,
                &[
                    "repo",
                    "view",
                    "--json",
                    "nameWithOwner",
                    "--jq",
                    ".nameWithOwner",
                ],
            )
            .await?;
        let slug = out.trim().to_string();
        let ok = slug.split_once('/').is_some()
            && slug
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'));
        if !ok {
            bail!("Couldn't tell which GitHub repository this folder belongs to");
        }
        self.repos.lock().insert(key, slug.clone());
        Ok(slug)
    }

    async fn poll_one(&self, t: &Trigger) -> anyhow::Result<()> {
        let cwd = Path::new(&t.project_path);
        if !cwd.is_dir() {
            bail!("Folder not found: {}", t.project_path);
        }
        match t.source {
            TriggerSource::GithubCi => {
                let json = self
                    .gh(
                        cwd,
                        &[
                            "pr",
                            "list",
                            "--state",
                            "open",
                            "--limit",
                            "30",
                            "--json",
                            "number,title,headRefName,headRefOid,url,statusCheckRollup",
                        ],
                    )
                    .await?;
                let events = parse_ci_failures(&json).context("reading GitHub's answer")?;
                if !self.store.is_baselined(&t.id) {
                    // What is red right now is not news; only new failures fire.
                    let ids: Vec<String> = events.iter().map(|e| e.id.clone()).collect();
                    self.store.baseline(&t.id, &ids);
                    return Ok(());
                }
                for e in events {
                    self.fire(&t.id, e).await;
                }
            }
            TriggerSource::GithubMention => {
                let slug = self.repo_slug(cwd).await?;
                let polled_at = now_ms();
                let since = iso_time(mention_since(t.updated_at, t.last_polled_at));
                for (kind, path) in [
                    (CommentKind::Issue, "issues/comments"),
                    (CommentKind::Review, "pulls/comments"),
                ] {
                    let endpoint = format!(
                        "repos/{slug}/{path}?since={since}&per_page=100&sort=updated&direction=desc"
                    );
                    let json = self.gh(cwd, &["api", &endpoint]).await?;
                    let events = parse_mentions(&json, kind, &t.keyword, t.allow_outsiders)
                        .context("reading GitHub's answer")?;
                    for e in events {
                        self.fire(&t.id, e).await;
                    }
                }
                self.store.set_polled(&t.id, polled_at);
            }
            TriggerSource::Webhook => {}
        }
        Ok(())
    }

    // ------------------------------------------------------------ webhook listener

    /// Brings the listener in line with the saved switch and port.
    pub async fn apply_listener(self: &Arc<Self>) -> anyhow::Result<()> {
        let (enabled, port) = self.store.webhook_config();
        {
            let mut l = self.listener.lock();
            if let Some(task) = l.task.take() {
                task.abort();
            }
            l.port = None;
            l.error = None;
            if !enabled {
                return Ok(());
            }
        }
        // Loopback only: the sender reaches it through a tunnel the user sets up.
        let bound = tokio::net::TcpListener::bind(("127.0.0.1", port)).await;
        let listener = match bound {
            Ok(l) => l,
            Err(e) => {
                let msg = format!("Couldn't listen on port {port}: {e}");
                self.listener.lock().error = Some(msg.clone());
                bail!(msg);
            }
        };
        let app = router(self.clone());
        let task = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                warn!("trigger webhook listener stopped: {e}");
            }
        });
        let mut l = self.listener.lock();
        l.task = Some(task);
        l.port = Some(port);
        Ok(())
    }

    pub fn view(&self) -> TriggersView {
        let (enabled, port) = self.store.webhook_config();
        let l = self.listener.lock();
        TriggersView {
            triggers: self.store.list(),
            webhook: WebhookStatus {
                enabled,
                port,
                running: l.port.is_some(),
                error: l.error.clone(),
            },
        }
    }
}

fn now_ms() -> i64 {
    kvistcode_core::now_ms()
}

pub fn router(eng: Arc<Engine>) -> Router {
    Router::new()
        .route("/hook/{token}", post(hook))
        .layer(DefaultBodyLimit::max(core_triggers::MAX_BODY_BYTES))
        .with_state(eng)
}

async fn hook(
    AxState(eng): AxState<Arc<Engine>>,
    UrlPath(token): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, &'static str) {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let signatures: Vec<&str> = SIGNATURE_HEADERS.iter().filter_map(|n| header(n)).collect();
    let req = HookRequest {
        token: &token,
        body: &body,
        authorization: header("authorization"),
        signatures,
        delivery_id: DELIVERY_HEADERS.iter().find_map(|n| header(n)),
    };
    match core_triggers::handle_webhook(
        &eng.store,
        &req,
        Instant::now(),
        &core_triggers::today(),
        now_ms(),
    ) {
        HookOutcome::Accepted(trigger, event) => {
            // Answer at once; starting an agent can take a while.
            tokio::spawn(async move {
                let _ = eng.launch(&trigger, &event).await;
            });
            (StatusCode::ACCEPTED, "accepted")
        }
        HookOutcome::Duplicate => (StatusCode::OK, "duplicate"),
        HookOutcome::Rejected(code, why) => (
            StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST),
            why,
        ),
    }
}

// ---------------------------------------------------------------- views

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookStatus {
    pub enabled: bool,
    pub port: u16,
    /// The listener is accepting connections right now.
    pub running: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggersView {
    pub triggers: Vec<Trigger>,
    pub webhook: WebhookStatus,
}

/// What the user pastes into the sending service.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookInfo {
    pub url: String,
    pub secret: String,
    pub auth: WebhookAuth,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use kvistcode_core::triggers::hmac_sha256;
    use tower::ServiceExt;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[tokio::test]
    async fn the_listener_answers_only_to_signed_requests_on_a_known_url() {
        let dir = std::env::temp_dir().join(format!("kvistcode-trig-http-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (sink, _rx) = {
            struct Quiet;
            impl kvistcode_core::EventSink for Quiet {
                fn emit(&self, _: kvistcode_core::KvistcodeEvent) {}
            }
            (Arc::new(Quiet) as Arc<dyn kvistcode_core::EventSink>, ())
        };
        let core = Core::open(dir.clone(), sink).unwrap();
        let eng = Arc::new(Engine::new(core));
        let t = eng
            .store
            .save(Trigger {
                id: String::new(),
                name: "Sentry".into(),
                enabled: true,
                source: TriggerSource::Webhook,
                provider: kvistcode_core::Provider::Claude,
                account_id: "none".into(),
                project_path: dir.to_string_lossy().into_owned(),
                prompt: "Look".into(),
                worktree: false,
                model: None,
                custom_agent_id: None,
                run_without_asking: false,
                keyword: String::new(),
                allow_outsiders: false,
                auth: WebhookAuth::Hmac,
                daily_cap: 5,
                path_token: String::new(),
                created_at: 0,
                updated_at: 0,
                last_fired_at: None,
                last_agent_id: None,
                last_error: None,
                last_polled_at: None,
                fired_day: String::new(),
                fired_today: 0,
                fired_total: 0,
            })
            .unwrap();
        let secret = eng.store.secret(&t.id).unwrap();
        let app = router(eng.clone());
        let body = r#"{"title":"boom"}"#;
        let sig = hex(&hmac_sha256(secret.as_bytes(), body.as_bytes()));
        let send = |path: String, sig: Option<String>, body: String| {
            let app = app.clone();
            async move {
                let mut r = Request::builder().method("POST").uri(path);
                if let Some(s) = sig {
                    r = r.header("x-hub-signature-256", format!("sha256={s}"));
                }
                app.oneshot(r.body(Body::from(body)).unwrap())
                    .await
                    .unwrap()
                    .status()
            }
        };
        let url = format!("/hook/{}", t.path_token);
        assert_eq!(
            send("/hook/nope".into(), Some(sig.clone()), body.into()).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            send(url.clone(), None, body.into()).await,
            StatusCode::UNAUTHORIZED
        );
        let huge = "x".repeat(core_triggers::MAX_BODY_BYTES + 10);
        assert_eq!(
            send(url.clone(), Some(sig.clone()), huge).await,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            send(url, Some(sig), body.into()).await,
            StatusCode::ACCEPTED
        );
        // GET isn't a thing here.
        let get = Request::builder()
            .uri(format!("/hook/{}", t.path_token))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(get).await.unwrap().status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
