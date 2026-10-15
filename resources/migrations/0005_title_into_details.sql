UPDATE payments SET details = json_set(details, '$.title', title) WHERE type = 'standard';
UPDATE payment_schedules SET details = json_set(details, '$.title', title) WHERE type = 'standard';

ALTER TABLE payments DROP COLUMN title;
ALTER TABLE payment_schedules DROP COLUMN title;

UPDATE payments SET status = 'unpaid' WHERE status = 'processing';
