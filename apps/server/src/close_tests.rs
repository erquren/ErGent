use super::*;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn fixture() -> (tempfile::TempDir, App, SessionInfo) {
    let tmp = tempfile::tempdir().unwrap();
    let app = App {
        db: crate::storage::open(&tmp.path().join("server.db"))
            .await
            .unwrap(),
        origin: "http://localhost".into(),
        allowed_origins: vec!["http://localhost".into()],
        allow_any_origin: false,
        trusted_proxies: vec![],
        hub: Arc::new(Mutex::new(crate::Hub::default())),
        login_locks: Arc::new(std::array::from_fn(|_| Mutex::new(()))),
    };
    sqlx::query("INSERT INTO users VALUES('owner','owner','unused')")
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines(id,owner_id,name,os,arch,credential_hash) VALUES('machine','owner','test','test','test','unused')").execute(&app.db).await.unwrap();
    sqlx::query("INSERT INTO operations VALUES('close','owner','machine','session','close','key','request','pending',NULL,0)").execute(&app.db).await.unwrap();
    let session = SessionInfo {
        id: "session".into(),
        machine_id: "machine".into(),
        agent_epoch: "epoch".into(),
        title: "test".into(),
        terminal_theme: None,
        cwd: "/".into(),
        shell: "sh".into(),
        lifecycle: "running".into(),
        cols: 80,
        rows: 24,
        exit_code: None,
        tool_status: None,
    };
    save_session(&app, &session).await.unwrap();
    (tmp, app, session)
}

async fn operation(app: &App) -> (String, serde_json::Value) {
    let row = sqlx::query("SELECT status,result FROM operations WHERE id='close'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    (
        row.get("status"),
        serde_json::from_str(&row.get::<String, _>("result")).unwrap(),
    )
}

async fn persisted(app: &App) -> SessionInfo {
    let data: String = sqlx::query_scalar("SELECT data FROM terminal_sessions WHERE id='session'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    serde_json::from_str(&data).unwrap()
}

#[tokio::test]
async fn close_acceptance_does_not_imply_exit() {
    let (_tmp, app, mut session) = fixture().await;
    session.lifecycle = "terminating".into();
    record_operation(
        &app,
        "machine",
        "epoch",
        "close",
        Some(session.clone()),
        None,
    )
    .await
    .unwrap();
    let (status, result) = operation(&app).await;
    assert_eq!(status, "accepted");
    assert_eq!(result["session"]["lifecycle"], "terminating");
    assert_eq!(persisted(&app).await.lifecycle, "terminating");
    let mut stale = session.clone();
    stale.lifecycle = "running".into();
    save_session(&app, &stale).await.unwrap();
    assert_eq!(persisted(&app).await.lifecycle, "terminating");
    session.lifecycle = "exited".into();
    session.exit_code = Some(0);
    save_session(&app, &session).await.unwrap();
    assert_eq!(persisted(&app).await, session);
    assert_eq!(
        operation(&app).await.0,
        "accepted",
        "operation records acceptance; lifecycle records exit"
    );
}

#[tokio::test]
async fn exit_before_acceptance_cannot_be_overwritten_or_revived() {
    let (_tmp, app, mut session) = fixture().await;
    session.lifecycle = "exited".into();
    session.exit_code = Some(9);
    save_session(&app, &session).await.unwrap();
    for lifecycle in ["terminating", "running", "exited"] {
        let mut stale = session.clone();
        stale.lifecycle = lifecycle.into();
        stale.exit_code = None;
        record_operation(&app, "machine", "epoch", "close", Some(stale), None)
            .await
            .unwrap();
        let (status, result) = operation(&app).await;
        assert_eq!(status, "accepted");
        assert_eq!(result["session"]["lifecycle"], "exited");
        assert_eq!(result["session"]["exitCode"], 9);
        assert_eq!(persisted(&app).await, session);
    }
}

#[tokio::test]
async fn failure_and_legacy_running_response_are_not_accepted() {
    let (_tmp, app, session) = fixture().await;
    record_operation(
        &app,
        "machine",
        "epoch",
        "close",
        None,
        Some("signal denied".into()),
    )
    .await
    .unwrap();
    assert_eq!(operation(&app).await.0, "failed");
    assert_eq!(persisted(&app).await.lifecycle, "running");
    record_operation(&app, "machine", "epoch", "close", Some(session), None)
        .await
        .unwrap();
    assert_eq!(operation(&app).await.0, "unknown");
    record_operation(&app, "machine", "epoch", "close", None, None)
        .await
        .unwrap();
    assert_eq!(operation(&app).await.0, "unknown");
}

#[tokio::test]
async fn missing_terminating_process_after_agent_restart_is_lost() {
    let (_tmp, app, mut session) = fixture().await;
    session.lifecycle = "terminating".into();
    save_session(&app, &session).await.unwrap();
    reconcile(&app, "machine", "new-epoch", vec![])
        .await
        .unwrap();
    assert_eq!(persisted(&app).await.lifecycle, "lost");
}

#[tokio::test]
async fn failed_wait_is_not_replaced_by_late_acceptance() {
    let (_tmp, app, mut session) = fixture().await;
    session.lifecycle = "lost".into();
    save_session(&app, &session).await.unwrap();
    let mut stale = session.clone();
    stale.lifecycle = "terminating".into();
    record_operation(&app, "machine", "epoch", "close", Some(stale), None)
        .await
        .unwrap();
    assert_eq!(operation(&app).await.0, "unknown");
    assert_eq!(persisted(&app).await, session);
}
