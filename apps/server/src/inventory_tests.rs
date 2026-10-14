use super::*;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn app(path: &std::path::Path) -> App {
    let app = App {
        db: crate::storage::open(path).await.unwrap(),
        origin: "http://localhost".into(),
        allowed_origins: vec!["http://localhost".into()],
        allow_any_origin: false,
        trusted_proxies: vec![],
        hub: Arc::new(Mutex::new(crate::Hub::default())),
        login_locks: Arc::new(std::array::from_fn(|_| Mutex::new(()))),
        login_policy: crate::login_guard::Policy::default(),
    };
    sqlx::query("INSERT INTO users VALUES('owner','owner','unused')")
        .execute(&app.db)
        .await
        .unwrap();
    for machine in ["machine", "other"] {
        sqlx::query("INSERT INTO machines(id,owner_id,name,os,arch,credential_hash) VALUES(?,'owner',?,'test','test',?)")
            .bind(machine).bind(machine).bind(machine).execute(&app.db).await.unwrap();
    }
    let (tx, _) = mpsc::channel(1);
    let (stop, _) = watch::channel(false);
    app.hub.lock().await.agents.insert(
        "machine".into(),
        AgentLink {
            connection: "connection".into(),
            owner: "owner".into(),
            tx,
            stop,
            ready: false,
        },
    );
    app
}
fn session(id: &str, epoch: &str, lifecycle: &str) -> SessionInfo {
    SessionInfo {
        id: id.into(),
        machine_id: "machine".into(),
        agent_epoch: epoch.into(),
        title: "历史中文🙂".repeat(10),
        terminal_theme: None,
        cwd: "/tmp".into(),
        shell: "/bin/sh".into(),
        lifecycle: lifecycle.into(),
        cols: 80,
        rows: 24,
        exit_code: None,
        output_truncated: false,
        tool_status: None,
    }
}

#[tokio::test]
async fn large_complete_inventory_preserves_history_and_marks_only_missing_live_sessions_lost() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp.path().join("db")).await;
    let old = session("old-exited", "old-epoch", "exited");
    save_session(&app, &old).await.unwrap();
    save_session(&app, &session("missing", "epoch", "running"))
        .await
        .unwrap();
    let sessions: Vec<_> = (0..1501)
        .map(|i| {
            session(
                &format!("s-{i}"),
                "epoch",
                if i == 0 { "running" } else { "exited" },
            )
        })
        .collect();
    let hello = AgentEvent::Hello {
        epoch: "epoch".into(),
        default_cwd: "/tmp".into(),
        sessions: sessions.clone(),
    };
    assert!(serde_json::to_vec(&hello).unwrap().len() > MAX_MESSAGE);
    let mut assembler = InventoryAssembler::default();
    let parts = chunks(&hello, 1).unwrap();
    for part in parts {
        let complete = assembler.push(part).unwrap();
        if let Some(bytes) = complete {
            let AgentEvent::Hello { sessions, .. } = serde_json::from_slice(&bytes).unwrap() else {
                panic!("wrong event");
            };
            reconcile(&app, "machine", "epoch", sessions, "connection")
                .await
                .unwrap();
        } else {
            // No reconciliation or lost-session inference can happen for a partial list.
            assert_eq!(app.hub.lock().await.sessions.len(), 2);
            assert_eq!(
                app.hub.lock().await.sessions["missing"].lifecycle,
                "running"
            );
        }
    }
    assert_eq!(app.hub.lock().await.sessions.len(), 1503);
    assert_eq!(app.hub.lock().await.sessions["missing"].lifecycle, "lost");
    assert_eq!(app.hub.lock().await.sessions["old-exited"], old);
    reconcile(&app, "machine", "epoch", sessions, "connection")
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM terminal_sessions")
            .fetch_one(&app.db)
            .await
            .unwrap(),
        1503
    );
    let persisted =
        sqlx::query_scalar::<_, String>("SELECT data FROM terminal_sessions WHERE id='old-exited'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert_eq!(
        serde_json::from_str::<SessionInfo>(&persisted).unwrap(),
        old
    );
}

