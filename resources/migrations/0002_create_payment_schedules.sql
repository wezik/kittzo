CREATE TABLE payment_schedules (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    total TEXT NOT NULL,
    recurrence_type TEXT NOT NULL,
    interval_months INTEGER NOT NULL,
    day_of_month INTEGER NOT NULL,
    last_run_at TEXT,
    status TEXT NOT NULL
);
