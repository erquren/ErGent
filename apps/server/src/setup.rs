//! First-account bootstrap. Possession of a short-lived operator code is required;
//! an empty users table alone can never reopen a completed installation.
use crate::{auth, login_guard, storage, ApiError, App};
use axum::{
    extract::{ConnectInfo, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use sqlx::{Row, SqliteConnection, SqlitePool};
use std::io::{IsTerminal, Write};

const CODE_LIFETIME_SECONDS: i64 = 15 * 60;

pub async fn required(db: &SqlitePool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT completed = 0 AND NOT EXISTS(SELECT 1 FROM users) FROM first_account_setup WHERE singleton = 1")
        .fetch_one(db)
        .await
}

pub async fn close(db: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE first_account_setup SET completed = 1, code_hash = NULL, code_expires_at = NULL WHERE singleton = 1")
        .execute(db)
        .await?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum Issuance {
    Issued,
    Pending,
    Closed,
}

// Only console_code calls this in production, after opening and verifying an
// actual terminal. Output failure rolls back rotation, retaining any prior code.
async fn issue_code_to(
    db: &SqlitePool,
    rotate: bool,
    output: &mut (impl Write + Send),
) -> anyhow::Result<Issuance> {
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let row = sqlx::query("SELECT completed, code_expires_at, EXISTS(SELECT 1 FROM users) AS has_users FROM first_account_setup WHERE singleton = 1")
        .fetch_one(&mut *tx)
        .await?;
    if row.get::<bool, _>("completed") || row.get::<bool, _>("has_users") {
        close(&mut tx).await?;
        tx.commit().await?;
        return Ok(Issuance::Closed);
    }
    let now = storage::now();
    if !rotate
        && row
            .get::<Option<i64>, _>("code_expires_at")
            .is_some_and(|end| end > now)
    {
        tx.commit().await?;
        return Ok(Issuance::Pending);
    }
    let code = storage::secret();
    sqlx::query("UPDATE first_account_setup SET code_hash = ?, code_expires_at = ? WHERE singleton = 1 AND completed = 0")
        .bind(storage::hash(&code))
        .bind(now + CODE_LIFETIME_SECONDS)
        .execute(&mut *tx)
        .await?;
    writeln!(
        output,
        "\nErGent first-account setup (expires in 15 minutes)."
    )?;
    writeln!(output, "Setup code: {code}")?;
    writeln!(
        output,
        "Open the configured localhost HTTP or HTTPS origin. Keep this code private.\n"
    )?;
    output.flush()?;
    tx.commit().await?;
    Ok(Issuance::Issued)
}

fn controlling_terminal() -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    let name = "/dev/tty";
    #[cfg(windows)]
    let name = "CONOUT$";
    #[cfg(not(any(unix, windows)))]
    return Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "No supported console",
    ));
    #[cfg(any(unix, windows))]
    {
        // Windows GetConsoleMode (used by IsTerminal) requires GENERIC_READ,
        // including when checking a CONOUT$ output handle.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)?;
        if !file.is_terminal() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "No controlling terminal",
            ));
        }
        Ok(file)
    }
}

/// Ordinary startup never rotates an unexpired code owned by another instance.
/// Explicit setup-code is the sole operator-requested rotation path. Neither
/// path sends a code to stdout/stderr, a log sink, an environment variable or a URL.
pub async fn console_code(db: &SqlitePool, rotate: bool) -> anyhow::Result<()> {
    if !required(db).await? {
        anyhow::ensure!(!rotate, "首次账号设置已永久关闭");
        return Ok(());
    }
    let mut terminal = match controlling_terminal() {
        Ok(terminal) => terminal,
        Err(_) if !rotate => {
            tracing::warn!("First-account setup requires a console code. Run `ergent-server --database PATH setup-code` in an operator terminal, or use admin-create.");
            return Ok(());
        }
        Err(_) => anyhow::bail!("需要实际控制终端；请在终端运行 setup-code，或使用 admin-create"),
    };
    match issue_code_to(db, rotate, &mut terminal).await {
        Ok(Issuance::Issued) => Ok(()),
        Ok(Issuance::Pending) => {
            tracing::info!("An existing setup code remains valid. To replace it, run `ergent-server --database PATH setup-code` in an operator terminal.");
            Ok(())
        }
        Ok(Issuance::Closed) => {
            anyhow::ensure!(!rotate, "首次账号设置已永久关闭");
            Ok(())
        }
        Err(_) => {
            anyhow::bail!("无法提供初始化码；请在终端重新运行 setup-code，或使用 admin-create")
        }
    }
}

pub async fn status(State(app): State<App>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        json!({"data": {"required": required(&app.db).await?}}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Setup {
    username: String,
    password: String,
    password_confirmation: String,
    setup_code: String,
}

fn origin_rejected() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "ORIGIN_REJECTED",
        "首次设置必须使用已配置的本机 HTTP 或 HTTPS 来源",
    )
}

fn strict_origin(app: &App, headers: &HeaderMap) -> Result<bool, ApiError> {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let origin = origins.next().and_then(|value| value.to_str().ok());
    if origins.next().is_some()
        || !crate::origin::request_allowed(false, origin, &app.allowed_origins)
        || !origin.is_some_and(|value| crate::origin::validate(value, false).is_ok())
    {
        return Err(origin_rejected());
    }
    Ok(origin.is_some_and(|value| value.starts_with("http:")))
}

