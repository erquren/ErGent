CREATE TABLE IF NOT EXISTS terminal_preferences (
    session_id TEXT PRIMARY KEY REFERENCES terminal_sessions(id) ON DELETE CASCADE,
    title TEXT,
    theme TEXT NOT NULL DEFAULT 'auto'
);
