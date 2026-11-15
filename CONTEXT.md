# Kittzo

Single-user daemon that tracks payments and asks a human to confirm them before they're paid.

## Settings

**Integration**:
An external service Kittzo talks to: Discord, email (IMAP) or OpenRouter.
_Avoid_: Plugin, connector

**Setting**:
A user-editable value configuring an Integration, changed in the Settings tab.
_Avoid_: Config, option, preference

**Secret**:
A Setting that grants access to an Integration (token, password, API key). Never shown back to the user once saved.
_Avoid_: Credential, password (as a general term)

**Override**:
A Setting value supplied by an environment variable. It wins over the stored value and can't be edited in the UI.
_Avoid_: Env config

**Default**:
The value a Setting has when nothing is stored and no Override exists, shipped in `kittzo.toml`.

**Configured**:
An Integration whose required Settings all have values. An Integration that isn't Configured is skipped, not failed.
_Avoid_: Enabled, active
