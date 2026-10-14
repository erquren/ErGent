use super::*;

fn wait_for_exit(manager: &TerminalManager, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while manager.get(id).unwrap().state.lock().info.lifecycle != "exited" {
        assert!(Instant::now() < deadline, "terminal did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn exited_runtime_is_reclaimed_but_history_and_idempotency_survive() {
    let tmp = tempfile::tempdir().unwrap();
    let manager = TerminalManager::new("machine".into(), "epoch".into());
    let config = CreateTerminal {
        title: "history".into(),
        cwd: tmp.path().to_string_lossy().into(),
        cols: 80,
        rows: 24,
    };
    manager.create("first", config.clone()).unwrap();
    let session = manager.get("first").unwrap();
    let resources = Arc::downgrade(session.resources.lock().as_ref().unwrap());
    manager.close("first").unwrap();
    wait_for_exit(&manager, "first");
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
    // Replayed creates remain idempotent even after the original directory disappears.
    drop(tmp);
    assert_eq!(manager.create("first", config.clone()).unwrap(), info);
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
    manager.close("original").unwrap();
    wait_for_exit(&manager, "original");
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
                state: Arc::new(Mutex::new(State {
                    info,
                    parser: None,
                    seq: 0,
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
    manager.close("new").unwrap();
    for i in 0..19 {
        manager.close(&format!("live-{i}")).unwrap();
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
