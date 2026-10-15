UPDATE outbox
SET id = substr(id, 1, 8) || '-' || substr(id, 9, 4) || '-' || substr(id, 13, 4) || '-'
    || substr(id, 17, 4) || '-' || substr(id, 21, 12)
WHERE length(id) = 32 AND instr(id, '-') = 0;
