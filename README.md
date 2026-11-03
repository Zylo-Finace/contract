# BANJEE — Soroban Contracts

Soroban (Stellar smart-contract) workspace for **BANJEE** (product codename: **Zylo**). Two contracts give every user a deterministic on-chain Stellar deposit wallet, hold SAC assets (USDC, native XLM SAC), and sweep to a Naira vault for off-ramp settlement.

> Live: **https://www.usebanjee.com**
> **Full product + architecture doc (spans all three repos): [`PRODUCT.md`](PRODUCT.md)**
> Sister repos: [`Zylo-Finace/frontend`](https://github.com/Zylo-Finace/frontend) · [`Zylo-Finace/backend`](https://github.com/Zylo-Finace/backend)

---

## What BANJEE does

BANJEE is a consumer wallet for Nigerian users that bridges **USDC on Stellar** and **Nigerian Naira**. Users fund in Naira (via a dedicated NUBAN), convert to USDC at a locked rate, hold it on Stellar, send it to anyone with a Stellar address, sell it back to Naira, and pay bills — all from the web app or the WhatsApp bot, using one PIN.

This repo is the **on-chain custody layer**. It is parallel to (and interoperable with) the classic `G…`-account custody driven directly from the backend. A `ZyloWallet` deployed per user gives:

- a unique, deterministic Stellar deposit **contract address** (`C…`) that cannot change;
- operator-controlled spend while the user is custodial (WhatsApp-only user with no self-custody key);
- optional owner link to a Stellar key once the user wants self-custody;
- a sweep path to a Naira vault that lets the backend off-ramp held USDC to NGN.

### Where this repo fits

```
 ┌──────────────┐     ┌────────────────────┐     ┌───────────────────────────┐
 │  Frontend    │ ──► │  Backend API       │ ──► │  Stellar network          │
 │  / WhatsApp  │     │  (NestJS + Postgres)│    │  ┌─────────────────────┐  │
 └──────────────┘     │                    │     │  │ Classic G… accounts │  │
                      │  stellar-sdk       │     │  │ (custodial today)   │  │
                      │  stellar-sdk-ctr   │     │  └─────────────────────┘  │
                      │                    │     │  ┌─────────────────────┐  │
                      │  as operator       │     │  │ Soroban C… wallets  │  │
                      │                    │     │  │ ZyloFactory         │◄─┼── this repo
                      │                    │     │  │ ZyloWallet          │  │
                      │                    │     │  └─────────────────────┘  │
                      └────────────────────┘     └───────────────────────────┘
                                                              │
                                                              ▼
                                                     Naira vault (sweep for off-ramp)
```

---

## Workspace layout

| Crate | Role |
|-------|------|
| [`wallet/`](wallet) | **`ZyloWallet`** — the per-user contract. Holds SAC balances, authorises transfers, supports an operator spend path and (optionally) a linked Stellar owner key. Can sweep to the configured Naira vault. |
| [`factory/`](factory) | **`ZyloFactory`** — upgradable registry. Stores the current `ZyloWallet` Wasm hash, deploys one wallet per `user_id`, and (as admin) can push config changes down to deployed wallets. |

Top-level:

```
Cargo.toml              Workspace manifest (edition 2021, soroban-sdk pinned)
Cargo.lock              Reproducible build
deploy-testnet.sh       Build + upload + deploy + initialize on testnet
deploy-mainnet.sh       Same, with mainnet guards + prompts
wallet/                 ZyloWallet crate (src/lib.rs, Cargo.toml)
factory/                ZyloFactory crate (src/lib.rs, Cargo.toml)
```

Artifacts land at `target/wasm32v1-none/release/zylo_wallet.wasm` and `target/wasm32v1-none/release/zylo_factory.wasm`.

---

## Contracts in detail

### `ZyloWallet`

Per-user on-chain balance holder. Deployed once per registered user by the factory.

**State keys** (`DataKey` in `wallet/src/lib.rs`):

| Key | Meaning |
|-----|---------|
| `Factory` | Address of the parent factory (admin channel for config updates) |
| `Owner`, `OwnerLinked` | Optional Stellar user key; set once via `link_owner` |
| `Operator` | Backend address — spends on behalf of a custodial user after off-chain PIN |
| `WithdrawalAuthority` | Address allowed to approve off-ramp sweeps |
| `NairaVault` | Sweep destination for `send_to_naira_vault` |
| `DailyCap`, `SpentToday`, `SpendDay` | Daily spend cap enforcement, rolling by UTC day |

**Error codes** (`WalletError`):

| Code | Case |
|------|------|
| `NotInitialized` | Called before `__constructor` ran |
| `Unauthorized` | Spender is not an allowed principal |
| `InvalidAmount` | Zero or negative transfer |
| `NairaVaultNotSet` | Sweep attempted before configuration |
| `OwnerAlreadyLinked` | `link_owner` called twice |
| `ExceedsDailyCap` | Would push rolling 24h spend over the configured cap |

**Spend paths**:

- `transfer(spender, token, to, amount)` — `spender` must be `owner` (if linked) or `operator`; cap enforced.
- `send_to_naira_vault(spender, token, amount)` — `spender` must be `operator` or `withdrawal_authority`; only allowed when `NairaVault` is set to a real address.

**Config lifecycle**: on deploy, `__constructor` takes the factory + operator + withdrawal authority + a Naira-vault placeholder (equal to the factory address) + the daily cap. The factory, as admin, calls `set_operator`, `set_withdrawal_authority`, `set_naira_vault`, `set_daily_cap`, and `link_owner` afterwards.

### `ZyloFactory`

Upgradable registry. One per deployment (testnet, mainnet).

**State keys**:

| Key | Meaning |
|-----|---------|
| `Admin` | Can `upgrade` the factory and set the wallet Wasm hash |
| `Operator` | Default operator pushed into new wallets |
| `WithdrawalAuthority` | Default withdrawal authority pushed into new wallets |
| `NairaVault` | Default vault address for new wallets |
| `DailyCap` | Default daily cap for new wallets |
| `WalletWasm` | Currently active `ZyloWallet` Wasm hash |
| `UserWallet(user_id)` | Lookup: `user_id` → wallet contract address |
| `WalletUser(address)` | Reverse lookup |

**Error codes** (`FactoryError`): `AlreadyInitialized`, `NotInitialized`, `Unauthorized`, `AlreadyRegistered`, `WalletWasmNotSet`, `UnknownWallet`, `InvalidAmount`.

**Core entrypoints**:

- `initialize(admin, operator, withdrawal_authority, naira_vault_placeholder, daily_cap)` — one-time.
- `set_wallet_wasm(hash)` — admin registers the uploaded `ZyloWallet` Wasm.
- `register(user_id) -> Address` — deploys a wallet keyed on `user_id`, returns the contract address. Idempotent per `user_id`.
- `wallet_of(user_id) -> Address` / `user_of(address) -> Bytes` — lookups.
- `upgrade(new_wasm_hash)` — admin rotates the factory binary.
- Config pushers (`set_operator`, `set_withdrawal_authority`, `set_naira_vault`, `set_daily_cap`) that forward to a target wallet via the `WalletClient` trait.

`user_id` should be a stable identifier the backend can re-derive at any time — for BANJEE that is **SHA-256 of the E.164 phone**, passed as `Bytes`. This makes `register` idempotent across backend restarts and migrations: same user, same wallet.

---

## Auth model (who can do what)

| Principal | Can do |
|-----------|--------|
| **Admin** (factory only) | Upgrade factory, set wallet Wasm hash, push defaults, push config to deployed wallets |
| **Operator** (backend) | Call `register`, spend via `transfer(spender=operator, …)` after an off-chain PIN check |
| **Owner** (optional, linked via `link_owner`) | Spend from their wallet with their Stellar key |
| **Withdrawal Authority** | Approve `send_to_naira_vault` off-ramp sweeps |
| **Naira Vault** | Passive receiver of swept tokens for fiat settlement |

Until `link_owner` is called, the wallet is **custodial** — only the operator can spend. That matches the WhatsApp-first user who has no Stellar key of their own. When the user later wants self-custody, the backend calls `link_owner` once with their key and from that point both the operator **and** the owner can spend. (If you want operator-free self-custody, rotate operator to a null address via the factory's `set_operator` push.)

---

## Prerequisites

- Rust toolchain (`rustup install stable`)
- WASM target: `rustup target add wasm32v1-none`
- Stellar CLI: `cargo install --locked stellar-cli` or install the prebuilt binary.
  - Verify: `stellar --version`
- A funded testnet source account (CLI: `stellar keys generate … && stellar keys fund …`).

> **Soroban SDK version drift.** This repo pins `soroban-sdk` via `Cargo.toml`. If `cargo test` fails on `ed25519-dalek` or `rand_core`, pin the transitive:
> ```bash
> cargo update -p ed25519-dalek@3.0.0 --precise 2.2.0
> ```

---

## Build

```bash
# From this directory
stellar contract build
# Artifacts:
#   target/wasm32v1-none/release/zylo_wallet.wasm
#   target/wasm32v1-none/release/zylo_factory.wasm
```

`cargo build` also works, but `stellar contract build` applies the right release flags and optimizations for on-chain footprint.

### Measure Wasm size

```bash
ls -l target/wasm32v1-none/release/*.wasm
```

Keeping wallet < ~40 KB and factory < ~60 KB keeps deploy fees predictable.

---

## Test

```bash
# Build first — factory tests include the wallet wasm by reference
stellar contract build
cargo test
```

If tests fail on `ed25519-dalek` / `rand_core` versions, apply the pin shown in Prerequisites and re-run.

Tests live in each crate under `#[cfg(test)]` modules inside `src/lib.rs`. They use `soroban_sdk::testutils` to spin up an environment, mint SAC balances, deploy the wallet Wasm, and drive full flows (register → deposit SAC → transfer → sweep).

---

## Deploy to testnet

A scripted one-liner lives in [`deploy-testnet.sh`](deploy-testnet.sh); the steps it performs are:

1. **Build** the two contracts (`stellar contract build`).
2. **Upload** `zylo_wallet.wasm` to the network; capture its hash.
3. **Install** `zylo_factory.wasm` and **deploy** it with
   `initialize(admin, operator, withdrawal_authority, naira_vault_placeholder, daily_cap)`.
4. **Register the wallet Wasm** on the factory: `set_wallet_wasm(wallet_hash)`.
5. **Backend registration** — the NestJS backend calls `register(user_id)` per new user; the returned address becomes the user's permanent Stellar deposit address.

Example (manual equivalent):

```bash
# 1. Upload wallet
WALLET_HASH=$(stellar contract upload \
  --network testnet --source "$SRC" \
  --wasm target/wasm32v1-none/release/zylo_wallet.wasm)

# 2. Deploy factory with initializer
FACTORY_ID=$(stellar contract deploy \
  --network testnet --source "$SRC" \
  --wasm target/wasm32v1-none/release/zylo_factory.wasm \
  -- initialize \
    --admin "$ADMIN" \
    --operator "$OPERATOR" \
    --withdrawal_authority "$WITHDRAWAL" \
    --naira_vault "$NAIRA_VAULT_PLACEHOLDER" \
    --daily_cap "$DAILY_CAP")

# 3. Register the wallet Wasm on the factory
stellar contract invoke \
  --network testnet --source "$SRC" --id "$FACTORY_ID" \
  -- set_wallet_wasm --wallet_hash "$WALLET_HASH"

# 4. Register a user (backend)
stellar contract invoke \
  --network testnet --source "$SRC" --id "$FACTORY_ID" \
  -- register --user_id "$(printf '%s' '+2348000000000' | sha256sum | cut -d' ' -f1)"
```

### Testnet vs mainnet

[`deploy-mainnet.sh`](deploy-mainnet.sh) adds confirmation prompts, uses the mainnet RPC, and expects real admin / operator / withdrawal / vault addresses. **Do not run it from a dev laptop** — it should run from the deploy host with the production signer only. The mainnet daily cap and Naira vault must be reviewed by two people before `set_wallet_wasm` / `initialize` go out.

---

## Integration with the backend

The NestJS backend (`backend/src/modules/ramp/ramp.stellar-wallet.ts`) is the only client that talks to these contracts in normal operation. It:

- holds the **operator** key in `RAMP_STELLAR_OPERATOR_SK` (encrypted at rest with `ENCRYPTION_KEY`);
- derives `user_id = SHA-256(phone_e164)` so that `register` is idempotent;
- stores the returned wallet address on the user row as the Stellar deposit address;
- builds Soroban transactions for `transfer` and `send_to_naira_vault`;
- polls the network for new SAC deposits into each registered wallet and credits the in-app USDC balance;
- handles re-tries + idempotency the same way as the classic `G…` custody path.

For local development you can point the backend at testnet by setting the factory id and operator/admin/withdrawal keys in the backend's `.env` (see `.env.example` in the backend repo for the full list).

---

## Upgrade path (classic → Soroban)

BANJEE custody is currently **classic `G…` accounts** driven from the backend, which is production-proven and cheap. Soroban `C…` wallets are the strategic path because they:

- give every user a deterministic contract address that can enforce a daily cap on-chain;
- make linked self-custody a one-call operation (`link_owner`);
- let the Naira vault sweep be a verifiable on-chain transfer rather than a backend accounting step.

The migration plan, tracked in issues on both repos:

1. **Shadow-register** — on every new user, also call `register(user_id)` and store the Soroban address alongside the classic one.
2. **Dual-deposit detection** — the backend watches both addresses and credits the same in-app balance; receipts show which rail was used.
3. **Flip send default** to Soroban for new users; old users keep classic until they opt in.
4. **Sweep + reconcile** once the Naira vault path is live on mainnet.

---

## Contributing

Issues are scoped to this repo and labeled (`contract`, `good-first-issue`, `test`, `docs`, `stellar`). Each issue states acceptance criteria and links the files involved.

1. Fork, branch from `main`.
2. Make the change — tests go in the same `#[cfg(test)]` module.
3. `stellar contract build && cargo test` must pass.
4. Open a PR; link the issue; include the `stellar contract invoke` transcript for anything that changes an entrypoint signature.
5. Breaking changes to `ZyloWallet` require a factory upgrade plan — call it out in the PR.

Secrets never go in commits. The `.env`, `.env.deploy`, and `.env.mainnet` files are ignored by design.

---

## Related documents

- [Backend](https://github.com/Zylo-Finace/backend) — API, WhatsApp bot, custody orchestration
- [Frontend](https://github.com/Zylo-Finace/frontend) — web wallet
- [Soroban docs](https://developers.stellar.org/docs/build/smart-contracts) — upstream SDK + CLI references

## License

UNLICENSED — all rights reserved. Contact the maintainers for commercial use.