fn invalid_code() -> ApiError {
    ApiError::new(
        StatusCode::BAD_REQUEST,
        "SETUP_CODE_INVALID",
        "初始化码无效或已过期；请在服务器终端运行 setup-code 获取新码",
    )
}

fn throttled(seconds: i64) -> Response {
    let mut response = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "SETUP_THROTTLED",
        "初始化尝试过多，请稍后重试",
    )
    .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        seconds
            .max(1)
            .to_string()
            .parse()
            .expect("positive seconds form a header"),
    );
    response
}

pub async fn create(
    State(app): State<App>,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Setup>,
) -> Result<Response, ApiError> {
    let loopback_http = strict_origin(&app, &headers)?;
    let ip = login_guard::client_ip(peer.ip(), &headers, &app.trusted_proxies)?;
    // Origin is a browser boundary, not proof of transport or network location.
    // HTTP setup must be local at both ends of a trusted proxy; HTTPS deployments
    // must terminate TLS at the configured origin and keep this backend private.
    if loopback_http && (!login_guard::normalize(peer.ip()).is_loopback() || !ip.is_loopback()) {
        return Err(origin_rejected());
    }
    let session = auth::cookie(&headers).ok_or_else(ApiError::unauthorized)?;
    let row = sqlx::query(
        "SELECT csrf FROM web_sessions WHERE token_hash = ? AND user_id IS NULL AND expires_at > ?",
    )
    .bind(storage::hash(&session))
    .bind(storage::now())
    .fetch_optional(&app.db)
    .await?
    .ok_or_else(ApiError::unauthorized)?;
    auth::csrf(&headers, row.get("csrf"))?;
    let ip = ip.to_string();
    // Independent of login settings/state; shared by server instances and CLI.
    let policy = login_guard::Policy::default();
    let mut tx = app.db.begin_with("BEGIN IMMEDIATE").await?;
    let row = sqlx::query("SELECT completed, code_hash, code_expires_at, EXISTS(SELECT 1 FROM users) AS has_users FROM first_account_setup WHERE singleton = 1")
        .fetch_one(&mut *tx)
        .await?;
    if row.get::<bool, _>("completed") || row.get::<bool, _>("has_users") {
        close(&mut tx).await?;
        tx.commit().await?;
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "SETUP_CLOSED",
            "首次账号设置已完成，请登录",
        ));
    }
    let now = storage::now();
    sqlx::query("DELETE FROM setup_ip_limits WHERE window_started_at <= ? AND blocked_until <= ?")
        .bind(now - i64::from(policy.window_seconds))
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let previous = sqlx::query(
        "SELECT failures, window_started_at, blocked_until FROM setup_ip_limits WHERE ip = ?",
    )
    .bind(&ip)
    .fetch_optional(&mut *tx)
    .await?
    .map(|row| login_guard::FailureState {
        failures: row.get("failures"),
        window_started_at: row.get("window_started_at"),
        blocked_until: row.get("blocked_until"),
    });
    if let Some(state) = previous {
        if state.retry_after(now) > 0 {
            tx.commit().await?;
            return Ok(throttled(state.retry_after(now)));
        }
    }
    let validation = auth::validate_new_credentials(&body.username, &body.password)
        .and_then(|()| {
            if body.password_confirmation.len() > 1024
                || body.password != body.password_confirmation
            {
                Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "VALIDATION_ERROR",
                    "两次输入的密码不一致",
                ))
            } else {
                Ok(())
            }
        })
        .and_then(|()| {
            if body.setup_code.len() == 43
                && row
                    .get::<Option<i64>, _>("code_expires_at")
                    .is_some_and(|end| end > now)
                && row.get::<Option<String>, _>("code_hash").as_deref()
                    == Some(storage::hash(&body.setup_code).as_str())
            {
                Ok(())
            } else {
                Err(invalid_code())
            }
        });
    if let Err(error) = validation {
        let failed = login_guard::FailureState::failed(previous, &policy, now);
        sqlx::query("INSERT INTO setup_ip_limits(ip, failures, window_started_at, blocked_until) VALUES(?, ?, ?, ?) ON CONFLICT(ip) DO UPDATE SET failures = excluded.failures, window_started_at = excluded.window_started_at, blocked_until = excluded.blocked_until")
            .bind(&ip)
            .bind(failed.failures)
            .bind(failed.window_started_at)
            .bind(failed.blocked_until)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return if failed.retry_after(now) > 0 {
            Ok(throttled(failed.retry_after(now)))
        } else {
            Err(error)
        };
    }
    let password_hash = auth::password_hash(body.password).await?;
    sqlx::query("INSERT INTO users(id, username, password_hash) VALUES(?, ?, ?)")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&body.username)
        .bind(password_hash)
        .execute(&mut *tx)
        .await?;
    close(&mut tx).await?;
    sqlx::query("DELETE FROM setup_ip_limits")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    // Keep the anonymous CSRF session for an explicit, ordinary login next.
    Ok(Json(json!({"data": {"created": true}})).into_response())
}

#[cfg(test)]
mod tests;
