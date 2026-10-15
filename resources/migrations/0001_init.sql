CREATE TABLE payments (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    total_amount TEXT NOT NULL,
    total_currency TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    title TEXT NOT NULL,
    flow TEXT NOT NULL,
    details TEXT NOT NULL,
    status TEXT NOT NULL,
    source_type TEXT NOT NULL,
    schedule_id TEXT,
    occurrence_date TEXT
);

CREATE TABLE payment_schedules (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    total_amount TEXT NOT NULL,
    total_currency TEXT NOT NULL,
    title TEXT NOT NULL,
    flow TEXT NOT NULL,
    details TEXT NOT NULL,
    recurrence_type TEXT NOT NULL,
    day_of_month INTEGER NOT NULL,
    status TEXT NOT NULL,
    next_due_at TEXT
);

CREATE TABLE acks (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    subject_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    state TEXT NOT NULL,
    decided_at TEXT
);

CREATE TABLE outbox (
    id TEXT PRIMARY KEY NOT NULL,
    event_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    payload BLOB NOT NULL,
    created_at TEXT NOT NULL,
    claimed_at TEXT,
    processed_at TEXT
);

CREATE INDEX idx_outbox_unprocessed ON outbox (id) WHERE processed_at IS NULL;

-- Infra-only mapping from a confirmation to the Discord message that asked about it.
-- Deliberately not part of the domain (Ack stays Discord-agnostic) - owned and queried
-- directly by infra::discord.
CREATE TABLE discord_confirmation_messages (
    confirmation_id TEXT PRIMARY KEY NOT NULL,
    channel_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL
);
