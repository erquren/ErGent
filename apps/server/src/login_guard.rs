use crate::ApiError;
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use sqlx::{Row, SqliteConnection, SqlitePool};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::IpAddr,
};

/// A fixed failure window. A backoff already issued always runs to its deadline.
#[derive(Clone, Debug, clap::Args)]
pub struct Policy {
    /// Failure-count window, measured from its first failure.
    #[arg(long = "login-window-seconds", default_value_t = 900, value_parser = clap::value_parser!(u32).range(1..=86400))]
    pub window_seconds: u32,
    #[arg(long = "login-failure-threshold", default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=1000000))]
    pub failure_threshold: u32,
    #[arg(long = "login-backoff-seconds", default_value_t = 30, value_parser = clap::value_parser!(u32).range(1..=86400))]
    pub backoff_seconds: u32,
    #[arg(long = "login-max-backoff-seconds", default_value_t = 900, value_parser = clap::value_parser!(u32).range(1..=86400))]
    pub max_backoff_seconds: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            window_seconds: 900,
            failure_threshold: 5,
            backoff_seconds: 30,
            max_backoff_seconds: 900,
        }
    }
}

impl Policy {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.backoff_seconds <= self.max_backoff_seconds,
            "login-backoff-seconds must not exceed login-max-backoff-seconds"
        );
        Ok(())
    }

    fn delay(&self, failures: i64) -> i64 {
        if failures < i64::from(self.failure_threshold) {
            return 0;
        }
        let exponent = (failures - i64::from(self.failure_threshold)).min(62) as u32;
        i64::from(self.backoff_seconds)
            .saturating_mul(1_i64 << exponent)
            .min(i64::from(self.max_backoff_seconds))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailureState {
    pub failures: i64,
    pub window_started_at: i64,
    pub blocked_until: i64,
}

impl FailureState {
    pub async fn load(db: &mut SqliteConnection, ip: &str) -> Result<Option<Self>, sqlx::Error> {
        Ok(sqlx::query(
            "SELECT failures,window_started_at,blocked_until FROM login_ip_limits WHERE ip=?",
        )
        .bind(ip)
        .fetch_optional(db)
        .await?
        .map(|row| Self {
            failures: row.get("failures"),
            window_started_at: row.get("window_started_at"),
            blocked_until: row.get("blocked_until"),
        }))
    }

    pub fn retry_after(&self, now: i64) -> i64 {
        self.blocked_until.saturating_sub(now).max(0)
    }

    pub fn failed(previous: Option<Self>, policy: &Policy, now: i64) -> Self {
        let previous = previous.filter(|state| {
            now < state
                .window_started_at
                .saturating_add(i64::from(policy.window_seconds))
        });
        let failures = previous.map_or(1, |state| state.failures.saturating_add(1));
        Self {
            failures,
            window_started_at: previous.map_or(now, |state| state.window_started_at),
            blocked_until: now.saturating_add(policy.delay(failures)),
        }
    }

    pub async fn save(&self, db: &mut SqliteConnection, ip: &str) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO login_ip_limits(ip,failures,window_started_at,blocked_until) VALUES(?,?,?,?) ON CONFLICT(ip) DO UPDATE SET failures=excluded.failures,window_started_at=excluded.window_started_at,blocked_until=excluded.blocked_until")
            .bind(ip)
            .bind(self.failures)
            .bind(self.window_started_at)
            .bind(self.blocked_until)
            .execute(db)
            .await?;
        Ok(())
    }
}

pub fn throttled(retry_after: i64) -> Response {
    let mut response = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "LOGIN_THROTTLED",
        "登录尝试过多，请稍后重试",
    )
    .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        retry_after
            .max(1)
            .to_string()
            .parse()
            .expect("positive seconds form a valid header"),
    );
    response
}

/// Serialize the audit and reset with live login attempts, including other processes.
/// The actor is an operator-supplied label, not independently authenticated identity.
pub async fn unblock(
    db: &SqlitePool,
    ip: IpAddr,
    actor: &str,
    reason: &str,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        !actor.trim().is_empty() && actor.len() <= 120,
        "actor must be 1–120 bytes"
    );
    anyhow::ensure!(
        !reason.trim().is_empty() && reason.len() <= 1024,
        "reason must be 1–1024 bytes"
    );
    let ip = normalize(ip).to_string();
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    let previous = FailureState::load(&mut tx, &ip).await?;
    let audit_id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO login_guard_audit(id,created_at,ip,action,actor,reason,previous_failures,previous_window_started_at,previous_blocked_until) VALUES(?,?,?,'unblock',?,?,?,?,?)")
        .bind(&audit_id)
        .bind(crate::storage::now())
        .bind(&ip)
        .bind(actor)
        .bind(reason)
        .bind(previous.map_or(0, |state| state.failures))
        .bind(previous.map(|state| state.window_started_at))
        .bind(previous.map(|state| state.blocked_until))
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM login_ip_limits WHERE ip=?")
        .bind(&ip)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    tracing::info!(%audit_id, %ip, actor, "login throttle cleared by CLI");
    Ok(audit_id)
}

