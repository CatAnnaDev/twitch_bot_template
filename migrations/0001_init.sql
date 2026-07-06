CREATE TABLE IF NOT EXISTS messages (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    login   TEXT NOT NULL,
    text    TEXT NOT NULL,
    at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_messages_channel ON messages(channel);

CREATE TABLE IF NOT EXISTS commands (
    name     TEXT PRIMARY KEY,
    response TEXT NOT NULL,
    uses     INTEGER NOT NULL DEFAULT 0
);
