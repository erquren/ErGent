//! Shared preview protocol. It is deliberately distinct from the planned v1 wire contract.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const SUBPROTOCOL: &str = "ergent.preview.v1";
pub const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub machine_id: String,
    pub agent_epoch: String,
    pub title: String,
    #[serde(default)]
    pub terminal_theme: Option<String>,
    pub cwd: String,
    pub shell: String,
    pub lifecycle: String,
    pub cols: u16,
    pub rows: u16,
    pub exit_code: Option<u32>,
    #[serde(default)]
    pub tool_status: Option<Box<ToolStatus>>,
}

/// Provider-neutral state carried with terminal metadata, not inferred from terminal text.
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Idle,
    Running,
    WaitingForApproval,
    Completed,
    Interrupted,
    Failed,
    Stopped,
}
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub provider: String,
    pub run_id: String,
    pub turn_id: Option<String>,
    pub state: ToolState,
    pub event_id: String,
    #[ts(type = "number")]
    pub occurred_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ToolEventKind {
    SessionStarted,
    TurnStarted,
    Activity,
    ApprovalRequested,
    TurnCompleted,
    TurnInterrupted,
    SessionStopped,
    SessionFailed,
}
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolEvent {
    pub version: u8,
    pub event_id: String,
    pub provider: String,
    pub run_id: String,
    pub turn_id: Option<String>,
    pub kind: ToolEventKind,
    #[ts(type = "number")]
    pub occurred_at: u64,
}
impl ToolEvent {
    pub fn validate(&self) -> Result<(), &'static str> {
        if (!matches!(
            self.kind,
            ToolEventKind::SessionStarted
                | ToolEventKind::SessionStopped
                | ToolEventKind::SessionFailed
        ) && self.turn_id.is_none())
            || self.version != 1
            || self.provider.is_empty()
            || self.provider.len() > 32
            || !self
                .provider
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
            || self.event_id.is_empty()
            || self.event_id.len() > 128
            || self.run_id.is_empty()
            || self.run_id.len() > 128
            || self
                .turn_id
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 128)
            || self.occurred_at > 9_007_199_254_740_991
        {
            return Err("invalid tool event");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MachineInfo {
    pub id: String,
    pub name: String,
    pub os: String,
    pub arch: String,
    pub online: bool,
    pub revoked: bool,
    pub default_cwd: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTerminal {
    pub title: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
}
impl CreateTerminal {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.title.trim().is_empty() || self.title.len() > 120 {
            return Err("终端名称应为 1–120 字节");
        }
        if self.cwd.is_empty() || self.cwd.len() > 4096 || self.cwd.contains('\0') {
            return Err("无效工作目录");
        }
        validate_size(self.cols, self.rows)
    }
}
pub fn validate_size(cols: u16, rows: u16) -> Result<(), &'static str> {
    if !(20..=500).contains(&cols) || !(5..=200).contains(&rows) {
        Err("终端尺寸超出范围")
    } else {
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "payload")]
#[serde(deny_unknown_fields)]
pub enum BrowserCommand {
    #[serde(rename = "terminal.attach")]
    Attach {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    #[serde(rename = "terminal.detach")]
    Detach {
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    #[serde(rename = "terminal.lease.claim")]
    Claim {
        #[serde(rename = "sessionId")]
        session_id: String,
        force: bool,
    },
    #[serde(rename = "terminal.lease.renew")]
    Renew {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "leaseId")]
        lease_id: String,
    },
    #[serde(rename = "terminal.input")]
    Input {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "leaseId")]
        lease_id: String,
        #[serde(rename = "inputId")]
        input_id: String,
        #[serde(rename = "dataBase64")]
        data_base64: String,
    },
    #[serde(rename = "terminal.resize")]
    Resize {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "leaseId")]
        lease_id: String,
        cols: u16,
        rows: u16,
    },
}
impl BrowserCommand {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Attach { session_id }
            | Self::Detach { session_id }
            | Self::Claim { session_id, .. }
            | Self::Renew { session_id, .. }
            | Self::Input { session_id, .. }
            | Self::Resize { session_id, .. } => session_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "payload")]
