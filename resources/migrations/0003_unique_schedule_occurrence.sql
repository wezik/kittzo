CREATE UNIQUE INDEX idx_payments_schedule_occurrence ON payments (schedule_id, occurrence_date)
    WHERE source_type = 'schedule';
