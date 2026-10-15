use crate::{
    storage::{hash, now, secret},
    ApiError, App,
};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
pub struct Identity {
    pub user: String,
    pub token_hash: String,
}
pub fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|v| v.trim().strip_prefix("ergent_session=").map(str::to_owned))
}
pub fn origin(app: &App, headers: &HeaderMap) -> Result<(), ApiError> {
    if !crate::origin::request_allowed(
        app.allow_any_origin,
        headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()),
        &app.allowed_origins,
    ) {
        tracing::warn!(origin=?headers.get(header::ORIGIN), allowed=?app.allowed_origins, "browser origin rejected");
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "ORIGIN_REJECTED",
            "请求来源不匹配",
        ));
    }
    Ok(())
}
pub async fn identity(app: &App, headers: &HeaderMap, write: bool) -> Result<Identity, ApiError> {
    let token = cookie(headers).ok_or_else(ApiError::unauthorized)?;
    let token_hash = hash(&token);
    let row =
        sqlx::query("SELECT user_id,csrf FROM web_sessions WHERE token_hash=? AND expires_at>?")
            .bind(&token_hash)
            .bind(now())
            .fetch_optional(&app.db)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
    let user: Option<String> = row.get("user_id");
    if write {
        origin(app, headers)?;
        csrf(headers, row.get("csrf"))?;
    }
    Ok(Identity {
        user: user.ok_or_else(ApiError::unauthorized)?,
        token_hash,
    })
}
pub(crate) fn csrf(headers: &HeaderMap, expected: &str) -> Result<(), ApiError> {
    if headers.get("x-csrf-token").and_then(|v| v.to_str().ok()) != Some(expected) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "CSRF_INVALID",
            "请刷新登录状态",
        ));
    }
    Ok(())
}
fn set_cookie(app: &App, token: &str, age: u32) -> String {
    format!(
        "ergent_session={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={age}{}",
        if app.origin.starts_with("https:") {
            "; Secure"
        } else {
            ""
        }
    )
}
pub async fn get_csrf(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    sqlx::query("DELETE FROM web_sessions WHERE expires_at<=?")
        .bind(now())
        .execute(&app.db)
        .await?;
    if let Some(token) = cookie(&headers) {
        if let Some(row) =
            sqlx::query("SELECT csrf FROM web_sessions WHERE token_hash=? AND expires_at>?")
                .bind(hash(&token))
                .bind(now())
                .fetch_optional(&app.db)
                .await?
        {
            return Ok(
                Json(json!({"data":{"csrfToken":row.get::<String,_>("csrf")}})).into_response(),
            );
        }
    }
    let token = secret();
    let csrf = secret();
    sqlx::query("INSERT INTO web_sessions(token_hash,csrf,expires_at) VALUES(?,?,?)")
        .bind(hash(&token))
        .bind(&csrf)
        .bind(now() + 600)
        .execute(&app.db)
        .await?;
    Ok((
        [(header::SET_COOKIE, set_cookie(&app, &token, 600))],
        Json(json!({"data":{"csrfToken":csrf}})),
    )
        .into_response())
}
#[derive(Deserialize)]
pub struct Login {
    username: String,
    password: String,
}
pub async fn login(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Login>,
) -> Result<Response, ApiError> {
    origin(&app, &headers)?;
    let old = cookie(&headers).ok_or_else(ApiError::unauthorized)?;
    let row = sqlx::query("SELECT csrf FROM web_sessions WHERE token_hash=? AND expires_at>?")
        .bind(hash(&old))
        .bind(now())
        .fetch_optional(&app.db)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    csrf(&headers, row.get("csrf"))?;
    let ip = crate::login_guard::client_ip(peer.ip(), &headers, &app.trusted_proxies)?;
    // Serialize attempts from the same IP through verification and session issuance.
    // Fixed-size striped locks bound memory use under arbitrary source addresses.
    let _guard = app.login_locks[crate::login_guard::stripe(ip)].lock().await;
    let key = ip.to_string();
    // SQLite also serializes admission with another server process or CLI unlock.
    // Keep the transaction through verification/session issuance so a successful
    // request cannot erase a later failure or resurrect state after an unlock.
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let attempt_at = now();
    sqlx::query("DELETE FROM login_ip_limits WHERE window_started_at<=? AND blocked_until<=?")
        .bind(attempt_at.saturating_sub(i64::from(app.login_policy.window_seconds)))
        .bind(attempt_at)
        .execute(&mut *tx)
        .await?;
    let previous = crate::login_guard::FailureState::load(&mut tx, &key).await?;
    if let Some(state) = previous {
        let retry_after = state.retry_after(attempt_at);
        if retry_after > 0 {
            tx.commit().await?;
            return Ok(crate::login_guard::throttled(retry_after));
        }
    }
    let user = if body.password.len() <= 1024 && body.username.len() <= 120 {
        sqlx::query("SELECT id,password_hash FROM users WHERE username=?")
            .bind(&body.username)
            .fetch_optional(&mut *tx)
            .await?
    } else {
        None
    };
    let verified = if let Some(ref user) = user {
        let password_hash: String = user.get("password_hash");
        tokio::task::spawn_blocking(move || {
            PasswordHash::new(&password_hash).ok().is_some_and(|h| {
                Argon2::default()
                    .verify_password(body.password.as_bytes(), &h)
                    .is_ok()
            })
        })
        .await
        .map_err(|_| ApiError::internal())?
    } else {
        false
    };
    if !verified {
        let failed_at = now();
        let state =
            crate::login_guard::FailureState::failed(previous, &app.login_policy, failed_at);
        state.save(&mut tx, &key).await?;
        tx.commit().await?;
        let retry_after = state.retry_after(failed_at);
        if retry_after > 0 {
            return Ok(crate::login_guard::throttled(retry_after));
        }
        return Err(crate::login_guard::invalid_login());
    }
    sqlx::query("DELETE FROM login_ip_limits WHERE ip=?")
        .bind(&key)
        .execute(&mut *tx)
        .await?;
    let id: String = user.ok_or_else(ApiError::internal)?.get("id");
    let token = secret();
    let new_csrf = secret();
    sqlx::query("DELETE FROM web_sessions WHERE token_hash=?")
        .bind(hash(&old))
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO web_sessions VALUES(?,?,?,?)")
        .bind(hash(&token))
        .bind(&id)
        .bind(&new_csrf)
        .bind(now() + 43200)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    app.close_auth(&hash(&old)).await;
    Ok((
        [(header::SET_COOKIE, set_cookie(&app, &token, 43200))],
        Json(json!({"data":{"user":{"id":id,"username":body.username},"csrfToken":new_csrf}})),
    )
        .into_response())
}
pub async fn logout(State(app): State<App>, headers: HeaderMap) -> Result<Response, ApiError> {
    let id = identity(&app, &headers, true).await?;
    sqlx::query("DELETE FROM web_sessions WHERE token_hash=?")
        .bind(&id.token_hash)
        .execute(&app.db)
        .await?;
    app.close_auth(&id.token_hash).await;
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, set_cookie(&app, "", 0))],
    )
        .into_response())
}
pub async fn me(
    State(app): State<App>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = identity(&app, &headers, false).await?;
    let row = sqlx::query("SELECT username FROM users WHERE id=?")
        .bind(&id.user)
        .fetch_one(&app.db)
        .await?;
    Ok(Json(
        json!({"data":{"id":id.user,"username":row.get::<String,_>("username")}}),
    ))
}
pub async fn create_admin(
    db: &sqlx::SqlitePool,
    username: &str,
    password: &str,
) -> anyhow::Result<()> {
    validate_new_credentials(username, password).map_err(|e| anyhow::anyhow!(e.message))?;
    let hash = password_hash(password.to_owned())
        .await
        .map_err(|_| anyhow::anyhow!("无法创建密码哈希"))?;
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("INSERT INTO users VALUES(?,?,?)")
        .bind(Uuid::new_v4().to_string())
        .bind(username)
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    crate::setup::close(&mut tx).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) fn validate_new_credentials(username: &str, password: &str) -> Result<(), ApiError> {
    if username.is_empty()
        || username.len() > 120
        || username.trim() != username
        || username.chars().any(char::is_control)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            "用户名必须为 1–120 字节，不能包含控制字符或首尾空白",
        ));
    }
    if !(12..=1024).contains(&password.len()) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_ERROR",
            "密码长度必须为 12–1024 字节",
        ));
    }
    Ok(())
}

