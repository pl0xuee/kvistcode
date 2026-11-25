//! Tauri commands for inbound event triggers. The engine (polling, webhook listener) lives in
//! `kvistcode_remote::triggers`, shared with the headless server.

use anyhow::anyhow;
use kvistcode_core::triggers::{Trigger, TriggerSource};
use kvistcode_core::{Agent, Core, Provider};
use kvistcode_remote::triggers::{engine, TriggersView, WebhookInfo, WebhookStatus};
use tauri::State;

pub use kvistcode_remote::triggers::start;

fn msg(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
pub async fn list_triggers(core: State<'_, Core>) -> Result<TriggersView, String> {
    Ok(engine(&core).view())
}

#[tauri::command]
pub async fn save_trigger(core: State<'_, Core>, trigger: Trigger) -> Result<Trigger, String> {
    let account = core.account(&trigger.account_id).map_err(msg)?;
    if account.provider != trigger.provider {
        return Err("That account belongs to a different provider".into());
    }
    if trigger.provider == Provider::Custom && trigger.custom_agent_id.is_none() {
        return Err("Choose a custom agent".into());
    }
    let eng = engine(&core);
    let saved = eng.store.save(trigger).map_err(msg)?;
    eng.wake.notify_one();
    Ok(saved)
}

#[tauri::command]
pub async fn delete_trigger(core: State<'_, Core>, trigger_id: String) -> Result<(), String> {
    let eng = engine(&core);
    eng.store.delete(&trigger_id);
    eng.wake.notify_one();
    Ok(())
}

#[tauri::command]
pub async fn test_trigger(core: State<'_, Core>, trigger_id: String) -> Result<Agent, String> {
    engine(&core).test(&trigger_id).await.map_err(msg)
}

#[tauri::command]
pub async fn set_trigger_webhook(
    core: State<'_, Core>,
    enabled: bool,
    port: Option<u16>,
) -> Result<WebhookStatus, String> {
    let eng = engine(&core);
    eng.store.set_webhook_config(enabled, port).map_err(msg)?;
    // A port that is taken shows up in the status, not as a failed save.
    let _ = eng.apply_listener().await;
    Ok(eng.view().webhook)
}

#[tauri::command]
pub async fn trigger_webhook_info(
    core: State<'_, Core>,
    trigger_id: String,
) -> Result<WebhookInfo, String> {
    let eng = engine(&core);
    let t = eng
        .store
        .get(&trigger_id)
        .filter(|t| t.source == TriggerSource::Webhook)
        .ok_or_else(|| "Trigger not found".to_string())?;
    let secret = eng
        .store
        .secret(&t.id)
        .ok_or_else(|| msg(anyhow!("The webhook secret is missing; regenerate it")))?;
    let (_, port) = eng.store.webhook_config();
    Ok(WebhookInfo {
        url: format!("http://127.0.0.1:{port}/hook/{}", t.path_token),
        secret,
        auth: t.auth,
    })
}

#[tauri::command]
pub async fn regenerate_trigger_webhook(
    core: State<'_, Core>,
    trigger_id: String,
) -> Result<Trigger, String> {
    engine(&core)
        .store
        .regenerate_webhook(&trigger_id)
        .map_err(msg)
}
