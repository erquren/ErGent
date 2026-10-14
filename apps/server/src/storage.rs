use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    SqlitePool,
};
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn secret() -> String {
    let mut bytes = [0; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub async fn open(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));
    let db = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;
    sqlx::raw_sql(include_str!("../../../migrations/0001_initial.sql"))
        .execute(&db)
        .await?;
    sqlx::raw_sql(include_str!(
        "../../../migrations/0002_login_ip_failures.sql"
    ))
    .execute(&db)
    .await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    sqlx::raw_sql(include_str!(
        "../../../migrations/0003_terminal_preferences.sql"
    ))
    .execute(&db)
    .await?;
    let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::raw_sql(include_str!("../../../migrations/0004_login_backoff.sql"))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn legacy_counts_are_audited_and_reset_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("legacy.db");
        let old = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../../migrations/0002_login_ip_failures.sql"
        ))
        .execute(&old)
        .await
        .unwrap();
        sqlx::query("INSERT INTO login_ip_failures VALUES('192.0.2.1',3),('192.0.2.2',1)")
            .execute(&old)
            .await
            .unwrap();
        old.close().await;
        let upgraded = open(&path).await.unwrap();
        let counts: Vec<i64> = sqlx::query_scalar("SELECT previous_failures FROM login_guard_audit WHERE action='legacy_reset' ORDER BY ip").fetch_all(&upgraded).await.unwrap();
        assert_eq!(counts, vec![3, 1]);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM login_ip_failures")
            .fetch_one(&upgraded)
            .await
            .unwrap();
        assert_eq!(count, 0);
        sqlx::query("INSERT INTO login_ip_limits VALUES('192.0.2.1',5,?,?)")
            .bind(now())
            .bind(now() + 30)
            .execute(&upgraded)
            .await
            .unwrap();
        upgraded.close().await;
        let reopened = open(&path).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM login_guard_audit")
            .fetch_one(&reopened)
            .await
            .unwrap();
        assert_eq!(count, 2);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM login_ip_limits")
            .fetch_one(&reopened)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
