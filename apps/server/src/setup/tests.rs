use super::*;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

const PASSWORD: &str = "setup-test-password";

async fn app(path: &std::path::Path) -> App {
    App {
        db: storage::open(path).await.unwrap(),
        origin: "https://setup.example".into(),
        allowed_origins: vec!["https://setup.example".into()],
        allow_any_origin: false,
        trusted_proxies: vec![],
        hub: Arc::new(Mutex::new(crate::Hub::default())),
        login_locks: Arc::new(std::array::from_fn(|_| Mutex::new(()))),
        login_policy: login_guard::Policy::default(),
        connections: crate::Connections::default(),
    }
}

async fn mint(app: &App, rotate: bool) -> (Issuance, String) {
    let mut output = Vec::new();
    let issued = issue_code_to(&app.db, rotate, &mut output).await.unwrap();
    let output = String::from_utf8(output).unwrap();
    let code = output
        .lines()
        .find_map(|line| line.strip_prefix("Setup code: "))
        .unwrap_or_default()
        .to_owned();
    (issued, code)
}

async fn headers(app: &App) -> HeaderMap {
    let response = auth::get_csrf(State(app.clone()), HeaderMap::new())
        .await
        .unwrap();
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let data = json_response(response).await;
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, cookie.parse().unwrap());
    headers.insert(header::ORIGIN, "https://setup.example".parse().unwrap());
    headers.insert(
        "x-csrf-token",
        data["data"]["csrfToken"].as_str().unwrap().parse().unwrap(),
    );
    headers
}

fn body(code: &str) -> Setup {
    Setup {
        username: "first-user".into(),
        password: PASSWORD.into(),
        password_confirmation: PASSWORD.into(),
        setup_code: code.into(),
    }
}

async fn attempt(app: &App, headers: HeaderMap, body: Setup) -> Response {
    attempt_from(app, "192.0.2.1:1234", headers, body).await
}

async fn attempt_from(app: &App, peer: &str, headers: HeaderMap, body: Setup) -> Response {
    match create(
        State(app.clone()),
        ConnectInfo(peer.parse::<SocketAddr>().unwrap()),
        headers,
        Json(body),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

#[tokio::test]
async fn http_setup_requires_loopback_peer_and_loopback_forwarded_client() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(&dir.path().join("server.db")).await;
    app.origin = "http://localhost".into();
    app.allowed_origins = vec![app.origin.clone()];
    let (_, code) = mint(&app, false).await;
    let mut headers = headers(&app).await;
    headers.insert(header::ORIGIN, "http://localhost".parse().unwrap());
    headers.insert("x-real-ip", "127.0.0.1".parse().unwrap());
    assert_error(
        attempt(&app, headers.clone(), body(&code)).await,
        StatusCode::FORBIDDEN,
        "ORIGIN_REJECTED",
    )
    .await;
    app.trusted_proxies = vec!["127.0.0.1".parse().unwrap()];
    headers.insert("x-real-ip", "203.0.113.5".parse().unwrap());
    assert_error(
        attempt_from(&app, "127.0.0.1:1234", headers.clone(), body(&code)).await,
        StatusCode::FORBIDDEN,
        "ORIGIN_REJECTED",
    )
    .await;
    headers.insert("x-real-ip", "::ffff:127.0.0.1".parse().unwrap());
    assert_eq!(
        attempt_from(&app, "[::ffff:127.0.0.1]:1234", headers, body(&code))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn setup_throttle_survives_restart_and_is_serialized_across_instances() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let first = app(&path).await;
    let second = app(&path).await;
    let (_, code) = mint(&first, false).await;
    let headers = headers(&first).await;
    let responses = futures_util::future::join_all((0..12).map(|n| {
        attempt(
            if n % 2 == 0 { &first } else { &second },
            headers.clone(),
            body(&"x".repeat(43)),
        )
    }))
    .await;
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.status() == StatusCode::BAD_REQUEST)
            .count(),
        4
    );
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.status() == StatusCode::TOO_MANY_REQUESTS)
            .count(),
        8
    );
    let deadline: i64 = sqlx::query_scalar("SELECT blocked_until FROM setup_ip_limits")
        .fetch_one(&first.db)
        .await
        .unwrap();
    first.db.close().await;
    second.db.close().await;
    let reopened = app(&path).await;
    assert_error(
        attempt(&reopened, headers.clone(), body(&code)).await,
        StatusCode::TOO_MANY_REQUESTS,
        "SETUP_THROTTLED",
    )
    .await;
    let persisted: i64 = sqlx::query_scalar("SELECT blocked_until FROM setup_ip_limits")
        .fetch_one(&reopened.db)
        .await
        .unwrap();
    assert_eq!(persisted, deadline);
    sqlx::query("UPDATE setup_ip_limits SET blocked_until = ?")
        .bind(storage::now())
        .execute(&reopened.db)
        .await
        .unwrap();
    assert_eq!(
        attempt(&reopened, headers, body(&code)).await.status(),
        StatusCode::OK
    );
}