#[tokio::test]
async fn invalid_or_replaced_inventory_cannot_partially_mutate_state() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp.path().join("db")).await;
    save_session(&app, &session("running", "epoch", "running"))
        .await
        .unwrap();
    let good = session("new", "epoch", "exited");
    let mut foreign = session("foreign", "epoch", "exited");
    foreign.machine_id = "other".into();
    save_session(&app, &foreign).await.unwrap();
    let collision = session("foreign", "epoch", "exited");
    for list in [
        vec![good.clone(), good.clone()],
        vec![good.clone(), foreign],
        vec![good.clone(), collision],
    ] {
        assert!(reconcile(&app, "machine", "epoch", list, "connection")
            .await
            .is_err());
        assert_eq!(app.hub.lock().await.sessions.len(), 2);
        assert_eq!(
            app.hub.lock().await.sessions["running"].lifecycle,
            "running"
        );
    }
    assert!(
        reconcile(&app, "machine", "epoch", vec![good], "replaced-connection")
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM terminal_sessions")
            .fetch_one(&app.db)
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn database_failure_rolls_back_full_inventory() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp.path().join("db")).await;
    save_session(&app, &session("running", "epoch", "running"))
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TRIGGER reject_test_session BEFORE INSERT ON terminal_sessions WHEN NEW.id='bad' BEGIN SELECT RAISE(ABORT, 'test failure'); END;")
        .execute(&app.db).await.unwrap();
    assert!(reconcile(
        &app,
        "machine",
        "epoch",
        vec![
            session("good", "epoch", "exited"),
            session("bad", "epoch", "exited")
        ],
        "connection"
    )
    .await
    .is_err());
    assert_eq!(app.hub.lock().await.sessions.len(), 1);
    assert_eq!(
        app.hub.lock().await.sessions["running"].lifecycle,
        "running"
    );
    let saved: String = sqlx::query_scalar("SELECT data FROM terminal_sessions WHERE id='running'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<SessionInfo>(&saved)
            .unwrap()
            .lifecycle,
        "running"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM terminal_sessions")
            .fetch_one(&app.db)
            .await
            .unwrap(),
        1
    );
}

#[test]
fn legacy_peers_fail_negotiation_explicitly() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "sec-websocket-protocol",
        "ergent.preview.v1".parse().unwrap(),
    );
    let error = protocol(&headers).unwrap_err();
    assert_eq!(error.status, StatusCode::UPGRADE_REQUIRED);
    assert_eq!(error.code, "PROTOCOL_UNSUPPORTED");
    headers.insert("sec-websocket-protocol", SUBPROTOCOL.parse().unwrap());
    assert!(protocol(&headers).is_ok());
}
#[tokio::test]
async fn websocket_large_hello_inventory_and_incomplete_reconnect() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message as ClientMessage},
    };
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp.path().join("db")).await;
    sqlx::query("UPDATE machines SET credential_hash=? WHERE id='machine'")
        .bind(hash("agent-token"))
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO web_sessions VALUES(?,'owner','csrf',?)")
        .bind(hash("browser-token"))
        .bind(now() + 600)
        .execute(&app.db)
        .await
        .unwrap();
    let router = axum::Router::new()
        .route("/agent", axum::routing::get(agent_upgrade))
        .route("/browser", axum::routing::get(browser_upgrade))
        .with_state(app.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let request = || {
        let mut request = format!("ws://{address}/agent")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("sec-websocket-protocol", SUBPROTOCOL.parse().unwrap());
        request
            .headers_mut()
            .insert("authorization", "Bearer agent-token".parse().unwrap());
        request
    };
    let sessions: Vec<_> = (0..1501)
        .map(|i| {
            session(
                &format!("ws-{i}"),
                "epoch",
                if i == 0 { "running" } else { "exited" },
            )
        })
        .collect();
    let hello = AgentEvent::Hello {
        epoch: "epoch".into(),
        default_cwd: "/tmp".into(),
        sessions,
    };
    let parts = chunks(&hello, 1).unwrap();
    assert!(parts.len() > 2);
    let (mut agent, _) = connect_async(request()).await.unwrap();
    agent
        .send(ClientMessage::Text(
            serde_json::to_string(&AgentEvent::InventoryChunk(parts[0].clone()))
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
    agent
        .send(ClientMessage::Ping(vec![1].into()))
        .await
        .unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), agent.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if matches!(message, ClientMessage::Pong(_)) {
            break;
        }
    }
    assert!(app.hub.lock().await.sessions.is_empty());
    assert!(!app.hub.lock().await.agents["machine"].ready);
    agent.close(None).await.unwrap();
    // A new socket starts its own transfer at one; the incomplete prefix was discarded.
    let (mut agent, _) = connect_async(request()).await.unwrap();
    for part in parts {
        let text = serde_json::to_string(&AgentEvent::InventoryChunk(part)).unwrap();
        assert!(text.len() <= MAX_MESSAGE);
        agent.send(ClientMessage::Text(text.into())).await.unwrap();
    }
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), agent.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let ClientMessage::Text(text) = message {
            assert!(matches!(
                serde_json::from_str::<AgentCommand>(&text).unwrap(),
                AgentCommand::Welcome
            ));
            break;
        }
    }
    assert_eq!(app.hub.lock().await.sessions.len(), 1501);
    let mut browser_request = format!("ws://{address}/browser")
        .into_client_request()
        .unwrap();
    browser_request
        .headers_mut()
        .insert("sec-websocket-protocol", SUBPROTOCOL.parse().unwrap());
    browser_request
        .headers_mut()
        .insert("origin", "http://localhost".parse().unwrap());
    browser_request
        .headers_mut()
        .insert("cookie", "ergent_session=browser-token".parse().unwrap());
    let (mut browser, _) = connect_async(browser_request).await.unwrap();
    let mut receiver = InventoryAssembler::default();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), browser.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let ClientMessage::Text(text) = message {
            assert!(text.len() <= MAX_MESSAGE);
            let Event::InventoryChunk(part) = serde_json::from_str(&text).unwrap() else {
                panic!("unchunked inventory");
            };
            if let Some(bytes) = receiver.push(part).unwrap() {
                let Event::Inventory { sessions, .. } = serde_json::from_slice(&bytes).unwrap()
                else {
                    panic!("wrong event");
                };
                assert_eq!(sessions.len(), 1501);
                break;
            }
        }
    }
    let replacement = chunks(&AgentEvent::Inventory { sessions: vec![] }, 2).unwrap();
    let replay = replacement[0].clone();
    for part in replacement {
        agent
            .send(ClientMessage::Text(
                serde_json::to_string(&AgentEvent::InventoryChunk(part))
                    .unwrap()
                    .into(),
            ))
            .await
            .unwrap();
    }
    // Read the browser's later completed snapshot; exited history remains, missing live is lost.
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), browser.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let ClientMessage::Text(text) = message {
            let Event::InventoryChunk(part) = serde_json::from_str(&text).unwrap() else {
                continue;
            };
            if let Some(bytes) = receiver.push(part).unwrap() {
                let Event::Inventory { sessions, .. } = serde_json::from_slice(&bytes).unwrap()
                else {
                    panic!("wrong event");
                };
                assert_eq!(sessions.len(), 1501);
                if sessions
                    .iter()
                    .any(|s| s.id == "ws-0" && s.lifecycle == "lost")
                {
                    break;
                }
            }
        }
    }
    agent
        .send(ClientMessage::Text(
            serde_json::to_string(&AgentEvent::InventoryChunk(replay))
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), agent.next())
            .await
            .unwrap()
        {
            None | Some(Err(_)) | Some(Ok(ClientMessage::Close(_))) => break,
            _ => {}
        }
    }
    assert_eq!(app.hub.lock().await.sessions.len(), 1501);
    let _ = browser.close(None).await;
    server.abort();
}