pub fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        _ => ip,
    }
}

pub fn invalid_login() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "INVALID_CREDENTIALS",
        "账号或密码错误",
    )
}

pub fn client_ip(
    peer: IpAddr,
    headers: &HeaderMap,
    trusted: &[IpAddr],
) -> Result<IpAddr, ApiError> {
    let peer = normalize(peer);
    if !trusted.contains(&peer) {
        return Ok(peer); // Never trust arbitrary clients' forwarding headers.
    }
    let mut values = headers.get_all("x-real-ip").iter();
    let value = values.next().and_then(|v| v.to_str().ok());
    if values.next().is_some() {
        return Err(invalid_login());
    }
    value
        .and_then(|v| v.parse::<IpAddr>().ok())
        .map(normalize)
        .ok_or_else(invalid_login)
}

pub fn stripe(ip: IpAddr) -> usize {
    let mut hash = DefaultHasher::new();
    ip.hash(&mut hash);
    hash.finish() as usize % 64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exponential_backoff_is_bounded_and_fixed_window_expires() {
        let policy = Policy::default();
        let mut state = None;
        for failures in 1..=12 {
            let next = FailureState::failed(state, &policy, 100);
            assert_eq!(next.failures, failures);
            assert_eq!(
                next.retry_after(100),
                match failures {
                    1..=4 => 0,
                    5 => 30,
                    6 => 60,
                    7 => 120,
                    8 => 240,
                    9 => 480,
                    _ => 900,
                }
            );
            state = Some(next);
        }
        assert_eq!(FailureState::failed(state, &policy, 999).failures, 13);
        let reset = FailureState::failed(state, &policy, 1000);
        assert_eq!(reset.failures, 1);
        assert_eq!(reset.window_started_at, 1000);
        assert_eq!(reset.retry_after(1000), 0);
        assert_eq!(policy.delay(i64::MAX), 900);
        assert_eq!(state.unwrap().retry_after(1000), 0);
    }

    #[test]
    fn policy_flags_have_safe_defaults_and_reject_invalid_settings() {
        use clap::Parser;
        let parse = |flags: &[&str]| {
            crate::Cli::try_parse_from(
                ["ergent-server", "run"]
                    .into_iter()
                    .chain(flags.iter().copied()),
            )
        };
        let crate::Command::Run { login_policy, .. } = parse(&[]).unwrap().command else {
            panic!("run expected")
        };
        assert_eq!(login_policy.window_seconds, 900);
        assert_eq!(login_policy.failure_threshold, 5);
        assert_eq!(login_policy.backoff_seconds, 30);
        assert_eq!(login_policy.max_backoff_seconds, 900);
        assert!(parse(&["--login-failure-threshold", "0"]).is_err());
        assert!(parse(&["--login-window-seconds", "0"]).is_err());
        assert!(parse(&["--login-max-backoff-seconds", "86401"]).is_err());
        let crate::Command::Run { login_policy, .. } = parse(&[
            "--login-backoff-seconds",
            "60",
            "--login-max-backoff-seconds",
            "30",
        ])
        .unwrap()
        .command
        else {
            panic!("run expected")
        };
        assert!(login_policy.validate().is_err());
        assert!(
            crate::Cli::try_parse_from(["ergent-server", "login-unblock", "192.0.2.1"]).is_err()
        );
    }

    #[test]
    fn forwarding_requires_explicit_trust_and_a_single_valid_address() {
        let peer = "127.0.0.1".parse().unwrap();
        let mut h = HeaderMap::new();
        h.insert("x-real-ip", "203.0.113.5".parse().unwrap());
        h.insert("x-forwarded-for", "198.51.100.1".parse().unwrap());
        assert_eq!(client_ip(peer, &h, &[]).unwrap(), peer);
        assert_eq!(
            client_ip(peer, &h, &[peer]).unwrap(),
            "203.0.113.5".parse::<IpAddr>().unwrap()
        );
        h.append("x-real-ip", "203.0.113.6".parse().unwrap());
        assert!(client_ip(peer, &h, &[peer]).is_err());
        for value in [
            "",
            "garbage",
            "203.0.113.5, 203.0.113.6",
            "203.0.113.5:1234",
        ] {
            h.remove("x-real-ip");
            h.insert("x-real-ip", value.parse().unwrap());
            assert!(client_ip(peer, &h, &[peer]).is_err());
        }
        assert!(client_ip(peer, &HeaderMap::new(), &[peer]).is_err());
        assert_eq!(
            normalize("::ffff:192.0.2.1".parse().unwrap()),
            "192.0.2.1".parse::<IpAddr>().unwrap()
        );
    }
}
