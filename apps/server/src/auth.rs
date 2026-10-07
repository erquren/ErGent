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
fn csrf(headers: &HeaderMap, expected: &str) -> Result<(), ApiError> {
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
    let failures: i64 = sqlx::query_scalar(
        "SELECT COALESCE((SELECT failures FROM login_ip_failures WHERE ip=?),0)",
    )
    .bind(&key)
    .fetch_one(&app.db)
    .await?;
    if failures >= 3 {
        return Err(crate::login_guard::invalid_login());
    }
    let user = if body.password.len() <= 1024 && body.username.len() <= 120 {
        sqlx::query("SELECT id,password_hash FROM users WHERE username=?")
            .bind(&body.username)
            .fetch_optional(&app.db)
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
        sqlx::query("INSERT INTO login_ip_failures(ip,failures) VALUES(?,1) ON CONFLICT(ip) DO UPDATE SET failures=MIN(failures+1,3)")
            .bind(&key).execute(&app.db).await?;
        return Err(crate::login_guard::invalid_login());
    }
    let id: String = user.ok_or_else(ApiError::internal)?.get("id");
    let token = secret();
    let new_csrf = secret();
    let mut tx = app.db.begin().await?;
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
    anyhow::ensure!(
        password.len() >= 12 && password.len() <= 1024,
        "密码长度必须为 12–1024 字节"
    );
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .to_string();
    sqlx::query("INSERT INTO users VALUES(?,?,?)")
        .bind(Uuid::new_v4().to_string())
        .bind(username)
        .bind(hash)
        .execute(db)
        .await?;
    Ok(())
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
    #[tokio::test]
    async fn failures_are_cumulative_persistent_and_ip_scoped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("server.db");
        let first = app(&path).await;
        create_admin(&first.db, "admin", "correct-password")
            .await
            .unwrap();
        assert_denied(attempt(&first, "192.0.2.1", "missing", "wrong").await).await;
        assert_eq!(
            attempt(&first, "192.0.2.1", "admin", "correct-password")
                .await
                .status(),
            StatusCode::OK
        );
        assert_denied(attempt(&first, "192.0.2.1", "admin", "wrong").await).await;
        assert_denied(attempt(&first, "::ffff:192.0.2.1", "admin", "wrong").await).await;
        assert_denied(attempt(&first, "192.0.2.1", "admin", "correct-password").await).await;
        first.db.close().await;
        let reopened = app(&path).await;
        assert_denied(attempt(&reopened, "192.0.2.1", "admin", "correct-password").await).await;
        assert_eq!(
            attempt(&reopened, "192.0.2.2", "admin", "correct-password")
                .await
                .status(),
            StatusCode::OK
        );
        sqlx::query("DELETE FROM login_ip_failures WHERE ip=?")
            .bind("192.0.2.1")
            .execute(&reopened.db)
            .await
            .unwrap();
        assert_eq!(
            attempt(&reopened, "192.0.2.1", "admin", "correct-password")
                .await
                .status(),
            StatusCode::OK
        );
    }
    #[tokio::test]
    async fn concurrent_failures_cannot_lose_increments() {
        let tmp = tempfile::tempdir().unwrap();
        let app = app(&tmp.path().join("server.db")).await;
        create_admin(&app.db, "admin", "correct-password")
            .await
            .unwrap();
        let (a, b, c, d) = tokio::join!(
            attempt(&app, "192.0.2.3", "admin", "wrong"),
            attempt(&app, "192.0.2.3", "admin", "wrong"),
            attempt(&app, "192.0.2.3", "admin", "wrong"),
            attempt(&app, "192.0.2.3", "admin", "wrong")
        );
        for response in [a, b, c, d] {
            assert_denied(response).await;
        }
        let failures: i64 =
            sqlx::query_scalar("SELECT failures FROM login_ip_failures WHERE ip='192.0.2.3'")
                .fetch_one(&app.db)
                .await
                .unwrap();
        assert_eq!(failures, 3);
        assert_denied(attempt(&app, "192.0.2.3", "admin", "correct-password").await).await;
    }
}
