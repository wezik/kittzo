# Kittzo

> [!NOTE]
> This README is a work in progress. It covers what Kittzo is and how to run it.

Kittzo is a single-user daemon that tracks payments. Non-manual payment entries require human confirmation before they can be processed. Kittzo never moves money by itself. Paying happens by hand or through local scripts using the Kittzo API.

There are a few points that are not settled yet and might change:
- Confirmations are done through UI or Discord.
- Payment entries are created by emails read by an LLM, schedules or manually.

Kittzo presents as a tray icon. The HTTP server listens on `127.0.0.1` only and has no auth. Don't expose it.

## Stack

- Rust 2024, with tokio and axum
- SQLite via sqlx
- LLMs via OpenRouter
- a plain HTML and JS UI

## Requirements

On Linux the tray needs GTK headers:

```sh
sudo apt-get install -y libgtk-3-dev libxdo-dev libayatana-appindicator3-dev
```

## Running locally

Install [mise](https://mise.jdx.dev/), then:

```sh
mise install
cp .env.example .env   # fill in the values you need
mise run dev
```

The UI is at http://127.0.0.1:7878. The tray's "Open UI" item opens it too.

Config lives in `kittzo.toml`. Kittzo replaces `${NAME}` in it with the env var `NAME`. That keeps secrets out of git.

Kittzo looks for its files in the working directory. Start it from the repo root.

`RUST_LOG` sets the log filter.

## Tasks

All tasks live in `mise.toml`.

| Command | What it does |
| --- | --- |
| `mise run check` | Checks that `.env` has every key from `.env.example` |
| `mise run dev` | Runs `check`, then starts Kittzo |
| `mise run test` | Runs the tests |
| `mise run format` | Formats the code |

## Building

```sh
cargo build --release --locked
```

The binary does not embed the UI. Ship `resources/static/` next to it.

## Creating a release

CI builds releases. Push a `v*` tag and `.github/workflows/release.yml` publishes a GitHub release with Windows and Linux archives.

```sh
git tag v0.2.0
git push origin v0.2.0
```

## Versioning

Kittzo follows [semantic versioning](https://semver.org/).

Before 1.0.0, any minor release can break the API, the config or the database. Back up `kittzo.db` before upgrading. Copy its `-wal` and `-shm` files too.

From 1.0.0 on, breaking any of those needs a major release. Schema changes go in new migrations. Old migrations stay untouched.

## Architecture

```
src/
  domain/  types and business rules, no IO
  app/     services and background jobs
  infra/   adapters for the database, Discord, email, config and the tray
  api/     HTTP routes
  main.rs  wires everything together
```

`domain/` does not depend on any other layer.

Routes stay thin. They parse the request, call a service and map the result. Unit tests cover mainly `domain/` and `app/`.

## Scripts

> [!WARNING]
> Work in progress. Releases don't include this feature yet.

`scripts/` holds local Python scripts that process confirmed payments. A script can drive a bank's website, for example. The folder is gitignored. It encodes how the user pays and can hold a logged-in browser session.

- `scripts/run.py` is the entry point. It lists the scripts and runs the one you pick.
- The tray's "Run payment script" item runs it with `scripts/.venv`.
- Scripts talk to Kittzo over the HTTP API, never the database.

Recommended rules for a script that touches money:

- Validate a payment before acting on it. Skip it if anything looks wrong.
- Mark a payment paid only after it's externally confirmed. If unsure, leave it unpaid and check by hand.
- Keep credentials in the OS keyring.
- Support a dry run that stops before the final submit.
