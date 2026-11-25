//! Bridges core events to the webview (`kvistcode://event`) and to the attention
//! engine / tray. `emit` only pushes onto a channel, so it never blocks the core.

use std::sync::Arc;
use std::time::Duration;

use crate::AppHandle;
use kvistcode_core::{EventSink, KvistcodeEvent};
use kvistcode_remote::coalesce::coalesce;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::warn;

use crate::attention::Attention;

pub const EVENT_CHANNEL: &str = "kvistcode://event";

pub struct TauriSink {
    tx: UnboundedSender<KvistcodeEvent>,
}

impl EventSink for TauriSink {
    fn emit(&self, event: KvistcodeEvent) {
        // Only fails once the dispatcher is gone (app shutting down).
        let _ = self.tx.send(event);
    }
}

pub fn channel() -> (Arc<dyn EventSink>, UnboundedReceiver<KvistcodeEvent>) {
    let (tx, rx) = unbounded_channel();
    (Arc::new(TauriSink { tx }), rx)
}

/// Most events taken per batch, so one busy agent can't starve the rest.
const MAX_BATCH: usize = 512;
/// Pause after forwarding streamed text so the next chunks arrive together: a
/// fast stream becomes ~60 window messages a second instead of hundreds.
const STREAM_PACING: Duration = Duration::from_millis(16);

pub fn spawn_dispatcher(app: AppHandle, mut rx: UnboundedReceiver<KvistcodeEvent>) {
    tauri::async_runtime::spawn(async move {
        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            while batch.len() < MAX_BATCH {
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
            let batch = coalesce(batch);
            // One window message per cycle (an array), instead of one per event.
            if let Err(e) = app.emit(EVENT_CHANNEL, &batch) {
                warn!("failed to emit events to the window: {e}");
            }
            for event in batch {
                dispatch(&app, event);
            }
            if streaming {
                tokio::time::sleep(STREAM_PACING).await;
            }
        }
    });
}

fn dispatch(app: &AppHandle, event: KvistcodeEvent) {
    {
        {
            if let Some(remote) = app.try_state::<Arc<crate::remote::Remote>>() {
                remote.publish(&event);
            }
            match event {
                // Answered (here, on the phone or elsewhere): drop its notification.
                KvistcodeEvent::ItemUpserted { item } => {
                    if let kvistcode_core::ItemBody::Permission {
                        request_id,
                        resolution: Some(_),
                        ..
                    }
                    | kvistcode_core::ItemBody::Question {
                        request_id,
                        resolution: Some(_),
                        ..
                    } = &item.body
                    {
                        if let Some(att) = app.try_state::<Arc<Attention>>() {
                            att.on_permission_resolved(&item.agent_id, request_id);
                        }
                    }
                }
                KvistcodeEvent::Attention {
                    agent_id,
                    reason,
                    title,
                    body,
                    request_id,
                } => {
                    if let Some(va) = app.try_state::<crate::voice_assistant::VoiceAssistant>() {
                        va.on_attention(&agent_id, reason);
                    }
                    if let Some(att) = app.try_state::<Arc<Attention>>() {
                        let att = att.inner().clone();
                        // Settings reads, notification + HTTP calls: keep them off the dispatcher.
                        tauri::async_runtime::spawn_blocking(move || {
                            att.on_attention(
                                &agent_id,
                                reason,
                                &title,
                                &body,
                                request_id.as_deref(),
                            )
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvistcode_core::DeltaField;

    fn delta(item: &str, text: &str, offset: usize) -> KvistcodeEvent {
        KvistcodeEvent::TextDelta {
            agent_id: "a".into(),
            item_id: item.into(),
            field: DeltaField::Text,
            delta: text.into(),
            offset,
        }
    }

    fn texts(out: &[KvistcodeEvent]) -> Vec<(String, String, usize)> {
        out.iter()
            .filter_map(|e| match e {
                KvistcodeEvent::TextDelta {
                    item_id,
                    delta,
                    offset,
                    ..
                } => Some((item_id.clone(), delta.clone(), *offset)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn merges_contiguous_deltas_across_interleaving() {
        let out = coalesce(vec![
            delta("m", "hé", 0),
            delta("t", "x", 0),
            delta("m", "🎉", 2),
            delta("t", "y", 1),
            delta("m", "!", 4),
            // A gap stays separate so the client notices it and refetches.
            delta("m", "z", 9),
        ]);
        assert_eq!(
            texts(&out),
            vec![
                ("m".into(), "hé🎉!".into(), 0),
                ("t".into(), "xy".into(), 0),
                ("m".into(), "z".into(), 9),
            ]
        );
    }

    #[test]
    fn full_upsert_stops_merging() {
        let item: kvistcode_core::TimelineItem = serde_json::from_value(serde_json::json!({
            "id": "m", "agentId": "a", "seq": 1, "ts": 0,
            "kind": "agent_message", "text": "hé"
        }))
        .unwrap();
        let out = coalesce(vec![
            delta("m", "hé", 0),
            KvistcodeEvent::ItemUpserted { item },
            delta("m", "!", 2),
        ]);
        assert_eq!(
            out.len(),
            3,
            "the delta after the upsert isn't merged before it"
        );
    }
}