async fn json_response(response: Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn assert_error(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(json_response(response).await["error"]["code"], code);
}

#[tokio::test]
async fn first_account_is_created_once_without_logging_in_and_never_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let first = app(&path).await;
    assert!(required(&first.db).await.unwrap());
    assert_eq!(
        status(State(first.clone())).await.unwrap().0,
        json!({"data":{"required":true}})
    );
    let (issued, code) = mint(&first, false).await;
    assert_eq!(issued, Issuance::Issued);
    assert_eq!(code.len(), 43);
    let secret_hash: String = sqlx::query_scalar("SELECT code_hash FROM first_account_setup")
        .fetch_one(&first.db)
        .await
        .unwrap();
    assert_eq!(secret_hash, storage::hash(&code));
    assert_ne!(secret_hash, code);
    let headers = headers(&first).await;
    let response = attempt(&first, headers.clone(), body(&code)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    assert_eq!(
        json_response(response).await,
        json!({"data":{"created":true}})
    );
    assert!(!required(&first.db).await.unwrap());
    let user = sqlx::query("SELECT username, password_hash FROM users")
        .fetch_one(&first.db)
        .await
        .unwrap();
    assert_eq!(user.get::<String, _>("username"), "first-user");
    use argon2::{
        password_hash::{PasswordHash, PasswordVerifier},
        Argon2,
    };
    let encoded: String = user.get("password_hash");
    assert!(Argon2::default()
        .verify_password(PASSWORD.as_bytes(), &PasswordHash::new(&encoded).unwrap())
        .is_ok());
    assert!(auth::identity(&first, &headers, false).await.is_err());
    let saved =
        sqlx::query("SELECT completed, code_hash, code_expires_at FROM first_account_setup")
            .fetch_one(&first.db)
            .await
            .unwrap();
    assert!(saved.get::<bool, _>("completed"));
    assert!(saved.get::<Option<String>, _>("code_hash").is_none());
    assert!(saved.get::<Option<i64>, _>("code_expires_at").is_none());
    assert_error(
        attempt(&first, headers.clone(), body(&code)).await,
        StatusCode::CONFLICT,
        "SETUP_CLOSED",
    )
    .await;
    sqlx::query("DELETE FROM users")
        .execute(&first.db)
        .await
        .unwrap();
    first.db.close().await;
    let reopened = app(&path).await;
    assert!(!required(&reopened.db).await.unwrap());
    assert_eq!(
        mint(&reopened, true).await,
        (Issuance::Closed, String::new())
    );
    assert_error(
        attempt(&reopened, headers, body(&code)).await,
        StatusCode::CONFLICT,
        "SETUP_CLOSED",
    )
    .await;
}

#[tokio::test]
async fn invalid_or_expired_codes_and_mismatch_do_not_consume_setup() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(&dir.path().join("server.db")).await;
    let (_, code) = mint(&app, false).await;
    let headers = headers(&app).await;
    assert_error(
        attempt(&app, headers.clone(), body(&"x".repeat(43))).await,
        StatusCode::BAD_REQUEST,
        "SETUP_CODE_INVALID",
    )
    .await;
    let mut mismatch = body(&code);
    mismatch.password_confirmation.push('!');
    assert_error(
        attempt(&app, headers.clone(), mismatch).await,
        StatusCode::BAD_REQUEST,
        "VALIDATION_ERROR",
    )
    .await;
    sqlx::query("UPDATE first_account_setup SET code_expires_at = ?")
        .bind(storage::now())
        .execute(&app.db)
        .await
        .unwrap();
    assert_error(
        attempt(&app, headers.clone(), body(&code)).await,
        StatusCode::BAD_REQUEST,
        "SETUP_CODE_INVALID",
    )
    .await;
    assert!(required(&app.db).await.unwrap());
    let (_, replacement) = mint(&app, false).await;
    assert_ne!(replacement, code);
    assert_eq!(
        attempt(&app, headers, body(&replacement)).await.status(),
        StatusCode::OK
    );
}

