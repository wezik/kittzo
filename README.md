# Kittzo

> [!NOTE]
> WIP readme. Covers what Kittzo is + how to run it.

Single-user daemon for tracking payments. Lives in the tray. Never moves money itself. You pay by hand or via local scripts hitting the Kittzo API.

Non-manual payments need human confirmation before processing.

Not settled yet, may change:
- Confirmations via UI or Discord.
- Payments created from emails (read by LLM), schedules, or by hand.

HTTP server binds `127.0.0.1` only, no auth. Don't expose it.

## Stack

- Rust 2024, tokio, axum
- SQLite via sqlx
- LLMs via OpenRouter
- plain HTML + JS UI

## Requirements

Linux tray needs GTK headers:

```sh
sudo apt-get install -y libgtk-3-dev libxdo-dev libayatana-appindicator3-dev
```

## Running locally

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install
cp .env.example .env   # fill what you need
mise run dev
```

UI at http://127.0.0.1:7878, or tray > "Open UI".

Config:
- Integrations (Discord, email, OpenRouter) set up in UI Settings tab. Applies on restart.
- Secrets go to OS keyring, rest to DB. Linux keyring needs a Secret Service provider, e.g. GNOME Keyring.
- Missing settings = integration stays off. Startup log says what's missing.
- `kittzo.toml` = safe defaults only (poll intervals, OpenRouter model).
- Env vars from `.env.example` override matching setting. UI shows that setting locked. Meant for dev.
- `RUST_LOG` sets log filter.

Home dir holds `kittzo.toml`, `kittzo.db`, `resources/static/`, `scripts/`. It's `KITTZO_HOME` if set, else exe's dir. `mise run dev` points it at repo root, so each worktree gets own config + DB.

## Tasks

All in `mise.toml`.

| Command | Does |
| --- | --- |
| `mise run check` | check `.env` has every key from `.env.example` |
| `mise run dev` | run `check`, then start Kittzo |
| `mise run test` | run tests |
| `mise run format` | format code |

## Building

```sh
cargo build --release --locked
```

UI not embedded in binary. Ship `resources/static/` next to it.

## Releasing

Push a `v*` tag. CI (`.github/workflows/release.yml`) publishes GitHub release with Windows + Linux archives.

```sh
git tag v0.2.0
git push origin v0.2.0
```

## Versioning

Follows [semver](https://semver.org/).

Pre-1.0, any minor can break API, config or DB. Back up `kittzo.db`, `kittzo.db-wal`, `kittzo.db-shm` before upgrading.

From 1.0 on, breaking those = major bump. Schema changes go in new migrations, old ones never touched.

## Architecture

```
src/
  domain/  types + business rules, no IO
  app/     services, background jobs
  infra/   adapters: DB, Discord, email, config, tray
  api/     HTTP routes
  main.rs  wiring
```

- `domain/` depends on nothing else.
- Routes stay thin. Parse request, call service, map result.
- Unit tests mainly cover `domain/` + `app/`.

## Scripts

> [!WARNING]
> WIP. Not in releases yet.

`scripts/` = local Python scripts that process confirmed payments, e.g. one driving a bank's website. Gitignored, since it encodes how you pay and may hold a logged-in browser session.

- `scripts/run.py` is entry point. Lists scripts, runs the one you pick.
- Tray > "Run payment script" runs it with `scripts/.venv`.
- Scripts use HTTP API only, never touch DB.

Recommended rules for scripts touching money:
- Validate payment before acting. Anything off, skip.
- Mark paid only after external confirmation. Unsure? Leave unpaid, check by hand.
- Creds in OS keyring.
- Support dry run that stops before final submit.