#[tokio::test]
async fn reconciliation_does_not_invert_operation_writer_and_hub_locks() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp.path().join("db")).await;
    // start_operation first inserts while holding a SQLite writer transaction,
    // then reads the hub. Reconcile must wait for this writer without holding hub.
    let mut operation = app.db.begin().await.unwrap();
    sqlx::query("INSERT INTO operations VALUES('op','owner','machine','new','create','key','hash','pending',NULL,0)")
        .execute(&mut *operation).await.unwrap();
    let concurrent = app.clone();
    let reconcile_task = tokio::spawn(async move {
        reconcile(
            &concurrent,
            "machine",
            "epoch",
            vec![session("new", "epoch", "running")],
            "connection",
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let hub = tokio::time::timeout(Duration::from_millis(500), app.hub.lock())
        .await
        .expect("inventory held hub while waiting for the operation's DB writer");
    assert!(hub.agents.contains_key("machine"));
    drop(hub);
    operation.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), reconcile_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(app.hub.lock().await.sessions.contains_key("new"));
}

#[tokio::test]
async fn truncated_output_marker_survives_inventory_and_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp.path().join("db")).await;
    let mut truncated = session("truncated", "epoch", "exited");
    truncated.output_truncated = true;
    reconcile(
        &app,
        "machine",
        "epoch",
        vec![truncated.clone()],
        "connection",
    )
    .await
    .unwrap();
    let saved: String =
        sqlx::query_scalar("SELECT data FROM terminal_sessions WHERE id='truncated'")
            .fetch_one(&app.db)
            .await
            .unwrap();
    assert!(
        serde_json::from_str::<SessionInfo>(&saved)
            .unwrap()
            .output_truncated
    );
    let Event::Inventory { sessions, .. } = app.inventory("owner").await.unwrap() else {
        panic!("wrong event");
    };
    assert_eq!(sessions, vec![truncated]);
}

#[tokio::test]
async fn stale_inventory_cannot_revive_sessions_or_erase_final_truncation() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(&tmp.path().join("db")).await;
    for state in ["exited", "lost", "terminating"] {
        save_session(&app, &session(state, "epoch", state))
            .await
            .unwrap();
    }
    let stale: Vec<_> = ["exited", "lost", "terminating"]
        .into_iter()
        .map(|id| {
            let mut update = session(id, "epoch", "running");
            update.output_truncated = true;
            update
        })
        .collect();
    reconcile(&app, "machine", "epoch", stale, "connection")
        .await
        .unwrap();
    for state in ["exited", "lost", "terminating"] {
        let hub = app.hub.lock().await;
        assert_eq!(hub.sessions[state].lifecycle, state);
        assert!(hub.sessions[state].output_truncated);
    }
    let stale = ["exited", "lost", "terminating"]
        .into_iter()
        .map(|id| session(id, "epoch", "running"))
        .collect();
    reconcile(&app, "machine", "epoch", stale, "connection")
        .await
        .unwrap();
    let saved: String = sqlx::query_scalar("SELECT data FROM terminal_sessions WHERE id='exited'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    let saved: SessionInfo = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved.lifecycle, "exited");
    assert!(saved.output_truncated);
}
