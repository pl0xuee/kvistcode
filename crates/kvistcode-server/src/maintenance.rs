//! Kvistcode's own copies of the agent tools (Claude Code, Codex, Grok): install what's
//! missing for the providers in use and keep them current. The same rules as the desktop
//! app's `tools.rs`, without its window state.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kvistcode_core::toolchain::{self, Log, UpdateTarget};
use kvistcode_core::{providers, AgentStatus, Core, KvistcodeEvent, Provider};
use tracing::{info, warn};

/// Let the server settle before anything downloads.
const STARTUP_DELAY: Duration = Duration::from_secs(12);
/// How often to look for newer releases.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// Retry sooner after a failed check (offline) or when updates had to wait for a busy agent.
const RETRY_EVERY: Duration = Duration::from_secs(15 * 60);

pub fn spawn(core: Core, data_dir: PathBuf) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            let wait = maintain(&core, &data_dir).await;
            tokio::time::sleep(wait).await;
        }
    })
}

fn log() -> Log {
    Arc::new(|line: &str| {
        if !line.trim().is_empty() {
            info!("tools: {line}");
        }
    })
}

/// A provider's agent is busy: replacing its files could break a running turn.
fn busy(core: &Core, provider: Provider) -> bool {
    core.agents().unwrap_or_default().iter().any(|a| {
        a.provider == provider
            && matches!(
                a.status,
                AgentStatus::Working | AgentStatus::NeedsPermission | AgentStatus::Starting
            )
    })
}

/// A provider Kvistcode should have its own copy of: one that's installed on the system,
/// or that has agents here.
fn in_use(core: &Core, provider: Provider) -> bool {
    providers::resolve_program(providers::cli_name(provider)).is_some()
        || core
            .agents()
            .unwrap_or_default()
            .iter()
            .any(|a| a.provider == provider)
}

pub async fn refresh(core: &Core, data_dir: &std::path::Path) {
    let providers = providers::detect(data_dir).await;
    core.emit(KvistcodeEvent::ProvidersChanged { providers });
}

async fn maintain(core: &Core, data_dir: &std::path::Path) -> Duration {
    let mut wait = CHECK_EVERY;
    for provider in Provider::ALL {
        if toolchain::needs_install(provider, data_dir) && in_use(core, provider) {
            if busy(core, provider) {
                wait = RETRY_EVERY;
                continue;
            }
            info!(
                provider = provider.as_str(),
                "installing Kvistcode's own copy"
            );
            if let Err(e) = toolchain::install(provider, data_dir, log()).await {
                warn!(provider = provider.as_str(), "couldn't install: {e:#}");
                wait = RETRY_EVERY;
            }
            refresh(core, data_dir).await;
        }
    }
    if let Err(e) = toolchain::check_updates(&log()).await {
        warn!("update check failed: {e:#}");
        return RETRY_EVERY;
    }
    refresh(core, data_dir).await;
    if core
        .settings()
        .map(|s| s.auto_update_tools)
        .unwrap_or(false)
    {
        for provider in Provider::ALL {
            if !toolchain::update_available(provider, data_dir)
                || toolchain::is_held(data_dir, provider)
            {
                continue;
            }
            if busy(core, provider) {
                wait = RETRY_EVERY;
                continue;
            }
            info!(provider = provider.as_str(), "updating");
            if let Err(e) = toolchain::update(provider, data_dir, UpdateTarget::Latest, log()).await
            {
                warn!(provider = provider.as_str(), "update failed: {e:#}");
            }
            refresh(core, data_dir).await;
        }
    }
    wait
}
