#!/usr/bin/env bash
# Deploy BANJEE factory + register wallet Wasm on Stellar testnet.
# Usage: ./deploy-testnet.sh
set -euo pipefail
cd "$(dirname "$0")"

NETWORK="${STELLAR_NETWORK:-testnet}"
IDENTITY="${STELLAR_SOURCE:-zylo-deploy}"
DAILY_CAP="${ZYLO_DAILY_CAP:-0}"

stellar network use "$NETWORK" 2>/dev/null || true

if ! stellar keys address "$IDENTITY" &>/dev/null; then
  echo "Generating identity: $IDENTITY"
  stellar keys generate "$IDENTITY" --network "$NETWORK" --fund
else
  ADDR="$(stellar keys address "$IDENTITY")"
  echo "Using identity: $IDENTITY ($ADDR)"
  # Top up if friendbot helps
  stellar keys fund "$IDENTITY" --network "$NETWORK" 2>/dev/null || true
fi

ADMIN="$(stellar keys address "$IDENTITY")"
OPERATOR="${ZYLO_OPERATOR:-$ADMIN}"
AUTHORITY="${ZYLO_AUTHORITY:-$ADMIN}"
# Naira vault placeholder = admin until configured
NAIRA_VAULT="${ZYLO_NAIRA_VAULT:-$ADMIN}"

echo "Building contracts..."
stellar contract build

WALLET_WASM="target/wasm32v1-none/release/zylo_wallet.wasm"
FACTORY_WASM="target/wasm32v1-none/release/zylo_factory.wasm"

echo "Uploading wallet Wasm..."
WALLET_HASH="$(stellar contract upload \
  --wasm "$WALLET_WASM" \
  --source "$IDENTITY" \
  --network "$NETWORK")"
echo "Wallet Wasm hash: $WALLET_HASH"

echo "Deploying factory..."
FACTORY_ID="$(stellar contract deploy \
  --wasm "$FACTORY_WASM" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  --alias zylo-factory)"
echo "Factory: $FACTORY_ID"

echo "initialize..."
stellar contract invoke \
  --id "$FACTORY_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- \
  initialize \
  --admin "$ADMIN" \
  --operator "$OPERATOR" \
  --withdrawal_authority "$AUTHORITY" \
  --naira_vault "$NAIRA_VAULT" \
  --daily_cap "$DAILY_CAP"

echo "set_wallet_wasm..."
stellar contract invoke \
  --id "$FACTORY_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- \
  set_wallet_wasm \
  --wasm_hash "$WALLET_HASH"

# Persist for local use
cat > .env.deploy <<EOF
STELLAR_NETWORK=$NETWORK
STELLAR_FACTORY_CONTRACT_ID=$FACTORY_ID
STELLAR_WALLET_WASM_HASH=$WALLET_HASH
STELLAR_ADMIN=$ADMIN
STELLAR_OPERATOR=$OPERATOR
STELLAR_AUTHORITY=$AUTHORITY
STELLAR_NAIRA_VAULT=$NAIRA_VAULT
STELLAR_RPC_URL=https://soroban-testnet.stellar.org
STELLAR_NETWORK_PASSPHRASE=Test SDF Network ; September 2015
STELLAR_SOURCE=$IDENTITY
EOF

echo ""
echo "=== Deploy complete ==="
echo "Factory:     $FACTORY_ID"
echo "Wallet WASM: $WALLET_HASH"
echo "Admin/Op:    $ADMIN"
echo "Wrote:       $(pwd)/.env.deploy"
echo ""
echo "Backend env:"
echo "  WA_RAMP_STELLAR_FACTORY_ID=$FACTORY_ID"
echo "  WA_RAMP_STELLAR_RPC_URL=https://soroban-testnet.stellar.org"
echo "  WA_RAMP_STELLAR_NETWORK_PASSPHRASE=Test SDF Network ; September 2015"
echo "  WA_RAMP_STELLAR_OPERATOR_SECRET=<secret for $IDENTITY — stellar keys show $IDENTITY>"
