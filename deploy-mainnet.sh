#!/usr/bin/env bash
# Deploy BANJEE factory + wallet Wasm to Stellar MAINNET.
# Requires: STELLAR_SECRET_KEY (S...) funded with XLM on mainnet.
# Usage:
#   export STELLAR_SECRET_KEY=S...
#   ./deploy-mainnet.sh
set -euo pipefail
cd "$(dirname "$0")"

NETWORK="mainnet"
RPC_URL="${STELLAR_RPC_URL:-https://mainnet.sorobanrpc.com}"
PASSPHRASE="${STELLAR_NETWORK_PASSPHRASE:-Public Global Stellar Network ; September 2015}"
DAILY_CAP="${ZYLO_DAILY_CAP:-0}"

if [[ -z "${STELLAR_SECRET_KEY:-}" ]]; then
  echo "Set STELLAR_SECRET_KEY to a funded mainnet secret (S...)"
  exit 1
fi

SOURCE="$STELLAR_SECRET_KEY"

# Resolve admin address without persisting the secret to disk via CLI identity
ADMIN="$(node -e "const {Keypair}=require('../backend/node_modules/@stellar/stellar-sdk'); console.log(Keypair.fromSecret(process.env.STELLAR_SECRET_KEY).publicKey())")"
OPERATOR="${ZYLO_OPERATOR:-$ADMIN}"
AUTHORITY="${ZYLO_AUTHORITY:-$ADMIN}"
NAIRA_VAULT="${ZYLO_NAIRA_VAULT:-$ADMIN}"

echo "Network:   MAINNET"
echo "Admin/Op:  $ADMIN"
echo "Building..."
stellar contract build

WALLET_WASM="target/wasm32v1-none/release/zylo_wallet.wasm"
FACTORY_WASM="target/wasm32v1-none/release/zylo_factory.wasm"

echo "Uploading wallet Wasm..."
WALLET_HASH="$(stellar contract upload \
  --wasm "$WALLET_WASM" \
  --source "$SOURCE" \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE")"
echo "Wallet Wasm hash: $WALLET_HASH"

echo "Deploying factory..."
FACTORY_ID="$(stellar contract deploy \
  --wasm "$FACTORY_WASM" \
  --source "$SOURCE" \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE" \
  --alias zylo-factory-mainnet)"
echo "Factory: $FACTORY_ID"

echo "initialize..."
stellar contract invoke \
  --id "$FACTORY_ID" \
  --source "$SOURCE" \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE" \
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
  --source "$SOURCE" \
  --rpc-url "$RPC_URL" \
  --network-passphrase "$PASSPHRASE" \
  -- \
  set_wallet_wasm \
  --wasm_hash "$WALLET_HASH"

cat > .env.mainnet <<EOF
STELLAR_NETWORK=mainnet
STELLAR_FACTORY_CONTRACT_ID=$FACTORY_ID
STELLAR_WALLET_WASM_HASH=$WALLET_HASH
STELLAR_ADMIN=$ADMIN
STELLAR_OPERATOR=$OPERATOR
STELLAR_AUTHORITY=$AUTHORITY
STELLAR_NAIRA_VAULT=$NAIRA_VAULT
STELLAR_RPC_URL=$RPC_URL
STELLAR_NETWORK_PASSPHRASE=$PASSPHRASE
EOF

echo ""
echo "=== MAINNET deploy complete ==="
echo "Factory:     $FACTORY_ID"
echo "Explorer:    https://stellar.expert/explorer/public/contract/$FACTORY_ID"
echo "Wallet WASM: $WALLET_HASH"
echo "Admin/Op:    $ADMIN"
echo "Wrote:       $(pwd)/.env.mainnet"
