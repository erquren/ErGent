CREATE TABLE IF NOT EXISTS users (id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE, password_hash TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS web_sessions (token_hash TEXT PRIMARY KEY, user_id TEXT REFERENCES users(id), csrf TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS web_sessions_expiry ON web_sessions(expires_at);
CREATE TABLE IF NOT EXISTS enrollments (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES users(id), token_hash TEXT NOT NULL UNIQUE, expires_at INTEGER NOT NULL, consumed INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS machines (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL REFERENCES users(id), name TEXT NOT NULL, os TEXT NOT NULL, arch TEXT NOT NULL, credential_hash TEXT NOT NULL UNIQUE, revoked INTEGER NOT NULL DEFAULT 0, default_cwd TEXT NOT NULL DEFAULT '');
CREATE TABLE IF NOT EXISTS terminal_sessions (id TEXT PRIMARY KEY, machine_id TEXT NOT NULL REFERENCES machines(id), data TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS operations (id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, machine_id TEXT NOT NULL, session_id TEXT NOT NULL, kind TEXT NOT NULL, idempotency_key TEXT NOT NULL, request_hash TEXT NOT NULL, status TEXT NOT NULL, result TEXT, created_at INTEGER NOT NULL, UNIQUE(owner_id, kind, idempotency_key));
