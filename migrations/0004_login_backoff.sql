CREATE TABLE IF NOT EXISTS login_ip_limits (
    ip TEXT PRIMARY KEY,
    failures INTEGER NOT NULL CHECK(failures >= 1),
    window_started_at INTEGER NOT NULL,
    blocked_until INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS login_ip_limits_window ON login_ip_limits(window_started_at);
CREATE TABLE IF NOT EXISTS login_guard_audit (
    id TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    ip TEXT NOT NULL,
    action TEXT NOT NULL CHECK(action IN ('legacy_reset', 'unblock')),
    actor TEXT NOT NULL,
    reason TEXT NOT NULL,
    previous_failures INTEGER NOT NULL,
    previous_window_started_at INTEGER,
    previous_blocked_until INTEGER
);
CREATE TABLE IF NOT EXISTS login_guard_migrations (name TEXT PRIMARY KEY);
-- Legacy counts have no timestamps: do not turn them into new timed bans.
-- Archive each count and clear exactly once, atomically with the migration marker.
INSERT INTO login_guard_audit(id,created_at,ip,action,actor,reason,previous_failures)
SELECT lower(hex(randomblob(16))),unixepoch(),ip,'legacy_reset','migration:0004',
       'Legacy permanent failure counts had no timestamps; reset during upgrade',failures
FROM login_ip_failures
WHERE NOT EXISTS (SELECT 1 FROM login_guard_migrations WHERE name='0004_login_backoff');
DELETE FROM login_ip_failures
WHERE NOT EXISTS (SELECT 1 FROM login_guard_migrations WHERE name='0004_login_backoff');
INSERT OR IGNORE INTO login_guard_migrations(name) VALUES('0004_login_backoff');
