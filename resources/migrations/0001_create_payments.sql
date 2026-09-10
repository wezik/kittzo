CREATE TABLE payments (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    total TEXT NOT NULL,
    created_at TEXT NOT NULL,
    source_type TEXT NOT NULL
);
