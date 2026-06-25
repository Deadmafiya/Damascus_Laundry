# dl-app.env — Operator Environment File

> **Status:** v1.0, 2026-06-21. Author: Product / Docs Lead.
> **Created from:** DAM-40 P0-2 (untranslated placeholders).
>
> The v2.0 plan commits to `EnvironmentFile=/home/<your-user>/.damascus/dl-app.env`
> for the Phase 3 systemd unit. This file is the operator's
> reference for what goes in it. Copy it to
> `~/.damascus/dl-app.env`, fill in the `<...>` slots, and
> the runbook commands become copy-pastable.

---

## Template

```bash
# ~/.damascus/dl-app.env
# Loaded by the dl-app systemd unit (Phase 3) and sourced
# manually during Phases 1c/1d. NEVER commit this file. NEVER
# share this file. Permissions: chmod 600.

# --- Operator identity (substitute before saving) ---
DL_OPERATOR_HOME=/home/<your-user>
DL_DAMASCUS_HOME=${DL_OPERATOR_HOME}/.damascus

# --- Live mode: devnet | mainnet-paper | mainnet ---
# Required. Empty = Refused (engine won't start).
DL_LIVE_MODE=mainnet-paper

# --- Hot wallet keyfiles (one per network) ---
# Generated once via `dl-signer generate` (see live-runbook §2).
DL_DEVNET_KEYFILE=${DL_DAMASCUS_HOME}/devnet-keyfile.json
DL_MAINNET_KEYFILE=${DL_DAMASCUS_HOME}/mainnet-keyfile.json

# --- Passphrase (Argon2id-derived key). Held in env, never on disk. ---
# Pulled from your password manager at boot; do not store in this file.
# DL_SIGNER_PASSPHRASE=...      # set in your shell, not here

# --- RPC endpoints ---
DL_LIVE_WS_URL=wss://api.mainnet-beta.solana.com
DL_LIVE_HTTP_URL=https://api.mainnet-beta.solana.com

# --- Jito tip account (the validator that receives bundle tips) ---
# Refreshed on first run; this is the default at the time of writing.
DL_JITO_TIP_ACCOUNT=$(curl -s https://mainnet.block-engine.jito.wtf/api/v1/getTipAccounts | jq -r '.[0]')

# --- Deployed dl-assert program ID (per network) ---
DL_ASSERT_DEVNET_PROGRAM_ID=<DEVNET_PROGRAM_ID>
DL_ASSERT_MAINNET_PROGRAM_ID=<MAINNET_PROGRAM_ID>
```

## How to use it

**Phase 1c / 1d (no systemd yet):** `source ~/.damascus/dl-app.env`
before running any `cargo run` or `dl-app` command. Then the
runbook's `cargo run --release -p dl-app -- run ...` lines
work as written (the env vars are picked up by `dl-app`).

**Phase 3 (systemd):** the unit's `EnvironmentFile=` directive
points to this file. The operator does not need to `source`
it manually; systemd does.

**Phase 1a / 1b (devnet):** same file, with
`DL_LIVE_MODE=devnet` and `DL_DEVNET_KEYFILE` active.
`DL_MAINNET_KEYFILE` is set but unused.

## What NOT to put in this file

- The passphrase. The passphrase is held in your shell env or
  your password manager's session, never on disk.
- Any mainnet keyfile on a devnet-only runbook step (and
  vice versa). The env var is the right level of separation.
- The cold wallet pubkey as a default — copy it from
  `dl-signer drain-to` output each session.

## Updating the template

This file lives in the repo as `docs/dl-app.env.example` (the
canonical, placeholders-filled version). The operator's
`~/.damascus/dl-app.env` is the live version. When a new env
var is needed, edit `docs/dl-app.env.example` first, then
copy, fill, and save to `~/.damascus/dl-app.env`. Do not edit
`~/.damascus/dl-app.env` directly without updating the
example file; the example is the source of truth.
