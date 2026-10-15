#![cfg(unix)]

use std::{
    net::{TcpListener, TcpStream},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn exits_cleanly(signal: &str) {
    let temp = tempfile::tempdir().unwrap();
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ergent-server"))
        .args([
            "--database",
            temp.path().join("server.db").to_str().unwrap(),
        ])
        .args(["run", "--listen", &address.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(address).is_err() {
        if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not become ready");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let sent = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "server did not drain cleanly: {status}");
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not stop after {signal}");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn sigterm_drains_and_exits_successfully() {
    exits_cleanly("-TERM");
}

#[test]
fn sigint_still_drains_and_exits_successfully() {
    exits_cleanly("-INT");
}

struct RunningServer(std::process::Child);
impl Drop for RunningServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn sigterm_waits_for_active_agent_database_cleanup() {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("server.db");
    let address = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let mut server = RunningServer(
        Command::new(env!("CARGO_BIN_EXE_ergent-server"))
            .args(["--database", database.to_str().unwrap()])
            .args(["run", "--listen", &address.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(address).is_err() {
        assert!(Instant::now() < deadline && server.0.try_wait().unwrap().is_none());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let db = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&database),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO users VALUES('owner','owner','unused')")
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines(id,owner_id,name,os,arch,credential_hash) VALUES('machine','owner','test','test','test',?)")
        .bind(format!("{:x}", Sha256::digest(b"shutdown-test-token")))
        .execute(&db).await.unwrap();
    // Add this after startup, which otherwise also marks stale operations unknown.
    sqlx::query("INSERT INTO operations VALUES('close','owner','machine','session','close','key','request','pending',NULL,0)")
        .execute(&db).await.unwrap();
    let mut request = format!("ws://{address}/api/v1/ws/agent")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "authorization",
        "Bearer shutdown-test-token".parse().unwrap(),
    );
    request.headers_mut().insert(
        "sec-websocket-protocol",
        ergent_protocol::SUBPROTOCOL.parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    // The initial ping proves the upgraded callback has registered in the Hub.
    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        message,
        tokio_tungstenite::tungstenite::Message::Ping(_)
    ));
    // Keep the agent's disconnect UPDATE pending, proving shutdown awaits cleanup
    // rather than dropping the runtime as soon as Axum's HTTP tasks finish.
    let transaction = db.begin_with("BEGIN IMMEDIATE").await.unwrap();
    assert!(Command::new("kill")
        .args(["-TERM", &server.0.id().to_string()])
        .status()
        .unwrap()
        .success());
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        server.0.try_wait().unwrap().is_none(),
        "shutdown skipped active-agent cleanup"
    );
    transaction.commit().await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            assert!(status.success(), "server did not drain cleanly: {status}");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "server did not finish connection cleanup"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let status: String = sqlx::query_scalar("SELECT status FROM operations WHERE id='close'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(status, "unknown");
}
