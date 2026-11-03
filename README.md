# BANJEE Soroban contracts

Per-user wallets on Stellar: register → unique deposit contract → hold / transfer SAC tokens (USDC, native XLM SAC) → later Naira off-ramp.

## Layout

| Crate | Role |
|-------|------|
| `wallet/` | `ZyloWallet` — holds funds, transfer, operator spend, Naira sweep stub |
| `factory/` | `ZyloFactory` — upgradable registry; deploys a wallet per `user_id` |

## Build

```bash
# From this directory
stellar contract build
# Artifacts: target/wasm32v1-none/release/zylo_wallet.wasm
#            target/wasm32v1-none/release/zylo_factory.wasm
```

## Test

```bash
stellar contract build   # factory tests include the wallet wasm
# If `cargo test` fails on ed25519-dalek / rand_core, pin:
#   cargo update -p ed25519-dalek@3.0.0 --precise 2.2.0
cargo test
```

## Auth model

- **Admin** — upgrade factory, set Wasm hash / defaults, push config to wallets
- **Operator** — backend (WhatsApp); `register`, spend via `transfer(spender=operator, …)` after PIN off-chain
- **Owner** (optional) — linked Stellar key; can spend once `link_owner` is called
- **Naira vault** — configure later; `send_to_naira_vault` sweeps tokens for fiat settlement

## Typical deploy (testnet)

1. Upload `zylo_wallet.wasm`, note hash  
2. Deploy `zylo_factory.wasm`, call `initialize(admin, operator, authority, naira_placeholder, daily_cap)`  
3. `set_wallet_wasm(wallet_hash)`  
4. Backend: `register(user_id)` → store returned address as deposit address  

`user_id` should be stable (e.g. SHA-256 of E.164 phone) as `Bytes`.
