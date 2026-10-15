DROP TABLE acks;

DELETE FROM outbox WHERE event_type LIKE 'ACK_%';
