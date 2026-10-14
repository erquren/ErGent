//! Size-bounded inventory transport. A transfer is committed only after its final chunk.
use crate::MAX_MESSAGE;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Leaves ample room for JSON/base64 overhead inside the 64 KiB WebSocket limit.
pub const CHUNK_BYTES: usize = 32 * 1024;
pub const TRANSFER_TIMEOUT_SECS: u64 = 60;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InventoryChunk {
    // Decimal sequence, starting at 1 on each new WebSocket. Never reused on that socket.
    pub transfer_id: String,
    pub index: u32,
    pub last: bool,
    pub data_base64: String,
}

/// Split serialized UTF-8 bytes, not records, so even a large individual record is safe.
pub fn chunks<T: Serialize>(value: &T, transfer_id: u64) -> Result<Vec<InventoryChunk>, String> {
    if transfer_id == 0 {
        return Err("invalid inventory transfer ID".into());
    }
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let count = bytes.len().div_ceil(CHUNK_BYTES);
    if count > u32::MAX as usize {
        return Err("inventory transfer has too many chunks".into());
    }
    Ok(bytes
        .chunks(CHUNK_BYTES)
        .enumerate()
        .map(|(index, data)| InventoryChunk {
            transfer_id: transfer_id.to_string(),
            index: index as u32,
            last: index + 1 == count,
            data_base64: STANDARD.encode(data),
        })
        .collect())
}

/// Connection-local staging. Dropping it on disconnect discards only the incomplete transfer.
/// No inventory is exposed until every contiguous chunk has arrived; replay is an error.
#[derive(Default)]
pub struct InventoryAssembler {
    completed: u64,
    next_index: u32,
    bytes: Vec<u8>,
}
impl InventoryAssembler {
    pub fn in_progress(&self) -> bool {
        self.next_index != 0
    }

    pub fn push(&mut self, chunk: InventoryChunk) -> Result<Option<Vec<u8>>, &'static str> {
        let expected = self
            .completed
            .checked_add(1)
            .ok_or("transfer ID overflow")?;
        if chunk.transfer_id != expected.to_string() || chunk.index != self.next_index {
            return Err("out-of-order or replayed inventory chunk");
        }
        // Bound before decoding, including when callers do not use a WebSocket.
        if chunk.data_base64.len() > CHUNK_BYTES.div_ceil(3) * 4 {
            return Err("inventory chunk too large");
        }
        let data = STANDARD
            .decode(&chunk.data_base64)
            .map_err(|_| "invalid inventory base64")?;
        if data.is_empty() || data.len() > CHUNK_BYTES || (!chunk.last && data.len() != CHUNK_BYTES)
        {
            return Err("invalid inventory chunk boundary");
        }
        self.bytes.extend_from_slice(&data);
        if chunk.last {
            self.completed = expected;
            self.next_index = 0;
            Ok(Some(std::mem::take(&mut self.bytes)))
        } else {
            self.next_index = self
                .next_index
                .checked_add(1)
                .ok_or("chunk index overflow")?;
            Ok(None)
        }
    }
}

/// All send paths must check the final envelope, not just the fragment payload.
pub fn encode_message<T: Serialize>(value: &T) -> Result<String, String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    if text.len() > MAX_MESSAGE {
        return Err("message exceeds 64 KiB".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentEvent, Event, SessionInfo};

    fn sessions() -> Vec<SessionInfo> {
        (0..1501)
            .map(|i| SessionInfo {
                id: i.to_string(),
                machine_id: "machine".into(),
                agent_epoch: "epoch".into(),
                title: "历史\\\"\n中文🙂".repeat(20),
                terminal_theme: None,
                cwd: "/tmp".into(),
                shell: "/bin/sh".into(),
                lifecycle: "exited".into(),
                cols: 80,
                rows: 24,
                exit_code: Some(0),
                output_truncated: false,
                tool_status: None,
            })
            .collect()
    }

    #[test]
    fn large_hello_and_inventories_round_trip_with_bounded_envelopes() {
        let sessions = sessions();
        for event in [
            AgentEvent::Hello {
                epoch: "epoch".into(),
                default_cwd: "/tmp".into(),
                sessions: sessions.clone(),
            },
            AgentEvent::Inventory {
                sessions: sessions.clone(),
            },
        ] {
            assert!(serde_json::to_vec(&event).unwrap().len() > MAX_MESSAGE);
            let mut assembler = InventoryAssembler::default();
            let mut result = None;
            for chunk in chunks(&event, 1).unwrap() {
                assert!(encode_message(&AgentEvent::InventoryChunk(chunk.clone())).is_ok());
                result = assembler.push(chunk).unwrap();
            }
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&result.unwrap()).unwrap(),
                serde_json::to_value(event).unwrap()
            );
        }
        let event = Event::Inventory {
            machines: vec![],
            sessions,
        };
        for chunk in chunks(&event, 1).unwrap() {
            assert!(encode_message(&Event::InventoryChunk(chunk)).is_ok());
        }
    }

    #[test]
    fn boundaries_empty_lists_and_large_individual_records() {
        for len in [
            0,
            CHUNK_BYTES - 2,
            CHUNK_BYTES - 1,
            CHUNK_BYTES,
            CHUNK_BYTES * 3,
        ] {
            let text = "a".repeat(len); // JSON quotes put these on both sides of exact boundaries.
            let mut assembler = InventoryAssembler::default();
            let parts = chunks(&text, 1).unwrap();
            for (index, part) in parts.iter().cloned().enumerate() {
                let value = assembler.push(part).unwrap();
                assert_eq!(value.is_some(), index + 1 == parts.len());
                if let Some(value) = value {
                    assert_eq!(serde_json::from_slice::<String>(&value).unwrap(), text);
                }
            }
        }
        let event = AgentEvent::Inventory { sessions: vec![] };
        assert_eq!(chunks(&event, 1).unwrap().len(), 1);
    }

    #[test]
    fn incomplete_replayed_skipped_and_interleaved_transfers_never_commit() {
        let parts = chunks(&"x".repeat(CHUNK_BYTES * 2), 1).unwrap();
        let mut assembler = InventoryAssembler::default();
        assert!(assembler.push(parts[1].clone()).is_err());
        assert!(assembler.push(parts[0].clone()).unwrap().is_none());
        assert!(assembler.in_progress());
        assert!(assembler.push(parts[0].clone()).is_err());
        assert!(assembler.push(parts[2].clone()).is_err());
        let mut other = parts[1].clone();
        other.transfer_id = "2".into();
        assert!(assembler.push(other).is_err());
        // Reconnect starts afresh. Neither the partial data nor its index survives.
        let mut reconnected = InventoryAssembler::default();
        for part in parts.iter().cloned() {
            reconnected.push(part).unwrap();
        }
        assert!(reconnected.push(parts[0].clone()).is_err());
        for part in chunks(&vec![1, 2, 3], 2).unwrap() {
            assert!(reconnected.push(part).unwrap().is_some());
        }
        let mut short = parts[0].clone();
        short.data_base64 = STANDARD.encode(b"short");
        assert!(InventoryAssembler::default().push(short).is_err());
    }
}