#[test]
fn credential_limits_are_utf8_bytes_and_never_normalize_passwords() {
    for username in ["", " ", " alice", "alice ", "a\nb", "a\0b"] {
        assert!(auth::validate_new_credentials(username, PASSWORD).is_err());
    }
    assert!(auth::validate_new_credentials(&"中".repeat(40), PASSWORD).is_ok());
    assert!(auth::validate_new_credentials(&"中".repeat(41), PASSWORD).is_err());
    assert!(auth::validate_new_credentials("alice", "12345678901").is_err());
    assert!(auth::validate_new_credentials("alice", "123456789012").is_ok());
    assert!(auth::validate_new_credentials("alice", &"x".repeat(1024)).is_ok());
    assert!(auth::validate_new_credentials("alice", &"x".repeat(1025)).is_err());
    assert!(auth::validate_new_credentials("alice", " 1234567890 ").is_ok());
}

#[tokio::test]
async fn setup_has_strict_origin_anonymous_csrf_and_independent_proxy_aware_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let mut app = app(&path).await;
    app.allow_any_origin = true;
    app.allowed_origins.push("http://192.168.1.2".into());
    app.trusted_proxies.push("192.0.2.1".parse().unwrap());
    let (_, code) = mint(&app, false).await;
    let mut headers = headers(&app).await;
    headers.insert("x-real-ip", "203.0.113.5".parse().unwrap());
    for origin in [
        None,
        Some("https://evil.example"),
        Some("http://192.168.1.2"),
    ] {
        let mut h = headers.clone();
        h.remove(header::ORIGIN);
        if let Some(origin) = origin {
            h.insert(header::ORIGIN, origin.parse().unwrap());
        }
        assert_error(
            attempt(&app, h, body(&code)).await,
            StatusCode::FORBIDDEN,
            "ORIGIN_REJECTED",
        )
        .await;
    }
    let mut duplicate = headers.clone();
    duplicate.append(header::ORIGIN, "http://localhost".parse().unwrap());
    assert_error(
        attempt(&app, duplicate, body(&code)).await,
        StatusCode::FORBIDDEN,
        "ORIGIN_REJECTED",
    )
    .await;
    let mut missing_csrf = headers.clone();
    missing_csrf.remove("x-csrf-token");
    assert_error(
        attempt(&app, missing_csrf, body(&code)).await,
        StatusCode::FORBIDDEN,
        "CSRF_INVALID",
    )
    .await;
    let bad = "x".repeat(43);
    for _ in 0..4 {
        assert_error(
            attempt(&app, headers.clone(), body(&bad)).await,
            StatusCode::BAD_REQUEST,
            "SETUP_CODE_INVALID",
        )
        .await;
    }
    let response = attempt(&app, headers.clone(), body(&bad)).await;
    assert_eq!(response.headers()[header::RETRY_AFTER], "30");
    assert_error(response, StatusCode::TOO_MANY_REQUESTS, "SETUP_THROTTLED").await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM login_ip_limits")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let ip: String = sqlx::query_scalar("SELECT ip FROM setup_ip_limits")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(ip, "203.0.113.5");
    // Trusted proxies have distinct buckets; arbitrary peers cannot spoof theirs.
    headers.insert("x-real-ip", "203.0.113.6".parse().unwrap());
    assert_error(
        attempt(&app, headers.clone(), body(&bad)).await,
        StatusCode::BAD_REQUEST,
        "SETUP_CODE_INVALID",
    )
    .await;
    headers.insert("x-real-ip", "203.0.113.5".parse().unwrap());
    assert_error(
        attempt(&app, headers, body(&code)).await,
        StatusCode::TOO_MANY_REQUESTS,
        "SETUP_THROTTLED",
    )
    .await;
}

