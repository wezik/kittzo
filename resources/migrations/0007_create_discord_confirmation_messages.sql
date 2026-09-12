-- Infra-only mapping from a confirmation to the Discord message that asked about it.
-- Deliberately not part of the domain (Confirmation stays Discord-agnostic) - owned and
-- queried directly by infra::discord.
CREATE TABLE discord_confirmation_messages (
    confirmation_id TEXT PRIMARY KEY NOT NULL,
    channel_id INTEGER NOT NULL,
    message_id INTEGER NOT NULL
);
