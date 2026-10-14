use super::*;

fn wait_for_exit(manager: &TerminalManager, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while manager.get(id).unwrap().state.lock().info.lifecycle != "exited" {
        if Instant::now() >= deadline {
            let session = manager.get(id).unwrap();
            let state = session.state.lock();
            panic!(
                "terminal did not exit: lifecycle={}, resources={}, screen={:?}",
                state.info.lifecycle,
                session.resources.lock().is_some(),
                state.parser.as_ref().map(|p| p.screen().contents())
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn exit_naturally(manager: &TerminalManager, id: &str) {
    let session = manager.get(id).unwrap();
    let resources = session.resources.lock().clone().unwrap();
    let mut writer = resources.writer.lock();
    // Headless ConPTY tests have no xterm to answer its inherited-cursor query.
    #[cfg(windows)]
    writer.write_all(b"\x1b[1;1R").unwrap();
    writer.write_all(b"exit\r").unwrap();
    drop(writer);
    drop(resources);
    wait_for_exit(manager, id);
}

#[test]
fn exited_runtime_is_reclaimed_but_history_and_idempotency_survive() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = TerminalManager::new("machine".into(), "epoch".into());
    let config = CreateTerminal {
        title: "  history  ".into(),
        cwd: tmp.path().to_string_lossy().into(),
        cols: 80,
        rows: 24,
    };
    manager.create("first", config.clone()).unwrap();
    let session = manager.get("first").unwrap();
    let resources = Arc::downgrade(session.resources.lock().as_ref().unwrap());
    exit_naturally(&manager, "first");
    assert!(session.resources.lock().is_none());
    assert!(session.state.lock().parser.is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    while resources.upgrade().is_some() || Arc::strong_count(&session.state) != 1 {
        assert!(
            Instant::now() < deadline,
            "PTY reader/resources were retained after exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let info = session.state.lock().info.clone();
    assert_eq!(info.title, "history");
    // Replayed creates remain idempotent even after the original directory disappears.
    drop(tmp);
    assert_eq!(manager.create("first", config.clone()).unwrap(), info);
    let mut normalized = config.clone();
    normalized.title = "history".into();
    assert_eq!(manager.create("first", normalized).unwrap(), info);
    let mut conflicting = config;
    conflicting.title = "different".into();
    assert!(manager.create("first", conflicting).is_err());
}

#[test]
fn more_than_one_thousand_historical_sessions_do_not_consume_live_capacity() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = TerminalManager::new("machine".into(), "epoch".into());
    let config = CreateTerminal {
        title: "history".into(),
        cwd: tmp.path().to_string_lossy().into(),
        cols: 80,
        rows: 24,
    };
    manager.create("original", config.clone()).unwrap();
    exit_naturally(&manager, "original");
    let original = manager.list().pop().unwrap();
    // Real exited runtime above establishes the archival shape; populate 1500 records
    // without spawning thousands of OS shells on every supported CI platform.
    for i in 0..1500 {
        let mut info = original.clone();
        info.id = format!("history-{i}");
        manager.sessions.lock().insert(
            info.id.clone(),
            Arc::new(Session {
                event_token: String::new(),
                config: config.clone(),
                resources: Mutex::new(None),
                close_lock: Mutex::new(()),
                state: Arc::new(Mutex::new(State {
                    info,
                    parser: None,
                    recovery: recovery::Recovery::new(),
                    streams: HashMap::new(),
                    final_seq: Some(0),
                    final_snapshot: None,
                    output_deadline: None,
                    viewers: HashSet::new(),
                    lease: None,
                    tool_events: VecDeque::new(),
                    tool_turns: VecDeque::new(),
                    inputs: VecDeque::new(),
                })),
            }),
        );
    }
    manager.create("new", config.clone()).unwrap();
    assert_eq!(manager.list().len(), 1502);
    assert_eq!(
        manager
            .create("history-1001", config.clone())
            .unwrap()
            .lifecycle,
        "exited"
    );
    // Retain the concurrent-runtime cap regardless of historical record count.
    for i in 0..19 {
        manager
            .create(&format!("live-{i}"), config.clone())
            .unwrap();
    }
    assert!(manager.create("over-capacity", config).is_err());
    assert_eq!(manager.list().len(), 1521);
    exit_naturally(&manager, "new");
    for i in 0..19 {
        exit_naturally(&manager, &format!("live-{i}"));
    }
}

#[cfg(unix)]
#[test]
fn exited_shell_does_not_leave_a_reader_waiting_for_background_descendants() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = TerminalManager::new("machine".into(), "epoch".into());
    manager
        .create(
            "background",
            CreateTerminal {
                title: "background".into(),
                cwd: tmp.path().to_string_lossy().into(),
                cols: 80,
                rows: 24,
            },
        )
        .unwrap();
    let session = manager.get("background").unwrap();
    // The short-lived background child ignores HUP and inherits the PTY slave.
    // It self-terminates after three seconds, even if an assertion fails.
    session
        .resources
        .lock()
        .as_ref()
        .unwrap()
        .writer
        .lock()
        .write_all(b"sh -c 'trap \"\" HUP; sleep 3 &'\nexit\n")
        .unwrap();
    wait_for_exit(&manager, "background");
    let deadline = Instant::now() + Duration::from_secs(1);
    while Arc::strong_count(&session.state) != 1 {
        assert!(
            Instant::now() < deadline,
            "reader waited for a background descendant"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn natural_exit_publishes_large_tail_before_exit_and_runtime_reclamation() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = TerminalManager::new("machine".into(), "epoch".into());
    manager
        .create(
            "tail",
            CreateTerminal {
                title: "tail".into(),
                cwd: tmp.path().to_string_lossy().into(),
                cols: 80,
                rows: 24,
            },
        )
        .unwrap();
    let mut events = manager.events();
    manager
        .handle(
            "viewer",
            BrowserCommand::Attach {
                session_id: "tail".into(),
                request_id: "tail".into(),
                epoch: None,
                seq: None,
                revision: None,
                cols: None,
                rows: None,
            },
        )
        .unwrap();
    let session = manager.get("tail").unwrap();
    session
        .resources
        .lock()
        .as_ref()
        .unwrap()
        .writer
        .lock()
        .write_all(
            b"exec sh -c 'head -c 131072 /dev/zero | tr \"\\000\" x; printf FINAL_%s SENTINEL'\r",
        )
        .unwrap();
    // Pause this viewer entirely until after child exit and reader completion.
    // The final replay must survive PTY/runtime reclamation and a stalled renderer.
    wait_for_exit(&manager, "tail");
    assert!(session.resources.lock().is_none());
    assert!(session.state.lock().parser.is_none());
    assert!(session.state.lock().final_snapshot.is_some());
    let (stream, mut seq) = {
        let state = session.state.lock();
        let stream = &state.streams["viewer"];
        (stream.id.clone(), stream.seq)
    };
    // Discard notifications that preceded our deliberately delayed pulls.
    while events.try_recv().is_ok() {}
    let mut output = Vec::new();
    loop {
        manager
            .handle(
                "viewer",
                BrowserCommand::Pull {
                    session_id: "tail".into(),
                    stream_id: stream.clone(),
                    seq: seq.to_string(),
                },
            )
            .unwrap();
        let AgentEvent::Viewer { event, .. } = events.try_recv().unwrap() else {
            panic!("expected viewer response");
        };
        match event {
            Event::Snapshot { .. } => {}
            Event::Output {
                seq: next,
                data_base64,
                ..
            } => {
                output.extend(STANDARD.decode(data_base64).unwrap());
                seq = next.parse().unwrap();
                // Even after sending the last bytes, they remain until the NEXT
                // pull acknowledges xterm's completion callback.
                if seq == session.state.lock().final_seq.unwrap() {
                    assert!(session.state.lock().final_snapshot.is_some());
                }
            }
            Event::Ended {
                complete: true,
                available: true,
                ..
            } => break,
            other => panic!("unexpected recovery event: {other:?}"),
        }
    }
    assert!(output
        .strip_suffix(b"\r\n")
        .unwrap_or(&output)
        .ends_with(b"FINAL_SENTINEL"));
    assert!(output
        .windows(131072)
        .any(|bytes| bytes.iter().all(|b| *b == b'x')));
    assert!(session.state.lock().final_snapshot.is_none());
    assert!(!session.state.lock().recovery.contains(0));
    assert!(!session.state.lock().info.output_truncated);
}

#[test]
fn final_recovery_expires_visibly_without_retaining_runtime_capacity() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = TerminalManager::new("machine".into(), "epoch".into());
    let config = CreateTerminal {
        title: "retained".into(),
        cwd: tmp.path().to_string_lossy().into(),
        cols: 80,
        rows: 24,
    };
    manager.create("finished", config.clone()).unwrap();
    manager
        .handle(
            "viewer",
            BrowserCommand::Attach {
                session_id: "finished".into(),
                request_id: "retention".into(),
                epoch: None,
                seq: None,
                revision: None,
                cols: None,
                rows: None,
            },
        )
        .unwrap();
    exit_naturally(&manager, "finished");
    let session = manager.get("finished").unwrap();
    assert!(session.state.lock().final_snapshot.is_some());
    // The retained final surface must not occupy one of the 20 live PTY slots.
    for i in 0..20 {
        manager.create(&format!("new-{i}"), config.clone()).unwrap();
    }
    assert!(manager.create("over-capacity", config).is_err());
    let mut events = manager.events();
    session.state.lock().output_deadline = Some(Instant::now());
    manager.expire_leases();
    let mut unavailable = false;
    let mut persisted = false;
    while let Ok(event) = events.try_recv() {
        match event {
            AgentEvent::Viewer {
                event: Event::Ended {
                    available: false, ..
                },
                ..
            } => unavailable = true,
            AgentEvent::Session { session }
                if session.id == "finished" && session.output_truncated =>
            {
                persisted = true
            }
            _ => {}
        }
    }
    assert!(unavailable && persisted);
    assert!(session.state.lock().final_snapshot.is_none());
    assert!(session.resources.lock().is_none());
    for i in 0..20 {
        exit_naturally(&manager, &format!("new-{i}"));
    }
}
