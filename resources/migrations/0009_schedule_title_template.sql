ALTER TABLE payment_schedules ADD COLUMN title_template TEXT NOT NULL DEFAULT '';

UPDATE payment_schedules SET
    title_template = json_extract(details, '$.title'),
    details = json_set(
        details,
        '$.title',
        replace(
            replace(json_extract(details, '$.title'), '{MM}', strftime('%m', next_due_at)),
            '{YYYY}',
            strftime('%Y', next_due_at)
        )
    )
WHERE type = 'standard';

ALTER TABLE payment_schedules DROP COLUMN status;
