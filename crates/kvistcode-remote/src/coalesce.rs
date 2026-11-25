//! Shrinking a batch of core events before they go to a client.

use kvistcode_core::{DeltaField, KvistcodeEvent};

/// Shrinks a batch without changing what clients end up with:
/// - text deltas for the same field merge when contiguous, even with other agents'
///   events in between (until that item is replaced by a full upsert);
/// - terminal output for the same terminal merges (until it exits);
/// - only the latest `AgentUpdated` per agent is kept (each carries the whole agent).
pub fn coalesce(batch: Vec<KvistcodeEvent>) -> Vec<KvistcodeEvent> {
    use std::collections::HashMap;

    let mut out: Vec<Option<KvistcodeEvent>> = Vec::with_capacity(batch.len());
    // (agent, item, field) → slot of the last delta for it, and where that delta ends
    // (in UTF-16 units, as offsets are), so a merge doesn't recount the whole delta.
    let mut deltas: HashMap<(String, String, DeltaField), (usize, usize)> = HashMap::new();
    let mut terminals: HashMap<String, usize> = HashMap::new();
    let mut agents: HashMap<String, usize> = HashMap::new();
    let mut side_chats: HashMap<String, usize> = HashMap::new();
    for event in batch {
        match event {
            KvistcodeEvent::TextDelta {
                agent_id,
                item_id,
                field,
                delta,
                offset,
            } => {
                let key = (agent_id, item_id, field);
                let end = offset + delta.encode_utf16().count();
                if let Some(&(slot, slot_end)) = deltas.get(&key) {
                    if let Some(KvistcodeEvent::TextDelta { delta: d1, .. }) = &mut out[slot] {
                        if slot_end == offset {
                            d1.push_str(&delta);
                            deltas.insert(key, (slot, end));
                            continue;
                        }
                    }
                }
                deltas.insert(key.clone(), (out.len(), end));
                out.push(Some(KvistcodeEvent::TextDelta {
                    agent_id: key.0,
                    item_id: key.1,
                    field: key.2,
                    delta,
                    offset,
                }));
            }
            KvistcodeEvent::TerminalOutput { terminal_id, data } => {
                if let Some(&slot) = terminals.get(&terminal_id) {
                    if let Some(KvistcodeEvent::TerminalOutput { data: d1, .. }) = &mut out[slot] {
                        d1.push_str(&data);
                        continue;
                    }
                }
                terminals.insert(terminal_id.clone(), out.len());
                out.push(Some(KvistcodeEvent::TerminalOutput { terminal_id, data }));
            }
            KvistcodeEvent::AgentUpdated { agent } => {
                if let Some(slot) = agents.insert(agent.id.clone(), out.len()) {
                    out[slot] = None;
                }
                out.push(Some(KvistcodeEvent::AgentUpdated { agent }));
            }
            KvistcodeEvent::SideChatUpdated { entry } => {
                if let Some(slot) = side_chats.insert(entry.id.clone(), out.len()) {
                    out[slot] = None;
                }
                out.push(Some(KvistcodeEvent::SideChatUpdated { entry }));
            }
            other => {
                // Later deltas must not merge across a full replacement or removal.
                match &other {
                    KvistcodeEvent::ItemUpserted { item } => {
                        deltas.retain(|(_, i, _), _| i != &item.id);
                    }
                    KvistcodeEvent::AgentRemoved { agent_id } => {
                        deltas.retain(|(a, _, _), _| a != agent_id);
                        agents.remove(agent_id);
                    }
                    KvistcodeEvent::TerminalExit { terminal_id, .. } => {
                        terminals.remove(terminal_id);
                    }
                    _ => {}
                }
                out.push(Some(other));
            }
        }
    }
    out.into_iter().flatten().collect()
}
