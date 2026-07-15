# AGENTS.md

Guidance for AI agents working in this repository.

## What this is

`eth-dca-bot` is a Rust service that runs a scheduled Dollar-Cost-Averaging (DCA)
strategy on Binance. It periodically buys ETH (and optionally BTC) with EUR/USDC,
records every purchase, optionally withdraws to cold storage when a threshold is
hit, and mirrors purchases into MongoDB and Notion.

## Tech stack

- **Language**: Rust, edition **2024** (`Cargo.toml`). Use the latest stable toolchain.
- **Async runtime**: `tokio` (full features). All I/O is `async`.
- **HTTP**: `reqwest` with `rustls-tls` (no OpenSSL — keep `default-features = false`).
- **Scheduling**: `tokio-cron-scheduler` for the running jobs; the `cron` crate is
  used separately to compute "next execution" display strings.
- **Persistence**: `mongodb` + `bson` for purchase history and stats. `sqlx`
  (sqlite) is a declared dependency for potential future use.
- **External integrations**: Binance REST API (custom client in `binance.rs`,
  HMAC-SHA256 request signing via `hmac`/`sha2`/`hex`); Notion via `notion-client`.
- **Money math**: `rust_decimal` / `rust_decimal_macros` everywhere — **never use
  `f64` for amounts, prices, balances, or fees.**
- **Dates**: `chrono` + `chrono-tz`. Schedules are timezone-aware.
- **Config**: `dotenv` loads a `.env` file; all config comes from env vars.
- **Logging**: `tracing` + `tracing-subscriber` (structured logs, not `println!`).
- **Errors**: `anyhow::Result` for fallible functions.

## Layout

Both a binary and a library target build from `src/` (`lib.rs` re-exports the
modules; `main.rs` redeclares them as `mod` for the binary).

```
src/
├── main.rs               # Entry point: load config, build scheduler, register jobs
├── lib.rs                # Library module exports
├── config.rs             # Config structs, env parsing, defaults
├── binance.rs            # Binance REST client, request signing, trading + withdrawals
├── dca.rs                # DcaTrader: core buy / startup catch-up / withdrawal logic
├── dca_stats_mongo.rs    # MongoDB persistence and statistics
├── notion_integration.rs # Notion API mirroring of purchases
├── date_utils.rs         # Date/time helpers (has unit tests)
└── bin/
    └── replace_purchase.rs  # Standalone maintenance binary (`cargo run --bin replace_purchase`)
```

## Architectural conventions

- **Per-asset workflows**: ETH is the primary workflow on the flat fields of
  `Config`. Additional assets (currently BTC) are modeled as
  `Option<AssetDcaConfig>` bundling their own `trading`/`schedule`/`notion`/
  `withdrawal` config. Each asset gets its **own MongoDB collection** and **own
  Notion database** so stats never mix. When adding an asset, follow this pattern
  rather than special-casing BTC.
- **Config is fully env-driven**. New tunables are env vars parsed in `config.rs`,
  with sensible defaults and a matching entry in `.env.example` / `.env.prod.example`
  and the README. Optional features default to off (e.g. `BTC_DCA_ENABLED=false`).
- **`DcaTrader` is the orchestration unit** (`dca.rs`): it owns the Binance client,
  stats store, and Notion client, and exposes `execute_dca_purchase`,
  `check_and_execute_startup_dca` (missed-run catch-up), and
  `check_and_execute_withdrawal`.
- **Safety first**: respect `min_balance_usdc` buffers and withdrawal thresholds;
  preserve the existing validation layers before any trade or withdrawal.
- **Secrets** (API keys, Notion tokens, wallet addresses) come only from env —
  never hardcode them or commit a real `.env`.

## Verification commands

Run before considering a change complete:

```bash
cargo fmt --all                 # format
cargo clippy --all-targets      # lint (treat warnings seriously)
cargo build                     # compile (use --release for production builds)
cargo test                      # unit tests (date_utils has coverage)
```

To run the bot locally you need a populated `.env` and a running MongoDB:

```bash
docker compose up -d            # start MongoDB
cargo run                       # run the bot
```

## Gotchas

- This is a **trading bot that moves real money**. Be conservative — `execute_dca_purchase`
  and `check_and_execute_withdrawal` place live orders and on-chain withdrawals against
  Binance. Do not run them casually; test against small amounts.
- Keep `reqwest` on `rustls-tls`; the project deliberately avoids OpenSSL (relevant
  for cross-compilation / Termux builds, see the repo scripts).
- Cron expressions here use the 7-field format (with seconds), and schedules are
  evaluated in the configured `TIMEZONE`.