pub(crate) async fn password_hash(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut rand::rngs::OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|_| ApiError::internal())
    })
    .await
    .map_err(|_| ApiError::internal())?
}

#[cfg(test)]
mod login_tests {
    use super::*;
    use std::{net::SocketAddr, sync::Arc};
    use tokio::sync::Mutex;

    async fn app(path: &std::path::Path) -> App {
        App {
            db: crate::storage::open(path).await.unwrap(),
            origin: "http://localhost".into(),
            allowed_origins: vec!["http://localhost".into()],
            allow_any_origin: false,
            trusted_proxies: vec![],
            hub: Arc::new(Mutex::new(crate::Hub::default())),
            login_locks: Arc::new(std::array::from_fn(|_| Mutex::new(()))),
            login_policy: crate::login_guard::Policy::default(),
            connections: crate::Connections::default(),
        }
    }
    async fn attempt(app: &App, ip: &str, username: &str, password: &str) -> Response {
        let response = get_csrf(State(app.clone()), HeaderMap::new())
            .await
            .unwrap();
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let data = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let data: serde_json::Value = serde_json::from_slice(&data).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        headers.insert(header::ORIGIN, "http://localhost".parse().unwrap());
        headers.insert(
            "x-csrf-token",
            data["data"]["csrfToken"].as_str().unwrap().parse().unwrap(),
        );
        // A direct client's spoofed header must not change its rate-limit bucket.
        headers.insert("x-real-ip", Uuid::new_v4().to_string().parse().unwrap());
        match login(
            State(app.clone()),
            ConnectInfo(SocketAddr::new(ip.parse().unwrap(), 1234)),
            headers,
            Json(Login {
                username: username.into(),
                password: password.into(),
            }),
        )
        .await
        {
            Ok(response) => response,
            Err(error) => error.into_response(),
        }
    }
    async fn assert_denied(response: Response) {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key("retry-after"));
        let data = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let data: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(data["error"]["code"], "INVALID_CREDENTIALS");
        assert_eq!(data["error"]["message"], "账号或密码错误");
    }
    async fn assert_throttled(response: Response) -> i64 {
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let seconds = response.headers()[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        let data = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let data: serde_json::Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(data["error"]["code"], "LOGIN_THROTTLED");
        seconds
    }

    async fn failure_state(app: &App, ip: &str) -> crate::login_guard::FailureState {
        crate::login_guard::FailureState::load(&mut app.db.acquire().await.unwrap(), ip)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn backoff_is_persistent_ip_scoped_and_blocked_requests_do_not_extend_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("server.db");
        let first = app(&path).await;
        create_admin(&first.db, "admin", "correct-password")
            .await
            .unwrap();
        for _ in 0..4 {
            assert_denied(attempt(&first, "192.0.2.1", "missing", "wrong").await).await;
        }
        assert_eq!(
            assert_throttled(attempt(&first, "::ffff:192.0.2.1", "missing", "wrong").await).await,
            30
        );
        let previous = failure_state(&first, "192.0.2.1").await;
        assert_throttled(attempt(&first, "192.0.2.1", "admin", "correct-password").await).await;
        assert_eq!(failure_state(&first, "192.0.2.1").await, previous);
        first.db.close().await;
        let reopened = app(&path).await;
        assert_throttled(attempt(&reopened, "192.0.2.1", "admin", "correct-password").await).await;
        assert_eq!(failure_state(&reopened, "192.0.2.1").await, previous);
        assert_eq!(
            attempt(&reopened, "192.0.2.2", "admin", "correct-password")
                .await
                .status(),
            StatusCode::OK
        );

        // Advance the stored deadline without sleeping; the same window remains active.
        sqlx::query("UPDATE login_ip_limits SET blocked_until=? WHERE ip='192.0.2.1'")
            .bind(now())
            .execute(&reopened.db)
            .await
            .unwrap();
        assert_eq!(
            assert_throttled(attempt(&reopened, "192.0.2.1", "missing", "wrong").await).await,
            60
        );
        assert_eq!(failure_state(&reopened, "192.0.2.1").await.failures, 6);
    }

    #[tokio::test]
    async fn window_expiry_and_success_reset_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app(&tmp.path().join("server.db")).await;
        create_admin(&app.db, "admin", "correct-password")
            .await
            .unwrap();
        for _ in 0..4 {
            assert_denied(attempt(&app, "192.0.2.1", "missing", "wrong").await).await;
        }
        assert_eq!(
            attempt(&app, "192.0.2.1", "admin", "correct-password")
                .await
                .status(),
            StatusCode::OK
        );
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM login_ip_limits")
            .fetch_one(&app.db)
            .await
            .unwrap();
        assert_eq!(count, 0);
        assert_denied(attempt(&app, "192.0.2.1", "missing", "wrong").await).await;
        assert_eq!(failure_state(&app, "192.0.2.1").await.failures, 1);
        sqlx::query(
            "UPDATE login_ip_limits SET window_started_at=?,failures=4 WHERE ip='192.0.2.1'",
        )
        .bind(now() - 900)
        .execute(&app.db)
        .await
        .unwrap();
        assert_denied(attempt(&app, "192.0.2.1", "missing", "wrong").await).await;
        assert_eq!(failure_state(&app, "192.0.2.1").await.failures, 1);

        // Expiring the window never cuts short a backoff already issued.
        sqlx::query(
            "UPDATE login_ip_limits SET window_started_at=?,blocked_until=? WHERE ip='192.0.2.1'",
        )
        .bind(now() - 901)
        .bind(now() + 30)
        .execute(&app.db)
        .await
        .unwrap();
        assert_throttled(attempt(&app, "192.0.2.1", "admin", "correct-password").await).await;
        sqlx::query("UPDATE login_ip_limits SET blocked_until=? WHERE ip='192.0.2.1'")
            .bind(now())
            .execute(&app.db)
            .await
            .unwrap();
        assert_eq!(
            attempt(&app, "192.0.2.1", "admin", "correct-password")
                .await
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn concurrent_failures_across_independent_pools_cannot_bypass_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("server.db");
        let first = app(&path).await;
        let second = app(&path).await; // Separate striped mutexes, as with separate processes.
        let responses = futures_util::future::join_all((0..12).map(|n| {
            attempt(
                if n % 2 == 0 { &first } else { &second },
                "192.0.2.3",
                "missing",
                "wrong",
            )
        }))
        .await;
        let mut invalid = 0;
        for response in responses {
            if response.status() == StatusCode::UNAUTHORIZED {
                invalid += 1;
                assert_denied(response).await;
            } else {
                assert_throttled(response).await;
            }
        }
        assert_eq!(invalid, 4);
        assert_eq!(failure_state(&first, "192.0.2.3").await.failures, 5);
    }

    #[tokio::test]
    async fn audited_unlock_releases_throttle_and_records_noops() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app(&tmp.path().join("server.db")).await;
        create_admin(&app.db, "admin", "correct-password")
            .await
            .unwrap();
        for _ in 0..5 {
            attempt(&app, "192.0.2.1", "missing", "wrong").await;
        }
        let id = crate::login_guard::unblock(
            &app.db,
            "::ffff:192.0.2.1".parse().unwrap(),
            "operator",
            "Shared NAT confirmed",
        )
        .await
        .unwrap();
        let row = sqlx::query("SELECT * FROM login_guard_audit WHERE id=?")
            .bind(id)
            .fetch_one(&app.db)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("ip"), "192.0.2.1");
        assert_eq!(row.get::<String, _>("actor"), "operator");
        assert_eq!(row.get::<String, _>("reason"), "Shared NAT confirmed");
        assert_eq!(row.get::<String, _>("action"), "unblock");
        assert_eq!(row.get::<i64, _>("previous_failures"), 5);
        assert!(row.get::<i64, _>("previous_blocked_until") >= row.get::<i64, _>("created_at"));
        assert_eq!(
            attempt(&app, "192.0.2.1", "admin", "correct-password")
                .await
                .status(),
            StatusCode::OK
        );
        let id = crate::login_guard::unblock(
            &app.db,
            "192.0.2.1".parse().unwrap(),
            "operator",
            "Already reset",
        )
        .await
        .unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT previous_failures FROM login_guard_audit WHERE id=?")
                .bind(id)
                .fetch_one(&app.db)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn unlock_audit_failure_does_not_clear_state() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app(&tmp.path().join("server.db")).await;
        assert_denied(attempt(&app, "192.0.2.1", "missing", "wrong").await).await;
        sqlx::raw_sql("CREATE TRIGGER reject_audit BEFORE INSERT ON login_guard_audit BEGIN SELECT RAISE(ABORT,'audit unavailable'); END;")
            .execute(&app.db).await.unwrap();
        assert!(crate::login_guard::unblock(
            &app.db,
            "192.0.2.1".parse().unwrap(),
            "operator",
            "Test"
        )
        .await
        .is_err());
        assert_eq!(failure_state(&app, "192.0.2.1").await.failures, 1);
    }
}
