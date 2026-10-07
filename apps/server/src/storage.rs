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
    Ok(db)
}
