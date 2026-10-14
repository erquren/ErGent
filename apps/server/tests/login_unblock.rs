use sqlx::{sqlite::SqliteConnectOptions, Row, SqlitePool};
use std::process::Command;

#[tokio::test]
async fn cli_unlock_requires_audit_details_and_persists_an_inspectable_event() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("server.db");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ergent-server"))
            .arg("--database")
            .arg(&path)
            .args(args)
            .output()
            .unwrap()
    };
    let missing_details = run(&["login-unblock", "192.0.2.1"]);
    assert!(!missing_details.status.success());
    assert!(!path.exists());
    let output = run(&[
        "login-unblock",
        "::ffff:192.0.2.1",
        "--actor",
        "on-call",
        "--reason",
        "Shared NAT verified",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let db = SqlitePool::connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    let row = sqlx::query("SELECT * FROM login_guard_audit")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("ip"), "192.0.2.1");
    assert_eq!(row.get::<String, _>("actor"), "on-call");
    assert_eq!(row.get::<String, _>("reason"), "Shared NAT verified");
    assert_eq!(row.get::<i64, _>("previous_failures"), 0);
    assert!(String::from_utf8_lossy(&output.stdout).contains(&row.get::<String, _>("id")));
    let blank_reason = run(&[
        "login-unblock",
        "192.0.2.1",
        "--actor",
        "on-call",
        "--reason",
        " ",
    ]);
    assert!(!blank_reason.status.success());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM login_guard_audit")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
