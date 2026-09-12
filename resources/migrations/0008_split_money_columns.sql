CREATE TABLE payments_new (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    total_amount TEXT NOT NULL,
    total_currency TEXT NOT NULL,
    created_at TEXT NOT NULL,
    source_type TEXT NOT NULL,
    schedule_id TEXT,
    occurrence_date TEXT
);

INSERT INTO payments_new
SELECT id, version, substr(total, 5), substr(total, 1, 3), created_at, source_type, schedule_id, occurrence_date
FROM payments;

DROP TABLE payments;
ALTER TABLE payments_new RENAME TO payments;

CREATE TABLE payment_schedules_new (
    id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    total_amount TEXT NOT NULL,
    total_currency TEXT NOT NULL,
    recurrence_type TEXT NOT NULL,
    day_of_month INTEGER NOT NULL,
    status TEXT NOT NULL,
    next_due_at TEXT
);

INSERT INTO payment_schedules_new
SELECT id, version, created_at, updated_at, substr(total, 5), substr(total, 1, 3), recurrence_type, day_of_month, status, next_due_at
FROM payment_schedules;

DROP TABLE payment_schedules;
ALTER TABLE payment_schedules_new RENAME TO payment_schedules;
