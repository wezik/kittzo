CREATE TABLE processed_emails (
    message_id TEXT PRIMARY KEY NOT NULL,
    outcome TEXT NOT NULL,
    payments_created INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    updated_at TEXT NOT NULL
);
