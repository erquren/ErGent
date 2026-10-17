mod recovery;
mod screen;
mod tool_status;
#[cfg(windows)]
mod windows_process;
#[cfg(any(windows, test))]
mod windows_startup;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ergent_protocol::{
    validate_size, AgentEvent, BrowserCommand, CreateTerminal, Event, SessionInfo, ToolEvent,
};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
#[cfg(not(windows))]
type ProcessKiller = Box<dyn portable_pty::ChildKiller + Send + Sync>;
#[cfg(windows)]
type ProcessKiller = std::os::windows::io::OwnedHandle;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Write},
    path::Path,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};
use tokio::sync::broadcast;
use uuid::Uuid;

struct Lease {
    viewer: String,
    id: String,
    expires: Instant,
}
struct ViewerStream {
    id: String,
    seq: u64,
    revision: u64,
    snapshot: Option<(Vec<u8>, usize)>,
    completed: bool,
}
struct State {
    info: SessionInfo,
    parser: Option<vt100::Parser>,
    recovery: recovery::Recovery,
    streams: HashMap<String, ViewerStream>,
    final_seq: Option<u64>,
    final_snapshot: Option<(u64, Vec<u8>)>,
    output_deadline: Option<Instant>,
    viewers: HashSet<String>,
    lease: Option<Lease>,
    tool_events: VecDeque<String>,
    tool_turns: VecDeque<(String, String)>,
    inputs: VecDeque<(String, String, bool)>,
}
struct QueuedInput {
    viewer: String,
    lease_id: String,
    input_id: String,
    bytes: Vec<u8>,
}
struct Session {
    event_token: String,
    config: CreateTerminal,
    state: Arc<Mutex<State>>,
    resources: Mutex<Option<Arc<Resources>>>,
    close_lock: Mutex<()>,
}
struct Resources {
    input: mpsc::SyncSender<QueuedInput>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<ProcessKiller>,
    #[cfg(unix)]
    pid: Option<u32>,
}
impl Session {
    fn kill(&self) -> Result<()> {
        let resources = self.resources.lock().clone();
        let Some(resources) = resources else {
            return Ok(());
        };
        #[cfg(unix)]
        if let Some(pid) = resources.pid {
            // Only kill the dedicated PTY process group, never our own or an unrelated group.
            unsafe {
                if libc::getpgid(pid as i32) == pid as i32
                    && libc::killpg(pid as i32, libc::SIGKILL) == 0
                {
                    // The dedicated group (including its leader) was signalled.
                    // A redundant second kill can race reaping and report ESRCH.
                    return Ok(());
                }
            }
        }
        #[cfg(not(windows))]
        resources.killer.lock().kill()?;
        #[cfg(windows)]
        windows_process::terminate(&resources.killer.lock())?;
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
    pub fn create(&self, id: &str, mut config: CreateTerminal) -> Result<SessionInfo> {
        config.validate().map_err(|e| anyhow!(e))?;
        config.title = config.title.trim().to_owned();
        let mut sessions = self.sessions.lock();
        if let Some(session) = sessions.get(id) {
            if session.config != config {
                bail!("会话 ID 与已有配置冲突");
            }
            return Ok(session.state.lock().info.clone());
        }
        // History retains metadata and the original config for idempotency. Only live
        // runtimes consume capacity, including a process whose termination is pending.
        if sessions
            .values()
            .filter(|s| s.state.lock().parser.is_some())
            .count()
            >= 20
        {
            bail!("已达到会话上限");
        }
        if !Path::new(&config.cwd).is_absolute() || !Path::new(&config.cwd).is_dir() {
            bail!("工作目录必须是存在的绝对路径");
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
        #[cfg(not(windows))]
        let killer = child.clone_killer();
        #[cfg(windows)]
        let killer = windows_process::clone_handle(child.as_ref())?;
        let mut reader = pair.master.try_clone_reader()?;
        #[cfg(unix)]
        let poll_fd = {
            use std::os::fd::BorrowedFd;
            let fd = pair
                .master
                .as_raw_fd()
                .context("PTY has no readable descriptor")?;
            // The master remains owned here; the duplicate keeps poll's descriptor
            // valid even when the wait thread releases the master after shell exit.
            unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?
        };
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
            output_truncated: false,
            tool_status: None,
        };
        let state = Arc::new(Mutex::new(State {
            info: info.clone(),
            parser: Some(vt100::Parser::new(config.rows, config.cols, 0)),
            recovery: recovery::Recovery::new(),
            streams: HashMap::new(),
            final_seq: None,
            final_snapshot: None,
            output_deadline: None,
            viewers: HashSet::new(),
            lease: None,
            tool_events: VecDeque::new(),
            tool_turns: VecDeque::new(),
            inputs: VecDeque::new(),
        }));
        let (input, input_rx) = mpsc::sync_channel::<QueuedInput>(32);
        let session = Arc::new(Session {
            event_token,
            config,
            state: state.clone(),
            close_lock: Mutex::new(()),
            resources: Mutex::new(Some(Arc::new(Resources {
                input,
                writer: Arc::new(Mutex::new(writer)),
                master: Mutex::new(pair.master),
                killer: Mutex::new(killer),
                #[cfg(unix)]
                pid,
            }))),
        });
        let input_session = Arc::downgrade(&session);
        let input_events = self.events.clone();
        std::thread::spawn(move || {
            // One bounded FIFO and one writer per session. A non-reading child
            // cannot block the Agent socket loop, recovery pulls or heartbeats.
            while let Ok(input) = input_rx.recv() {
                let Some(session) = input_session.upgrade() else {
                    break;
                };
                let result = (|| -> Result<()> {
                    // Do not keep the PTY master alive while waiting on its input
                    // pipe. ConPTY teardown must be able to close the other end.
                    let writer = session
                        .resources
                        .lock()
                        .as_ref()
                        .map(|resources| resources.writer.clone())
                        .ok_or_else(|| anyhow!("terminal has exited"))?;
                    let mut writer = writer.lock();
                    check_lease(&session.state.lock(), &input.viewer, &input.lease_id)?;
                    writer.write_all(&input.bytes)?;
                    Ok(())
                })();
                let mut state = session.state.lock();
                if result.is_ok() {
                    if let Some((_, _, written)) = state
                        .inputs
                        .iter_mut()
                        .find(|(viewer, id, _)| viewer == &input.viewer && id == &input.input_id)
                    {
                        *written = true;
                    }
                    send(
                        &input_events,
                        &input.viewer,
                        Event::InputAck {
                            input_id: input.input_id,
                        },
                    );
                } else {
                    // A write error may mean partial delivery. Never retry it.
                    send(
                        &input_events,
                        &input.viewer,
                        Event::InputRejected {
                            session_id: state.info.id.clone(),
                            input_id: input.input_id,
                            message: "输入结果未确认，不会自动重发".into(),
                        },
                    );
                }
            }
        });
        sessions.insert(id.into(), session.clone());
        let read_state = state.clone();
        #[cfg(windows)]
        let startup_writer = Arc::downgrade(&session.resources.lock().as_ref().unwrap().writer);
        #[cfg(unix)]
        let exited_at = Arc::new(Mutex::new(None::<Instant>));
        #[cfg(unix)]
        let read_exited_at = exited_at.clone();
        let (reader_done, reader_finished) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            #[cfg(windows)]
            let mut cursor_handshake = windows_startup::CursorHandshake::default();
            let complete = loop {
                #[cfg(unix)]
                {
                    use std::os::fd::AsRawFd;
                    let exited_at = *read_exited_at.lock();
                    if exited_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(2)) {
                        break false;
                    }
                    let mut descriptor = libc::pollfd {
                        fd: poll_fd.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    // Drain queued tail output before releasing a reader whose slave may
                    // still be held by descendants. Quiet after shell exit is a boundary.
                    let ready = unsafe { libc::poll(&mut descriptor, 1, 100) };
                    if ready == 0 {
                        if exited_at.is_some() {
                            break true;
                        }
                        continue;
                    }
                    if ready < 0 {
                        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                        {
                            continue;
                        }
                        break false;
                    }
                }
                match reader.read(&mut buffer) {
                    Ok(0) => break true,
                    Ok(n) => {
                        let bytes = &buffer[..n];
                        #[cfg(windows)]
                        let startup_output;
                        #[cfg(windows)]
                        let bytes = if !cursor_handshake.is_complete() {
                            // Answer before locking state, independently of viewers
                            // and input leases. Keep no PTY resources alive on exit.
                            let Some(writer) = startup_writer.upgrade() else {
                                break false;
                            };
                            startup_output =
                                match cursor_handshake.process(bytes, &mut **writer.lock()) {
                                    Ok(output) => output,
                                    Err(_) => break false,
                                };
                            startup_output.as_slice()
                        } else {
                            bytes
                        };
                        let mut s = read_state.lock();
                        let State {
                            parser, recovery, ..
                        } = &mut *s;
                        let Some(parser) = parser.as_mut() else {
                            break false;
                        };
                        recovery.process(parser, bytes);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    #[cfg(unix)]
                    Err(e) if e.raw_os_error() == Some(libc::EIO) => break true,
                    Err(e) => {
                        break matches!(
                            e.kind(),
                            std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
                        )
                    }
                }
            };
            let _ = reader_done.send(complete);
        });
        let events = self.events.clone();
        std::thread::spawn(move || {
            let exit = child.wait();
            let exited = exit.is_ok();
            let drained = if exited {
                #[cfg(unix)]
                {
                    *exited_at.lock() = Some(Instant::now());
                }
                // In particular, ClosePseudoConsole must run without the reader's
                // state lock: it can synchronously wait for that reader to drain.
                let resources = session.resources.lock().take();
                drop(resources);
                reader_finished
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap_or(false)
            } else {
                false
            };
            record_exit(
                &state,
                &events,
                exit.map(|status| status.exit_code()),
                drained,
            );
        });
        Ok(info)
    }
    pub fn close(&self, id: &str) -> Result<SessionInfo> {
        let session = self.get(id)?;
        // Serialize repeated requests, but never keep the state lock while signaling.
        // The wait thread must be free to publish an exit before the signal returns.
        let _close = session.close_lock.lock();
        request_close(&session.state, &self.events, || session.kill())
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
            s.streams.remove(viewer);
            release_finished(&mut s);
            if s.lease.as_ref().is_some_and(|l| l.viewer == viewer) {
                s.lease = None;
            }
        }
    }
    pub fn reset_connections(&self) {
        for session in self.sessions.lock().values() {
            let mut s = session.state.lock();
            s.viewers.clear();
            s.streams.clear();
            release_finished(&mut s);
            s.lease = None;
        }
    }
    pub fn expire_leases(&self) {
        for session in self.sessions.lock().values() {
            let mut s = session.state.lock();
            if s.output_deadline
                .is_some_and(|deadline| deadline <= Instant::now())
            {
                for (viewer, stream) in &s.streams {
                    if !stream.completed {
                        send(
                            &self.events,
                            viewer,
                            Event::Ended {
                                session_id: s.info.id.clone(),
                                stream_id: stream.id.clone(),
                                epoch: s.info.agent_epoch.clone(),
                                seq: s.final_seq.unwrap().to_string(),
                                complete: false,
                                available: false,
                                truncated: true,
                            },
                        );
                    }
                }
                s.info.output_truncated = true;
                clear_finished(&mut s);
                let _ = self.events.send(AgentEvent::Session {
                    session: s.info.clone(),
                });
            }
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
                            stream_id: s
                                .streams
                                .get(viewer)
                                .map(|v| v.id.clone())
                                .unwrap_or_default(),
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
            BrowserCommand::Attach {
                request_id,
                epoch,
                seq,
                revision,
                cols,
                rows,
                ..
            } => {
                if request_id.len() > 64 {
                    bail!("无效同步 ID");
                }
                if s.viewers.len() >= 8 && !s.viewers.contains(viewer) {
                    bail!("观看者数量已达上限");
                }
                let cursor = seq.as_deref().and_then(|v| v.parse::<u64>().ok());
                let resume = epoch.as_ref() == Some(&s.info.agent_epoch)
                    && revision.as_deref().and_then(|v| v.parse::<u64>().ok())
                        == Some(s.recovery.revision)
                    && cols == Some(s.info.cols)
                    && rows == Some(s.info.rows)
                    && cursor.is_some_and(|v| s.recovery.contains(v));
                let (seq, snapshot) = if resume {
                    (cursor.unwrap(), None)
                } else {
                    let snapshot = if s.final_seq.is_some() {
                        s.final_snapshot.clone()
                    } else {
                        s.parser
                            .as_ref()
                            .and_then(|parser| s.recovery.snapshot(parser))
                    };
                    let Some((seq, bytes)) = snapshot else {
                        if s.final_seq.is_some() {
                            send(
                                &self.events,
                                viewer,
                                Event::Unavailable {
                                    session_id: s.info.id.clone(),
                                    request_id,
                                },
                            );
                            return Ok(());
                        }
                        bail!("等待完整终端序列后重新同步");
                    };
                    (seq, Some((bytes, 0)))
                };
                let stream_id = Uuid::new_v4().to_string();
                let revision = s.recovery.revision;
                // Snapshot/cursor and registration share the reader lock. A new
                // attachment invalidates the previous attachment's write lease.
                if s.lease.as_ref().is_some_and(|l| l.viewer == viewer) {
                    s.lease = None;
                }
                s.streams.insert(
                    viewer.into(),
                    ViewerStream {
                        id: stream_id.clone(),
                        seq,
                        revision,
                        snapshot,
                        completed: false,
                    },
                );
                s.viewers.insert(viewer.into());
                send(
                    &self.events,
                    viewer,
                    Event::Sync {
                        session_id: s.info.id.clone(),
                        request_id,
                        stream_id,
                        epoch: s.info.agent_epoch.clone(),
                        seq: seq.to_string(),
                        head: s.recovery.seq.to_string(),
                        revision: revision.to_string(),
                        cols: s.info.cols,
                        rows: s.info.rows,
                        reset: !resume,
                    },
                );
            }
            BrowserCommand::Pull { stream_id, seq, .. } => {
                let session_id = s.info.id.clone();
                let epoch = s.info.agent_epoch.clone();
                let Some(mut stream) = s.streams.remove(viewer) else {
                    bail!("请重新连接终端");
                };
                if stream.id != stream_id {
                    s.streams.insert(viewer.into(), stream);
                    return Ok(()); // An obsolete socket/attachment cannot alter the new stream.
                }
                let event = if seq.parse::<u64>().ok() != Some(stream.seq)
                    || stream.revision != s.recovery.revision
                {
                    Event::Resync {
                        session_id,
                        stream_id,
                        reason: "cursor_changed".into(),
                    }
                } else if s.final_seq == Some(stream.seq) && stream.snapshot.is_none() {
                    stream.completed = true;
                    Event::Ended {
                        session_id,
                        stream_id,
                        epoch,
                        seq: stream.seq.to_string(),
                        complete: true,
                        available: true,
                        truncated: s.info.output_truncated,
                    }
                } else if let Some((bytes, offset)) = &mut stream.snapshot {
                    let end = (*offset + recovery::CHUNK_BYTES).min(bytes.len());
                    let event = Event::Snapshot {
                        session_id,
                        stream_id,
                        epoch,
                        seq: stream.seq.to_string(),
                        index: (*offset / recovery::CHUNK_BYTES) as u32,
                        first: *offset == 0,
                        last: end == bytes.len(),
                        data_base64: STANDARD.encode(&bytes[*offset..end]),
                    };
                    *offset = end;
                    if end == bytes.len() {
                        stream.snapshot = None;
                    }
                    event
                } else if let Some(bytes) = s.recovery.read(stream.seq) {
                    let from = stream.seq.to_string();
                    stream.seq += bytes.len() as u64;
                    Event::Output {
                        session_id,
                        stream_id,
                        epoch,
                        from,
                        seq: stream.seq.to_string(),
                        data_base64: STANDARD.encode(bytes),
                    }
                } else if let Some(seq) = s.final_seq.filter(|_| s.final_snapshot.is_none()) {
                    Event::Ended {
                        session_id,
                        stream_id,
                        epoch,
                        seq: seq.to_string(),
                        complete: false,
                        available: false,
                        truncated: true,
                    }
                } else {
                    Event::Resync {
                        session_id,
                        stream_id,
                        reason: "replay_expired".into(),
                    }
                };
                s.streams.insert(viewer.into(), stream);
                release_finished(&mut s);
                send(&self.events, viewer, event);
            }
            BrowserCommand::Detach { .. } => {
                s.viewers.remove(viewer);
                s.streams.remove(viewer);
                release_finished(&mut s);
                if s.lease.as_ref().is_some_and(|l| l.viewer == viewer) {
                    s.lease = None;
                }
            }
            BrowserCommand::Claim {
                force, stream_id, ..
            } => {
                if !s.streams.get(viewer).is_some_and(|v| v.id == stream_id) {
                    bail!("终端连接已更新");
                }
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
                            stream_id: s
                                .streams
                                .get(viewer)
                                .map(|v| v.id.clone())
                                .unwrap_or_default(),
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
                            stream_id: s.streams.get(v).map(|v| v.id.clone()).unwrap_or_default(),
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
                // Queue accepted bytes once. The dedicated FIFO checks the lease
                // again immediately before writing, without holding the screen lock.
                let resources = session
                    .resources
                    .lock()
                    .clone()
                    .ok_or_else(|| anyhow!("terminal has exited"))?;
                if resources
                    .input
                    .try_send(QueuedInput {
                        viewer: viewer.into(),
                        lease_id,
                        input_id: input_id.clone(),
                        bytes,
                    })
                    .is_err()
                {
                    send(
                        &self.events,
                        viewer,
                        Event::InputRejected {
                            session_id: s.info.id.clone(),
                            input_id,
                            message: "输入队列已满，不会自动重发".into(),
                        },
                    );
                }
            }
            BrowserCommand::Resize {
                lease_id,
                cols,
                rows,
                ..
            } => {
                check_lease(&s, viewer, &lease_id)?;
                validate_size(cols, rows).map_err(|e| anyhow!(e))?;
                if s.info.cols != cols || s.info.rows != rows {
                    let resources = session
                        .resources
                        .lock()
                        .clone()
                        .ok_or_else(|| anyhow!("terminal has exited"))?;
                    resources.master.lock().resize(size(cols, rows))?;
                    s.parser
                        .as_mut()
                        .ok_or_else(|| anyhow!("terminal has exited"))?
                        .set_size(rows, cols);
                    s.recovery.resized();
                    s.info.cols = cols;
                    s.info.rows = rows;
                    for (viewer, stream) in &s.streams {
                        send(
                            &self.events,
                            viewer,
                            Event::Resync {
                                session_id: s.info.id.clone(),
                                stream_id: stream.id.clone(),
                                reason: "resized".into(),
                            },
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
impl Drop for TerminalManager {
    fn drop(&mut self) {
        for session in self.sessions.lock().values() {
            if session.resources.lock().is_some() {
                let _ = session.kill();
            }
        }
    }
}
fn record_exit(
    state: &Mutex<State>,
    events: &broadcast::Sender<AgentEvent>,
    result: std::io::Result<u32>,
    drained: bool,
) {
    let mut s = state.lock();
    let exited = result.is_ok();
    // A failed wait cannot prove exit; keep its runtime counted and killable.
    s.info.lifecycle = if exited { "exited" } else { "lost" }.into();
    s.info.exit_code = result.ok();
    s.info.output_truncated |= exited && !drained;
    if exited {
        s.final_seq = Some(s.recovery.seq);
        if !s.viewers.is_empty() {
            s.final_snapshot = s
                .parser
                .as_ref()
                .and_then(|parser| s.recovery.snapshot(parser));
            s.output_deadline = Some(Instant::now() + Duration::from_secs(60));
        }
        for (viewer, stream) in &s.streams {
            send(
                events,
                viewer,
                Event::Ended {
                    session_id: s.info.id.clone(),
                    stream_id: stream.id.clone(),
                    epoch: s.info.agent_epoch.clone(),
                    seq: s.recovery.seq.to_string(),
                    complete: false,
                    available: true,
                    truncated: s.info.output_truncated,
                },
            );
        }
        release_finished(&mut s);
        s.parser = None;
        s.inputs = VecDeque::new();
        s.tool_events = VecDeque::new();
        s.tool_turns = VecDeque::new();
    }
    revoke_lease(&mut s, events);
    let _ = events.send(AgentEvent::Session {
        session: s.info.clone(),
    });
}

// The caller serializes close requests. A successful signal is acceptance, not exit.
fn request_close(
    state: &Mutex<State>,
    events: &broadcast::Sender<AgentEvent>,
    kill: impl FnOnce() -> Result<()>,
) -> Result<SessionInfo> {
    {
        let s = state.lock();
        if matches!(s.info.lifecycle.as_str(), "terminating" | "exited") {
            return Ok(s.info.clone());
        }
        if s.info.lifecycle != "running" {
            bail!("无法确认终端进程状态");
        }
    }
    let result = kill();
    let mut s = state.lock();
    if s.info.lifecycle != "running" {
        // The wait result wins, including natural exit or loss of process status.
        return Ok(s.info.clone());
    }
    result?;
    s.info.lifecycle = "terminating".into();
    revoke_lease(&mut s, events);
    let _ = events.send(AgentEvent::Session {
        session: s.info.clone(),
    });
    Ok(s.info.clone())
}
fn revoke_lease(s: &mut State, events: &broadcast::Sender<AgentEvent>) {
    s.lease = None;
    for viewer in &s.viewers {
        send(
            events,
            viewer,
            Event::Lease {
                stream_id: s
                    .streams
                    .get(viewer)
                    .map(|v| v.id.clone())
                    .unwrap_or_default(),
                session_id: s.info.id.clone(),
                lease_id: None,
                writable: false,
            },
        );
    }
}
fn clear_finished(state: &mut State) {
    state.recovery.clear();
    state.final_snapshot = None;
    state.output_deadline = None;
    for stream in state.streams.values_mut() {
        stream.snapshot = None;
    }
}
fn release_finished(state: &mut State) {
    if state.final_seq.is_some() && state.streams.values().all(|stream| stream.completed) {
        clear_finished(state);
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
    fn running_state() -> Mutex<State> {
        Mutex::new(State {
            info: SessionInfo {
                id: "session".into(),
                machine_id: "machine".into(),
                agent_epoch: "epoch".into(),
                title: "Close test".into(),
                terminal_theme: None,
                cwd: "/".into(),
                shell: "sh".into(),
                lifecycle: "running".into(),
                cols: 80,
                rows: 24,
                exit_code: None,
                output_truncated: false,
                tool_status: None,
            },
            parser: Some(vt100::Parser::new(24, 80, 0)),
            recovery: recovery::Recovery::new(),
            streams: HashMap::new(),
            final_seq: None,
            final_snapshot: None,
            output_deadline: None,
            viewers: HashSet::from(["viewer".into()]),
            lease: Some(Lease {
                viewer: "viewer".into(),
                id: "lease".into(),
                expires: Instant::now() + Duration::from_secs(30),
            }),
            tool_events: VecDeque::new(),
            tool_turns: VecDeque::new(),
            inputs: VecDeque::new(),
        })
    }

    #[test]
    fn accepted_close_is_terminating_and_repeated_close_is_a_noop() {
        let state = running_state();
        let (events, mut receiver) = broadcast::channel(16);
        let accepted = request_close(&state, &events, || Ok(())).unwrap();
        assert_eq!(accepted.lifecycle, "terminating");
        assert_eq!(accepted.exit_code, None);
        assert!(state.lock().lease.is_none());
        assert!(matches!(
            receiver.try_recv().unwrap(),
            AgentEvent::Viewer {
                event: Event::Lease {
                    writable: false,
                    ..
                },
                ..
            }
        ));
        assert!(
            matches!(receiver.try_recv().unwrap(), AgentEvent::Session { session } if session.lifecycle == "terminating")
        );
        assert_eq!(
            request_close(&state, &events, || panic!("must not signal twice")).unwrap(),
            accepted
        );
        assert!(receiver.try_recv().is_err());
        assert!(check_lease(&state.lock(), "viewer", "lease").is_err());
    }

    #[test]
    fn failed_signal_leaves_running_and_can_be_retried() {
        let state = running_state();
        let (events, mut receiver) = broadcast::channel(16);
        assert!(request_close(&state, &events, || bail!("permission denied")).is_err());
        assert_eq!(state.lock().info.lifecycle, "running");
        assert!(state.lock().lease.is_some());
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            request_close(&state, &events, || Ok(())).unwrap().lifecycle,
            "terminating"
        );
    }

    #[test]
    fn authoritative_exit_wins_before_or_during_signal() {
        for signal_fails in [false, true] {
            let state = running_state();
            let (events, mut receiver) = broadcast::channel(16);
            let result = request_close(&state, &events, || {
                // Simulate wait completing while the platform signal is in flight.
                let mut s = state.lock();
                s.info.lifecycle = "exited".into();
                s.info.exit_code = Some(7);
                if signal_fails {
                    bail!("no such process");
                }
                Ok(())
            })
            .unwrap();
            assert_eq!(result.lifecycle, "exited");
            assert_eq!(result.exit_code, Some(7));
            assert!(
                receiver.try_recv().is_err(),
                "must not publish a stale terminating event"
            );
            assert_eq!(
                request_close(&state, &events, || panic!("already exited")).unwrap(),
                result
            );
        }
    }

    #[test]
    fn failed_wait_never_claims_exit_or_gets_overwritten_by_acceptance() {
        let state = running_state();
        let (events, mut receiver) = broadcast::channel(16);
        let result = request_close(&state, &events, || {
            record_exit(
                &state,
                &events,
                Err(std::io::Error::other("wait failed")),
                false,
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(result.lifecycle, "lost");
        assert_eq!(result.exit_code, None);
        while let Ok(event) = receiver.try_recv() {
            if let AgentEvent::Session { session } = event {
                assert_eq!(session.lifecycle, "lost");
            }
        }
        assert!(state.lock().lease.is_none());
    }

    #[test]
    fn close_real_process_returns_acceptance_then_wait_confirms_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = TerminalManager::new("machine".into(), "epoch".into());
        let mut events = manager.events();
        manager
            .create(
                "close",
                CreateTerminal {
                    title: "close".into(),
                    cwd: tmp.path().to_string_lossy().into(),
                    cols: 80,
                    rows: 24,
                },
            )
            .unwrap();
        assert!(matches!(
            manager.close("close").unwrap().lifecycle.as_str(),
            "terminating" | "exited"
        ));
        assert!(matches!(
            manager.close("close").unwrap().lifecycle.as_str(),
            "terminating" | "exited"
        ));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(AgentEvent::Session { session }) = events.try_recv() {
                if session.lifecycle == "exited" {
                    break;
                }
                assert_eq!(session.lifecycle, "terminating");
            }
            assert!(
                Instant::now() < deadline,
                "wait must publish authoritative exit"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(manager.close("close").unwrap().lifecycle, "exited");
    }

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
    fn reconnect_invalidates_streams_and_rechecks_queued_writes() {
        struct Capture(Arc<Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let manager = Arc::new(TerminalManager::new("machine".into(), "epoch".into()));
        manager
            .create(
                "s",
                CreateTerminal {
                    title: "test".into(),
                    cwd: tmp.path().to_string_lossy().into(),
                    cols: 80,
                    rows: 24,
                },
            )
            .unwrap();
        let attach = || BrowserCommand::Attach {
            session_id: "s".into(),
            request_id: "request".into(),
            epoch: None,
            seq: None,
            revision: None,
            cols: None,
            rows: None,
        };
        manager.handle("v", attach()).unwrap();
        let session = manager.get("s").unwrap();
        let old_stream = session.state.lock().streams["v"].id.clone();
        manager
            .handle(
                "v",
                BrowserCommand::Claim {
                    session_id: "s".into(),
                    stream_id: old_stream.clone(),
                    force: false,
                },
            )
            .unwrap();
        let old_lease = session.state.lock().lease.as_ref().unwrap().id.clone();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let resources = session.resources.lock().clone().unwrap();
        let _original_writer = std::mem::replace(
            &mut *resources.writer.lock(),
            Box::new(Capture(captured.clone())),
        );
        let retired = Arc::downgrade(&resources);
        let writer_handle = resources.writer.clone();
        drop(resources);
        let writer = writer_handle.lock();
        let mut events = manager.events();
        let pending = manager.clone();
        let (queued, received) = mpsc::channel();
        let write = std::thread::spawn(move || {
            queued
                .send(pending.handle(
                    "v",
                    BrowserCommand::Input {
                        session_id: "s".into(),
                        lease_id: old_lease,
                        input_id: "queued".into(),
                        data_base64: STANDARD.encode(b"must not be written"),
                    },
                ))
                .unwrap();
        });
        assert!(received
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .is_ok());
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.state.lock().inputs.is_empty() || Arc::strong_count(&writer_handle) < 3 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        manager.handle("v", attach()).unwrap();
        session.kill().unwrap();
        session.resources.lock().take();
        assert!(
            retired.upgrade().is_none(),
            "blocked input retained the PTY master"
        );
        drop(writer);
        write.join().unwrap();
        loop {
            if let Ok(AgentEvent::Viewer {
                event: Event::InputRejected { input_id, .. },
                ..
            }) = events.try_recv()
            {
                if input_id == "queued" {
                    break;
                }
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(captured.lock().is_empty());
        assert!(manager
            .handle(
                "v",
                BrowserCommand::Claim {
                    session_id: "s".into(),
                    stream_id: old_stream,
                    force: true
                }
            )
            .is_err());
        assert!(session.state.lock().lease.is_none());
    }

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
        // A Windows shell must start without a browser or an input lease to
        // answer ConPTY's inherited-cursor query.
        #[cfg(windows)]
        {
            let session = manager.get("s").unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while session
                .state
                .lock()
                .parser
                .as_ref()
                .unwrap()
                .screen()
                .contents()
                .is_empty()
            {
                assert!(Instant::now() < deadline, "ConPTY shell did not start");
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(!session
                .state
                .lock()
                .recovery
                .read(0)
                .unwrap()
                .windows(4)
                .any(|bytes| bytes == b"\x1b[6n"));
        }
        manager
            .handle(
                "v",
                BrowserCommand::Attach {
                    session_id: "s".into(),
                    request_id: "test".into(),
                    epoch: None,
                    seq: None,
                    revision: None,
                    cols: None,
                    rows: None,
                },
            )
            .unwrap();
        let stream_id = manager.get("s").unwrap().state.lock().streams["v"]
            .id
            .clone();
        manager
            .handle(
                "v",
                BrowserCommand::Claim {
                    stream_id,
                    session_id: "s".into(),
                    force: false,
                },
            )
            .unwrap();
        let session = manager.get("s").unwrap();
        let lease = session.state.lock().lease.as_ref().unwrap().id.clone();
        manager
            .handle(
                "v",
                BrowserCommand::Resize {
                    session_id: "s".into(),
                    lease_id: lease.clone(),
                    cols: 100,
                    rows: 30,
                },
            )
            .unwrap();
        #[cfg(unix)]
        let command = b"printf 'ERGENT_%s\\n' 'READY'\r".as_slice();
        #[cfg(windows)]
        let command = b"echo ERGENT_^READY\r".as_slice();
        let input = |lease_id: String| BrowserCommand::Input {
            session_id: "s".into(),
            lease_id,
            input_id: Uuid::new_v4().to_string(),
            data_base64: STANDARD.encode(command),
        };
        assert!(manager.handle("intruder", input(lease.clone())).is_err());
        manager.handle("v", input(lease)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if session
                .state
                .lock()
                .parser
                .as_ref()
                .unwrap()
                .screen()
                .contents()
                .contains("ERGENT_READY")
            {
                break;
            }
            assert!(Instant::now() < deadline, "PTY did not produce output");
            std::thread::sleep(Duration::from_millis(20));
        }
        manager.disconnect("v");
        assert_eq!(session.state.lock().info.lifecycle, "running");
        manager.close("s").unwrap();
    }
}

#[cfg(test)]
mod history_tests;
