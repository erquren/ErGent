CREATE TABLE IF NOT EXISTS first_account_setup (
    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
    completed INTEGER NOT NULL CHECK(completed IN (0, 1)),
    code_hash TEXT,
    code_expires_at INTEGER,
    CHECK((code_hash IS NULL) = (code_expires_at IS NULL)),
    CHECK(completed = 0 OR code_hash IS NULL)
);
-- An existing installation is permanently closed, even if its users are later deleted.
INSERT OR IGNORE INTO first_account_setup(singleton, completed)
SELECT 1, EXISTS(SELECT 1 FROM users);
UPDATE first_account_setup SET completed = 1, code_hash = NULL, code_expires_at = NULL
WHERE EXISTS(SELECT 1 FROM users);
-- Cover CLI creation and every other insertion path in the same transaction.
CREATE TRIGGER IF NOT EXISTS users_close_first_account_setup
AFTER INSERT ON users BEGIN
    UPDATE first_account_setup SET completed = 1, code_hash = NULL, code_expires_at = NULL
    WHERE singleton = 1;
END;
CREATE TABLE IF NOT EXISTS setup_ip_limits (
    ip TEXT PRIMARY KEY,
    failures INTEGER NOT NULL CHECK(failures >= 1),
    window_started_at INTEGER NOT NULL,
    blocked_until INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS setup_ip_limits_window ON setup_ip_limits(window_started_at);
