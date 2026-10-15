ALTER TABLE payments RENAME COLUMN flow TO type;
ALTER TABLE payment_schedules RENAME COLUMN flow TO type;

UPDATE payments SET type = 'standard' WHERE type = 'domestic';
UPDATE payment_schedules SET type = 'standard' WHERE type = 'domestic';

UPDATE payments SET status = 'processing' WHERE status = 'submitting';

ALTER TABLE payments ADD COLUMN paid_at TEXT;
UPDATE payments SET paid_at = updated_at WHERE status = 'paid';
