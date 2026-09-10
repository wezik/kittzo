CREATE TABLE confirmations (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    subject_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    state TEXT NOT NULL,
    decided_at TEXT
);