pub enum Event {
    #[serde(rename = "inventory.sync")]
    Inventory {
        machines: Vec<MachineInfo>,
        sessions: Vec<SessionInfo>,
    },
    #[serde(rename = "terminal.snapshot")]
    Snapshot {
        #[serde(rename = "sessionId")]
        session_id: String,
        seq: String,
        cols: u16,
        rows: u16,
        first: bool,
        last: bool,
        #[serde(rename = "dataBase64")]
        data_base64: String,
    },
    #[serde(rename = "terminal.output")]
    Output {
        #[serde(rename = "sessionId")]
        session_id: String,
        seq: String,
        #[serde(rename = "dataBase64")]
        data_base64: String,
    },
    #[serde(rename = "terminal.lease.changed")]
    Lease {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "leaseId")]
        lease_id: Option<String>,
        writable: bool,
    },
    #[serde(rename = "terminal.input.ack")]
    InputAck {
        #[serde(rename = "inputId")]
        input_id: String,
    },
    #[serde(rename = "session.updated")]
    Session { session: SessionInfo },
    #[serde(rename = "error")]
    Error { code: String, message: String },
}
impl Event {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self::Error {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum AgentCommand {
    Welcome,
    Create {
        operation_id: String,
        session_id: String,
        config: CreateTerminal,
    },
    Close {
        operation_id: String,
        session_id: String,
    },
    Browser {
        viewer_id: String,
        command: BrowserCommand,
    },
    ViewerGone {
        viewer_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum AgentEvent {
    Hello {
        epoch: String,
        default_cwd: String,
        sessions: Vec<SessionInfo>,
    },
    Inventory {
        sessions: Vec<SessionInfo>,
    },
    Operation {
        operation_id: String,
        session: Option<SessionInfo>,
        error: Option<String>,
    },
    Viewer {
        viewer_id: String,
        event: Event,
    },
    Session {
        session: SessionInfo,
    },
}

/// Planned v1 binary codec, separate from the JSON preview transport.
pub mod frame {
    pub const HEADER: usize = 20;
    #[derive(Debug, PartialEq)]
    pub struct Frame<'a> {
        pub kind: u8,
        pub stream_id: u32,
        pub seq: u64,
        pub payload: &'a [u8],
    }
    pub fn decode(data: &[u8]) -> Result<Frame<'_>, &'static str> {
        if data.len() < HEADER || data.len() > super::MAX_MESSAGE {
            return Err("frame size");
        }
        if data[0] != 1 || !matches!(data[1], 1 | 2) || data[2..4] != [0, 0] {
            return Err("frame header");
        }
        let len = u32::from_be_bytes(data[16..20].try_into().unwrap()) as usize;
        if len != data.len() - HEADER {
            return Err("payload length");
        }
        Ok(Frame {
            kind: data[1],
            stream_id: u32::from_be_bytes(data[4..8].try_into().unwrap()),
            seq: u64::from_be_bytes(data[8..16].try_into().unwrap()),
            payload: &data[20..],
        })
    }
    pub fn encode(frame: Frame<'_>) -> Result<Vec<u8>, &'static str> {
        if !matches!(frame.kind, 1 | 2) || frame.payload.len() > super::MAX_MESSAGE - HEADER {
            return Err("invalid frame");
        }
        let mut out = vec![1, frame.kind, 0, 0];
        out.extend(frame.stream_id.to_be_bytes());
        out.extend(frame.seq.to_be_bytes());
        out.extend((frame.payload.len() as u32).to_be_bytes());
        out.extend(frame.payload);
        Ok(out)
    }
}

pub fn typescript() -> String {
    format!("// Generated by cargo run -p ergent-protocol --example export. Do not edit.\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        ToolState::decl(), ToolStatus::decl(), ToolEventKind::decl(), ToolEvent::decl(), SessionInfo::decl(), MachineInfo::decl(), CreateTerminal::decl(), BrowserCommand::decl(), Event::decl())
        .replace("type ", "export type ")
}

#[cfg(test)]
mod tests {
    use super::frame::*;
    #[test]
    fn frames_preserve_bytes_and_large_sequences() {
        let raw = [0, 0xff, 0xe4, 0xb8];
        let frame = Frame {
            kind: 1,
            stream_id: 42,
            seq: u64::MAX,
            payload: &raw,
        };
        let encoded = encode(frame).unwrap();
        assert_eq!(decode(&encoded).unwrap().seq, u64::MAX);
        assert_eq!(decode(&encoded).unwrap().payload, raw);
        for end in 0..encoded.len() {
            assert!(decode(&encoded[..end]).is_err());
        }
        let mut bad = encoded;
        bad[2] = 1;
        assert!(decode(&bad).is_err());
    }
}
