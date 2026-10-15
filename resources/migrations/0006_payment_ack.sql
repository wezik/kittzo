ALTER TABLE payments ADD COLUMN ack TEXT NOT NULL DEFAULT 'pending';

UPDATE payments
SET ack = (
    SELECT a.state FROM acks a
    WHERE a.subject_type = 'payment' AND a.subject_id = payments.id
    ORDER BY a.created_at DESC
    LIMIT 1
)
WHERE EXISTS (
    SELECT 1 FROM acks a WHERE a.subject_type = 'payment' AND a.subject_id = payments.id
);

INSERT INTO outbox (id, event_type, aggregate_id, payload, created_at)
SELECT
    lower(hex(randomblob(16))),
    'PAYMENT_CREATED',
    p.id,
    CAST(json_object('Created', json_object('id', p.id)) AS BLOB),
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM payments p
WHERE NOT EXISTS (
    SELECT 1 FROM acks a WHERE a.subject_type = 'payment' AND a.subject_id = p.id
);

UPDATE discord_confirmation_messages
SET confirmation_id = (
    SELECT a.subject_id FROM acks a WHERE a.id = discord_confirmation_messages.confirmation_id
)
WHERE EXISTS (
    SELECT 1 FROM acks a WHERE a.id = discord_confirmation_messages.confirmation_id
);
