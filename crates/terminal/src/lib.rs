mod screen;
mod tool_status;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ergent_protocol::{
    validate_size, AgentEvent, BrowserCommand, CreateTerminal, Event, SessionInfo, ToolEvent,
};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Write},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::broadcast;
use uuid::Uuid;

struct Lease {
    viewer: String,
    id: String,
    expires: Instant,
}
struct State {
    info: SessionInfo,
    parser: vt100::Parser,
    seq: u64,
    viewers: HashSet<String>,
    lease: Option<Lease>,
    tool_events: VecDeque<String>,
    tool_turns: VecDeque<(String, String)>,
    inputs: VecDeque<(String, String, bool)>,
}
struct Session {
    event_token: String,
    config: CreateTerminal,
    state: Arc<Mutex<State>>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg(unix)]
    pid: Option<u32>,
}
impl Session {
    fn kill(&self) -> Result<()> {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // Only kill the dedicated PTY process group, never our own or an unrelated group.
            unsafe {
                if libc::getpgid(pid as i32) == pid as i32 {
                    libc::killpg(pid as i32, libc::SIGKILL);
                }
            }
        }
        self.killer.lock().kill()?;
        Ok(())
    }
}
pub struct TerminalManager {
    event_url: Mutex<Option<String>>,
    machine_id: String,
    epoch: String,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    events: broadcast::Sender<AgentEvent>,
}
impl TerminalManager {
    pub fn new(machine_id: String, epoch: String) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            event_url: Mutex::new(None),
            machine_id,
            epoch,
            sessions: Mutex::new(HashMap::new()),
            events,
        }
    }
    pub fn set_event_url(&self, url: String) {
        *self.event_url.lock() = Some(url);
    }
    pub fn report_tool_event(&self, id: &str, token: &str, event: ToolEvent) -> Result<()> {
        event.validate().map_err(|e| anyhow!(e))?;
        let session = self.get(id)?;
        if token != session.event_token {
            bail!("invalid event credential");
        }
        let mut s = session.state.lock();
        if s.info.lifecycle != "running" {
            bail!("terminal is not running");
        }
        if s.tool_events.contains(&event.event_id) {
            return Ok(());
        }
        if let Some(turn) = &event.turn_id {
            let is_current = s.info.tool_status.as_ref().is_some_and(|old| {
                old.run_id == event.run_id && old.turn_id.as_ref() == Some(turn)
            });
            if !is_current && s.tool_turns.contains(&(event.run_id.clone(), turn.clone())) {
                return Ok(());
            }
        }
        if let Some(status) = tool_status::reduce(s.info.tool_status.as_deref(), &event) {
            if let Some(turn) = &event.turn_id {
                let key = (event.run_id.clone(), turn.clone());
                if !s.tool_turns.contains(&key) {
                    if s.tool_turns.len() == 128 {
                        s.tool_turns.pop_front();
                    }
                    s.tool_turns.push_back(key);
                }
            }
            s.info.tool_status = Some(Box::new(status));
            if s.tool_events.len() == 128 {
                s.tool_events.pop_front();
            }
            s.tool_events.push_back(event.event_id);
            let _ = self.events.send(AgentEvent::Session {
                session: s.info.clone(),
            });
        }
        Ok(())
    }
    pub fn events(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }
    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .values()
            .map(|s| s.state.lock().info.clone())
            .collect()
    }
    pub fn create(&self, id: &str, config: CreateTerminal) -> Result<SessionInfo> {
        config.validate().map_err(|e| anyhow!(e))?;
        if !Path::new(&config.cwd).is_absolute() || !Path::new(&config.cwd).is_dir() {
            bail!("工作目录必须是存在的绝对路径");
        }
        let mut sessions = self.sessions.lock();
        if let Some(session) = sessions.get(id) {
            if session.config != config {
                bail!("会话 ID 与已有配置冲突");
            }
            return Ok(session.state.lock().info.clone());
        }
        if sessions.len() >= 1000
            || sessions
                .values()
                .filter(|s| s.state.lock().info.lifecycle == "running")
                .count()
                >= 20
        {
            bail!("已达到会话上限");
        }
        let shell = default_shell();
        let pair = native_pty_system().openpty(size(config.cols, config.rows))?;
        let mut command = CommandBuilder::new(&shell);
        command.cwd(&config.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let event_token = format!("{}{}", Uuid::new_v4(), Uuid::new_v4());
        if let Some(url) = self.event_url.lock().as_ref() {
            command.env("ERGENT_EVENTS_URL", url);
            command.env("ERGENT_TERMINAL_ID", id);
            command.env("ERGENT_EVENT_TOKEN", &event_token);
            if let Ok(exe) = std::env::current_exe() {
                command.env("ERGENT_AGENT_BIN", exe);
            }
        }
        #[cfg(unix)]
        command.arg("-i");
        let mut child = pair
            .slave
            .spawn_command(command)
            .context("无法启动 Shell")?;
        drop(pair.slave);
        #[cfg(unix)]
        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let info = SessionInfo {
            id: id.into(),
            machine_id: self.machine_id.clone(),
            agent_epoch: self.epoch.clone(),
            title: config.title.clone(),
            terminal_theme: None,
            cwd: config.cwd.clone(),
            shell,
            lifecycle: "running".into(),
            cols: config.cols,
            rows: config.rows,
            exit_code: None,
            tool_status: None,
        };
        let state = Arc::new(Mutex::new(State {
            info: info.clone(),
            parser: vt100::Parser::new(config.rows, config.cols, 0),
            seq: 0,
            viewers: HashSet::new(),
            lease: None,
            tool_events: VecDeque::new(),
            tool_turns: VecDeque::new(),
            inputs: VecDeque::new(),
        }));
        let session = Arc::new(Session {
            event_token,
            config,
            state: state.clone(),
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            #[cfg(unix)]
            pid,
        });
        sessions.insert(id.into(), session);
        let events = self.events.clone();
        let read_state = state.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut s = read_state.lock();
                        s.parser.process(&buffer[..n]);
                        s.seq += 1;
                        let data = STANDARD.encode(&buffer[..n]);
                        for viewer in &s.viewers {
                            let _ = events.send(AgentEvent::Viewer {
                                viewer_id: viewer.clone(),
                                event: Event::Output {
                                    session_id: s.info.id.clone(),
                                    seq: s.seq.to_string(),
                                    data_base64: data.clone(),
                                },
                            });
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
        let events = self.events.clone();
        std::thread::spawn(move || {
            let exit = child.wait();
            let mut s = state.lock();
            s.info.lifecycle = "exited".into();
            s.info.exit_code = exit.ok().map(|v| v.exit_code());
            s.lease = None;
            for viewer in &s.viewers {
                send(
                    &events,
                    viewer,
                    Event::Lease {
                        session_id: s.info.id.clone(),
                        lease_id: None,
                        writable: false,
                    },
                );
            }
            let _ = events.send(AgentEvent::Session {
                session: s.info.clone(),
            });
        });
        Ok(info)
    }
    pub fn close(&self, id: &str) -> Result<SessionInfo> {
        let session = self.get(id)?;
        if session.state.lock().info.lifecycle == "running" {
            session.kill()?;
        }
        // The wait thread is authoritative about exit. Caller may see running until it finishes.
        let info = session.state.lock().info.clone();
        Ok(info)
    }
    fn get(&self, id: &str) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("会话不存在"))
    }
    pub fn disconnect(&self, viewer: &str) {
        for session in self.sessions.lock().values() {
            let mut s = session.state.lock();
            s.viewers.remove(viewer);
            if s.lease.as_ref().is_some_and(|l| l.viewer == viewer) {
                s.lease = None;
            }
        }
    }
    pub fn reset_connections(&self) {
        for session in self.sessions.lock().values() {
            let mut s = session.state.lock();
            s.viewers.clear();
            s.lease = None;
        }
    }
    pub fn expire_leases(&self) {
        for session in self.sessions.lock().values() {
            let mut s = session.state.lock();
            if s.lease
                .as_ref()
                .is_some_and(|l| l.expires <= Instant::now())
            {
                s.lease = None;
                for viewer in &s.viewers {
                    send(
                        &self.events,
                        viewer,
                        Event::Lease {
                            session_id: s.info.id.clone(),
                            lease_id: None,
                            writable: false,
                        },
                    );
                }
            }
        }
    }
    pub fn handle(&self, viewer: &str, command: BrowserCommand) -> Result<()> {
        let session = self.get(command.session_id())?;
        let mut s = session.state.lock();
        match command {
            BrowserCommand::Attach { .. } => {
                if s.viewers.len() >= 8 && !s.viewers.contains(viewer) {
                    bail!("观看者数量已达上限");
                }
                // Snapshot + subscription registration share the reader lock: no output can slip between them.
                let data = screen::snapshot(s.parser.screen());
                let chunks: Vec<_> = data.chunks(16 * 1024).collect();
                for (i, chunk) in chunks.iter().enumerate() {
                    send(
                        &self.events,
                        viewer,
                        Event::Snapshot {
                            session_id: s.info.id.clone(),
                            seq: s.seq.to_string(),
                            cols: s.info.cols,
                            rows: s.info.rows,
                            first: i == 0,
                            last: i + 1 == chunks.len(),
                            data_base64: STANDARD.encode(chunk),
                        },
                    );
                }
                s.viewers.insert(viewer.into());
            }
            BrowserCommand::Detach { .. } => {
                s.viewers.remove(viewer);
                if s.lease.as_ref().is_some_and(|l| l.viewer == viewer) {
                    s.lease = None;
                }
            }
            BrowserCommand::Claim { force, .. } => {
                if !s.viewers.contains(viewer) || s.info.lifecycle != "running" {
                    bail!("请先连接正在运行的终端");
                }
                if s.lease
                    .as_ref()
                    .is_some_and(|l| l.expires > Instant::now() && l.viewer != viewer)
                    && !force
                {
                    send(
                        &self.events,
                        viewer,
                        Event::Lease {
                            session_id: s.info.id.clone(),
                            lease_id: None,
                            writable: false,
                        },
                    );
                    return Ok(());
                }
                let id = Uuid::new_v4().to_string();
                s.lease = Some(Lease {
                    viewer: viewer.into(),
                    id: id.clone(),
                    expires: Instant::now() + Duration::from_secs(30),
                });
                for v in &s.viewers {
                    send(
                        &self.events,
                        v,
                        Event::Lease {
                            session_id: s.info.id.clone(),
                            lease_id: (v == viewer).then(|| id.clone()),
                            writable: v == viewer,
                        },
                    );
                }
            }
            BrowserCommand::Renew { lease_id, .. } => {
                check_lease(&s, viewer, &lease_id)?;
                s.lease.as_mut().unwrap().expires = Instant::now() + Duration::from_secs(30);
            }
            BrowserCommand::Input {
                lease_id,
                input_id,
                data_base64,
                ..
            } => {
                check_lease(&s, viewer, &lease_id)?;
                if input_id.len() > 64 {
                    bail!("无效输入 ID");
                }
                let bytes = STANDARD.decode(data_base64)?;
                if bytes.len() > 16 * 1024 {
                    bail!("输入过大");
                }
                if let Some((_, _, written)) = s
                    .inputs
                    .iter()
                    .find(|(v, id, _)| v == viewer && id == &input_id)
                {
                    if !written {
                        bail!("先前输入结果未知，不会自动重发");
                    }
                    send(&self.events, viewer, Event::InputAck { input_id });
                    return Ok(());
                } else {
                    if s.inputs.len() == 256 {
                        s.inputs.pop_front();
                    }
                    s.inputs.push_back((viewer.into(), input_id.clone(), false));
                }
                // A blocking PTY write must never hold the screen lock: the reader
                // needs it to keep draining output, including echoed pasted input.
                drop(s);
                session.writer.lock().write_all(&bytes)?;
                let mut s = session.state.lock();
                if let Some((_, _, written)) = s
                    .inputs
                    .iter_mut()
                    .find(|(v, id, _)| v == viewer && id == &input_id)
                {
                    *written = true;
                }
                send(&self.events, viewer, Event::InputAck { input_id });
            }
            BrowserCommand::Resize {
                lease_id,
                cols,
                rows,
                ..
            } => {
                check_lease(&s, viewer, &lease_id)?;
                validate_size(cols, rows).map_err(|e| anyhow!(e))?;
                session.master.lock().resize(size(cols, rows))?;
                s.parser.set_size(rows, cols);
                s.info.cols = cols;
                s.info.rows = rows;
            }
        }
        Ok(())
    }
}
impl Drop for TerminalManager {
    fn drop(&mut self) {
        for session in self.sessions.lock().values() {
            if session.state.lock().info.lifecycle == "running" {
                let _ = session.kill();
            }
        }
    }
}
fn send(events: &broadcast::Sender<AgentEvent>, viewer: &str, event: Event) {
    let _ = events.send(AgentEvent::Viewer {
        viewer_id: viewer.into(),
        event,
    });
}
fn check_lease(s: &State, viewer: &str, id: &str) -> Result<()> {
    if s.info.lifecycle != "running"
        || !s
            .lease
            .as_ref()
            .is_some_and(|l| l.viewer == viewer && l.id == id && l.expires > Instant::now())
    {
        bail!("写入权已过期或被其他窗口接管");
    }
    Ok(())
}
fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}
fn default_shell() -> String {
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL")
            .ok()
            .filter(|p| Path::new(p).is_file())
            .unwrap_or_else(|| "/bin/sh".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn screen_recovers_unicode_and_alternate_screen() {
        let mut source = vt100::Parser::new(24, 80, 100);
        source.process("hello 中文\x1b[31mred\x1b[0m\x1b[?1049hTUI".as_bytes());
        let mut restored = vt100::Parser::new(24, 80, 100);
        restored.process(&screen::snapshot(source.screen()));
        assert_eq!(restored.screen().contents(), source.screen().contents());
        assert_eq!(
            restored.screen().cursor_position(),
            source.screen().cursor_position()
        );
        assert!(restored.screen().alternate_screen());
        source.process(b"\x1b[?1049l");
        restored.process(b"\x1b[?1049l");
        assert_eq!(restored.screen().contents(), source.screen().contents());
        assert!(restored.screen().contents().contains("hello 中文"));
    }
    #[cfg(unix)]
    #[test]
    fn tool_events_are_terminal_scoped_and_deduplicated() {
        use ergent_protocol::{ToolEventKind as K, ToolState};
        let tmp = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new("machine".into(), "epoch".into());
        let config = CreateTerminal {
            title: "events".into(),
            cwd: tmp.path().to_string_lossy().into(),
            cols: 80,
            rows: 24,
        };
        manager.create("a", config.clone()).unwrap();
        manager.create("b", config).unwrap();
        let token = manager.get("a").unwrap().event_token.clone();
        let mut event = ToolEvent {
            version: 1,
            event_id: "first".into(),
            provider: "future-provider".into(),
            run_id: "run".into(),
            turn_id: None,
            kind: K::SessionStarted,
            occurred_at: 1,
        };
        assert!(manager
            .report_tool_event("a", "forged", event.clone())
            .is_err());
        assert!(manager
            .report_tool_event("b", &token, event.clone())
            .is_err());
        manager
            .report_tool_event("a", &token, event.clone())
            .unwrap();
        event.kind = K::SessionFailed; // Same event id cannot change state.
        manager
            .report_tool_event("a", &token, event.clone())
            .unwrap();
        assert_eq!(
            manager
                .get("a")
                .unwrap()
                .state
                .lock()
                .info
                .tool_status
                .as_ref()
                .unwrap()
                .state,
            ToolState::Idle
        );
        assert!(manager
            .get("b")
            .unwrap()
            .state
            .lock()
            .info
            .tool_status
            .is_none());
        manager.close("a").unwrap();
        manager.close("b").unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn actual_pty_survives_detach_and_create_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new("machine".into(), "epoch".into());
        let cfg = CreateTerminal {
            title: "test".into(),
            cwd: tmp.path().to_string_lossy().into(),
            cols: 80,
            rows: 24,
        };
        manager.create("s", cfg.clone()).unwrap();
        manager.create("s", cfg).unwrap();
        assert_eq!(manager.list().len(), 1);
        manager
            .handle(
                "v",
                BrowserCommand::Attach {
                    session_id: "s".into(),
                },
            )
            .unwrap();
        manager
            .handle(
                "v",
                BrowserCommand::Claim {
                    session_id: "s".into(),
                    force: false,
                },
            )
            .unwrap();
        let session = manager.get("s").unwrap();
        let lease = session.state.lock().lease.as_ref().unwrap().id.clone();
        let input = |lease_id: String| BrowserCommand::Input {
            session_id: "s".into(),
            lease_id,
            input_id: Uuid::new_v4().to_string(),
            data_base64: STANDARD.encode(b"printf 'ERGENT_%s\\n' 'READY'\r"),
        };
        assert!(manager.handle("intruder", input(lease.clone())).is_err());
        manager.handle("v", input(lease)).unwrap();
        manager.disconnect("v");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if session
                .state
                .lock()
                .parser
                .screen()
                .contents()
                .contains("ERGENT_READY")
            {
                break;
            }
            assert!(Instant::now() < deadline, "PTY did not produce output");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(session.state.lock().info.lifecycle, "running");
        manager.close("s").unwrap();
    }
}