#[tokio::test]
async fn concurrent_submission_and_issuance_are_serialized_across_independent_pools() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let first = app(&path).await;
    let second = app(&path).await;
    let (a, b) = tokio::join!(mint(&first, false), mint(&second, false));
    let code = if a.0 == Issuance::Issued {
        assert_eq!(b.0, Issuance::Pending);
        a.1
    } else {
        assert_eq!(a.0, Issuance::Pending);
        assert_eq!(b.0, Issuance::Issued);
        b.1
    };
    let headers = headers(&first).await;
    let (a, b) = tokio::join!(
        attempt(&first, headers.clone(), body(&code)),
        attempt(&second, headers, body(&code))
    );
    let statuses = [a.status(), b.status()];
    assert!(statuses.contains(&StatusCode::OK));
    assert!(statuses.contains(&StatusCode::CONFLICT));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&first.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn restart_preserves_pending_code_and_explicit_rotation_replaces_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.db");
    let first = app(&path).await;
    let (_, original) = mint(&first, false).await;
    first.db.close().await;
    let reopened = app(&path).await;
    assert_eq!(
        mint(&reopened, false).await,
        (Issuance::Pending, String::new())
    );
    let saved: String = sqlx::query_scalar("SELECT code_hash FROM first_account_setup")
        .fetch_one(&reopened.db)
        .await
        .unwrap();
    assert_eq!(saved, storage::hash(&original));
    let (_, replacement) = mint(&reopened, true).await;
    assert_ne!(original, replacement);
    let headers = headers(&reopened).await;
    assert_error(
        attempt(&reopened, headers.clone(), body(&original)).await,
        StatusCode::BAD_REQUEST,
        "SETUP_CODE_INVALID",
    )
    .await;
    assert_eq!(
        attempt(&reopened, headers, body(&replacement))
            .await
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn failed_terminal_write_rolls_back_rotation() {
    struct Unwritable;
    impl Write for Unwritable {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let app = app(&dir.path().join("server.db")).await;
    let (_, code) = mint(&app, false).await;
    assert!(issue_code_to(&app.db, true, &mut Unwritable).await.is_err());
    let hash: String = sqlx::query_scalar("SELECT code_hash FROM first_account_setup")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(hash, storage::hash(&code));
}

#[tokio::test]
async fn legacy_users_and_cli_creation_permanently_close_setup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let old = SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../../../../migrations/0001_initial.sql"))
        .execute(&old)
        .await
        .unwrap();
    sqlx::query("INSERT INTO users VALUES('old-id', 'old-user', 'test-only-hash')")
        .execute(&old)
        .await
        .unwrap();
    old.close().await;
    let upgraded = app(&path).await;
    assert!(!required(&upgraded.db).await.unwrap());
    sqlx::query("DELETE FROM users")
        .execute(&upgraded.db)
        .await
        .unwrap();
    upgraded.db.close().await;
    let reopened = app(&path).await;
    assert!(!required(&reopened.db).await.unwrap());

    let fresh = app(&dir.path().join("fresh.db")).await;
    let (_, code) = mint(&fresh, false).await;
    auth::create_admin(&fresh.db, "cli-user", PASSWORD)
        .await
        .unwrap();
    assert!(!required(&fresh.db).await.unwrap());
    assert_error(
        attempt(&fresh, headers(&fresh).await, body(&code)).await,
        StatusCode::CONFLICT,
        "SETUP_CLOSED",
    )
    .await;
    sqlx::query("DELETE FROM users")
        .execute(&fresh.db)
        .await
        .unwrap();
    assert!(!required(&fresh.db).await.unwrap());
}

#[tokio::test]
async fn failed_account_insert_does_not_consume_the_setup_code() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(&dir.path().join("server.db")).await;
    let (_, code) = mint(&app, false).await;
    sqlx::raw_sql("CREATE TRIGGER fail_user BEFORE INSERT ON users BEGIN SELECT RAISE(ABORT, 'test failure'); END;").execute(&app.db).await.unwrap();
    let response = attempt(&app, headers(&app).await, body(&code)).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(required(&app.db).await.unwrap());
    let hash: String = sqlx::query_scalar("SELECT code_hash FROM first_account_setup")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(hash, storage::hash(&code));
}
