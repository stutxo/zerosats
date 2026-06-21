#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CIPHERA_DIR="${CIPHERA_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"
CIPHERA_WORKTREE="${CIPHERA_WORKTREE:-$(git -C "$CIPHERA_DIR" rev-parse --show-toplevel)}"
WORKSPACE_PARENT="$(cd "$CIPHERA_WORKTREE/.." && pwd)"

MERCURY_LAB_DIR="${MERCURY_LAB_DIR:-"$WORKSPACE_PARENT/mercury-lab"}"
DEMO_DATA_DIR="${DEMO_DATA_DIR:-"$WORKSPACE_PARENT/ciphera-demo-data"}"
ZEROSATS_WALLET_DIR="${ZEROSATS_WALLET_DIR:-"$DEMO_DATA_DIR/zerosats-wallets"}"
DEMO_DIR="${DEMO_DIR:-"$DEMO_DATA_DIR/runs/mercury-ciphera-latch-demo-$(date +%s)"}"
SESSION="${SESSION:-mercury-latch-swap}"

mkdir -p "$ZEROSATS_WALLET_DIR" "$DEMO_DIR/mercury-wallet"

RUNNER="$DEMO_DIR/run-demo.sh"
cat > "$RUNNER" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

exec > >(tee -a "$DEMO_DIR/demo.log") 2>&1

unset STATECHAIN_ID

log() {
  printf '\n[%s] %s\n' "$(date +%H:%M:%S)" "$*"
}

section() {
  printf '\n\n========== %s ==========\n' "$*"
}

on_error() {
  local rc=$?
  echo
  echo "Demo failed with exit code $rc."
  echo "Failed step: ${CURRENT_STEP:-setup}"
  echo "Run directory: $DEMO_DIR"
  if [ -n "${SELLER_STATE:-}" ]; then
    echo "Seller state: $SELLER_STATE"
  fi
  if [ -n "${BUYER_STATE:-}" ]; then
    echo "Buyer state: $BUYER_STATE"
  fi
  if [ -n "${BUYER_OUT:-}" ] && [ -f "$BUYER_OUT-refund.json" ]; then
    echo "Refund note: $BUYER_OUT-refund.json"
    if [ -n "${WALLETS:-}" ] && [ -n "${CIPHERA_CLI:-}" ]; then
      echo "After the refund timelock, recover from the Zerosats wallet dir with:"
      printf '  cd %q && %q --name %q --host %q --chain %q escrow-refund --note %q\n' \
        "$WALLETS" "$CIPHERA_CLI" "$FUNDER_ZEROSATS_WALLET" "$CIPHERA_HOST" \
        "$CITREA_CHAIN" "$BUYER_OUT-refund.json"
    fi
  fi
  echo "Do not run seller claim unless ${SWAP_JSON:-swap.json} contains buyer_receive for STATECHAIN_ID=${STATECHAIN_ID:-unknown}."
  exit "$rc"
}
trap on_error ERR

install_barretenberg() {
  export BB_PATH="$DEMO_DATA_DIR/barretenberg"
  mkdir -p "$BB_PATH"

  if [ -x "$BB_PATH/bb" ]; then
    "$BB_PATH/bb" --version
    return
  fi

  local os arch archive
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:arm64) archive="$CIPHERA_DIR/web/binaries/darwin/barretenberg-arm64-darwin.tar.gz" ;;
    Darwin:x86_64) archive="$CIPHERA_DIR/web/binaries/darwin/barretenberg-amd64-darwin.tar.gz" ;;
    Linux:aarch64|Linux:arm64) archive="$CIPHERA_DIR/web/binaries/linux64/barretenberg-arm64-linux.tar.gz" ;;
    Linux:x86_64) archive="$CIPHERA_DIR/web/binaries/linux64/barretenberg-amd64-linux.tar.gz" ;;
    *)
      echo "No bundled Barretenberg binary for $os/$arch" >&2
      exit 1
      ;;
  esac

  tar -xzf "$archive" -C "$BB_PATH"
  chmod +x "$BB_PATH/bb"
  xattr -dr com.apple.quarantine "$BB_PATH" 2>/dev/null || true
  xattr -dr com.apple.provenance "$BB_PATH" 2>/dev/null || true
  "$BB_PATH/bb" --version
}

inspect_swap_transcript() {
  if [ ! -f "$SWAP_JSON" ]; then
    return
  fi

  log "operator inspects swap transcript: $(basename "$SWAP_JSON")"
  "$SWAP" inspect-swap --swap "$SWAP_JSON" | jq -r '
    "  transcript_version: \(.version)",
    "  statechain_id: " + (.statechain_id // "unknown"),
    "  completed_sections: " + (.completed_sections | join(", ")),
    "  next: " + (.next_command // "none"),
    "  safe_contents: " + (.safe_contents | join(", "))
  '
}

print_state_status() {
  local label="$1"
  local state_file="$2"

  if [ ! -f "$state_file" ]; then
    return
  fi

  "$SWAP" status --state "$state_file" | jq -r --arg label "$label" '
    "  local_next[" + $label + "]: " + (.next_local_command // "none"),
    (if .waiting_for then "  waiting_for[" + $label + "]: " + .waiting_for.file + " via " + .waiting_for.command else empty end),
    (if (.available_exports | length) > 0 then "  exports[" + $label + "]: " + ([.available_exports[].file] | join(", ")) else empty end)
  '
}

print_step_status() {
  case "$1" in
    seller-*) print_state_status seller "$SELLER_STATE" ;;
    buyer-*) print_state_status buyer "$BUYER_STATE" ;;
  esac
}

json_step() {
  local name="$1"
  local title="$2"
  shift
  shift
  CURRENT_STEP="$name"
  export CURRENT_STEP

  SWAP_STEP=$((SWAP_STEP + 1))
  log "Swap step $SWAP_STEP/$TOTAL_SWAP_STEPS: $title"

  "$@" >/dev/null

  print_step_summary "$name"
  print_step_status "$name"
  printf '  status: ok\n'
}

print_step_summary() {
  local name="$1"
  case "$name" in
    seller-init)
      jq -r '"  summary: seller statechain_id=\(.terms.statechain_id) amount_sat=\(.terms.amount_sat) mercury_amount_sat=\(.terms.mercury_amount_sat)"' "$SELLER_STATE"
      printf '  state: %s\n' "$SELLER_STATE"
      ;;
    seller-offer)
      jq -r '"  transcript: seller_offer batch_id=\(.seller_offer.create_hash.batch_id)"' "$SWAP_JSON"
      printf '  public: %s\n' "$SWAP_JSON"
      printf '  state: %s\n' "$SELLER_STATE"
      ;;
    buyer-accept)
      jq -r '"  transcript: buyer_addresses mercury=\(.buyer_addresses.mercury_address.mercury_transfer_address) refund=\(.buyer_addresses.zerosats_address.refund_address)"' "$SWAP_JSON"
      printf '  public: %s\n' "$SWAP_JSON"
      printf '  state: %s\n' "$BUYER_STATE"
      ;;
    seller-template)
      jq -r '"  transcript: seller_template commitment=\(.seller_template.template.latch_commitment)"' "$SWAP_JSON"
      printf '  public: %s\n' "$SWAP_JSON"
      printf '  state: %s\n' "$SELLER_STATE"
      ;;
    buyer-funding)
      jq -r '"  transcript: buyer_funding tx=\(.buyer_funding.fund.lock_transaction.txn_hash)"' "$SWAP_JSON"
      printf '  public: %s\n' "$SWAP_JSON"
      printf '  state: %s\n' "$BUYER_STATE"
      ;;
    seller-release)
      jq -r '"  transcript: seller_unlock unlocked=\(.seller_unlock.unlock.unlocked)"' "$SWAP_JSON"
      printf '  public: %s\n' "$SWAP_JSON"
      printf '  state: %s\n' "$SELLER_STATE"
      ;;
    buyer-receipt)
      jq -r '"  transcript: buyer_receive received_amount_sat=\(.buyer_receive.receive.received_mercury_amount_sat)"' "$SWAP_JSON"
      printf '  public: %s\n' "$SWAP_JSON"
      printf '  state: %s\n' "$BUYER_STATE"
      ;;
    seller-claim)
      jq -r '"  summary: claim_tx=\(.claim.claim_transaction.txn_hash) balance_sat=\(.claim.balance_sat) ticker=\(.claim.ticker)"' "$SELLER_STATE"
      printf '  state: %s\n' "$SELLER_STATE"
      ;;
  esac
}

ensure_wallet() {
  local wallet="$1"
  if [ ! -f "$ZEROSATS_WALLET_DIR/$wallet.json" ]; then
    "$CIPHERA_CLI" \
      --name "$wallet" \
      --host "$CIPHERA_HOST" \
      --chain "$CITREA_CHAIN" \
      create
  fi
}

sync_wallet() {
  local wallet="$1"
  "$CIPHERA_CLI" \
    --name "$wallet" \
    --host "$CIPHERA_HOST" \
    --chain "$CITREA_CHAIN" \
    sync
}

wallet_balance_wei() {
  local wallet="$1"
  jq -r '.balance // 0' "$ZEROSATS_WALLET_DIR/$wallet.json"
}

require_private_key() {
  if [ -n "${CITREA_PRIVATE_KEY:-}" ]; then
    return
  fi

  printf "Citrea private key for mint funding: "
  stty -echo
  read CITREA_PRIVATE_KEY
  stty echo
  printf "\n"
  export CITREA_PRIVATE_KEY
}

log "demo run directory: $DEMO_DIR"
mkdir -p "$ZEROSATS_WALLET_DIR" "$DEMO_DIR/mercury-wallet" "$DEMO_DIR/seller" "$DEMO_DIR/buyer"

export SWAP_DIR="$CIPHERA_DIR/pkg/mercury-latch-swap"
export SWAP_MANIFEST="$SWAP_DIR/Cargo.toml"
export SWAP="$SWAP_DIR/target/debug/mercury-latch-swap"
export CIPHERA_CLI="$CIPHERA_DIR/target/release/ciphera-cli"
export MERCURY_CLIENT_MANIFEST="$MERCURY_LAB_DIR/clients/apps/rust/Cargo.toml"
export MERCURY_CLIENT="$MERCURY_LAB_DIR/target/debug/client-rust"
export SETTINGS="$DEMO_DIR/mercury-wallet/Settings.toml"
export ML="$DEMO_DIR/ml"
export SELLER_OUT="$DEMO_DIR/seller/mercury-ciphera-swap"
export BUYER_OUT="$DEMO_DIR/buyer/mercury-ciphera-swap"
export SWAP_JSON="$DEMO_DIR/swap.json"
export WALLETS="$ZEROSATS_WALLET_DIR"
export MERCURY_SETTINGS_FILE="$SETTINGS"
export SELLER_STATE="$SELLER_OUT-state.json"
export BUYER_STATE="$BUYER_OUT-state.json"
export TOTAL_SWAP_STEPS=8
export SWAP_STEP=0

export SELLER_WALLET="${SELLER_WALLET:-seller}"
export BUYER_WALLET="${BUYER_WALLET:-buyer}"
export OWNER_MERCURY_WALLET="${OWNER_MERCURY_WALLET:-$SELLER_WALLET}"
export RECEIVER_MERCURY_WALLET="${RECEIVER_MERCURY_WALLET:-$BUYER_WALLET}"
export FUNDER_ZEROSATS_WALLET="${FUNDER_ZEROSATS_WALLET:-$BUYER_WALLET}"
export CLAIMER_ZEROSATS_WALLET="${CLAIMER_ZEROSATS_WALLET:-$SELLER_WALLET}"

export MERCURY_URL="${MERCURY_URL:-http://204.168.231.217:28000}"
export MERCURY_ELECTRUM_SERVER="${MERCURY_ELECTRUM_SERVER:-tcp://62.238.20.16:50001}"
export MERCURY_FAUCET_URL="${MERCURY_FAUCET_URL:-http://62.238.20.16:3000}"
export MERCURY_ADMIN_URL="${MERCURY_ADMIN_URL:-http://62.238.20.16:3030}"

export CIPHERA_HOST="${CIPHERA_HOST:-https://ciphera.satsbridge.com}"
export CITREA_CHAIN="${CITREA_CHAIN:-5115}"
export CITREA_RPC="${CITREA_RPC:-https://rpc.testnet.citrea.xyz}"
export BTC_EXPLORER="${BTC_EXPLORER:-https://mempool.space}"

export MERCURY_AMOUNT_SAT="${MERCURY_AMOUNT_SAT:-1000}"
export AMOUNT_SAT="${AMOUNT_SAT:-1000}"
export REFUND_BLOCKS="${REFUND_BLOCKS:-2}"

section "Setup"
log "installing Barretenberg"
install_barretenberg

log "building swap binary"
MERCURY_PATCH_CONFIG="patch.\"https://github.com/zerosats/mercury-lab.git\".mercuryrustlib.path=\"$MERCURY_LAB_DIR/clients/libs/rust\""
cargo build --manifest-path "$SWAP_MANIFEST" --config "$MERCURY_PATCH_CONFIG"

log "building ciphera-cli"
(
  cd "$CIPHERA_DIR"
  cargo build -p cli --bin ciphera-cli --release
)

log "building Mercury client"
cargo build --manifest-path "$MERCURY_CLIENT_MANIFEST"

cat > "$SETTINGS" <<SETTINGS_EOF
statechain_entity = "$MERCURY_URL"
chain_backend = "electrum"
electrum_server = "$MERCURY_ELECTRUM_SERVER"
electrum_type = "electrs"
network = "signet"
fee_rate_tolerance = 5
database_file = "$DEMO_DIR/mercury-wallet/wallet.db"
confirmation_target = 2
max_fee_rate = 100
SETTINGS_EOF

cat > "$ML" <<ML_EOF
#!/usr/bin/env bash
set -euo pipefail
cd "$DEMO_DIR/mercury-wallet"
exec env ML_SETTINGS_FILE="$SETTINGS" "$MERCURY_CLIENT" "\$@"
ML_EOF
chmod +x "$ML"

section "Prepare Mercury"
log "creating Mercury wallets"
"$ML" create-wallet "$OWNER_MERCURY_WALLET" >/dev/null
"$ML" create-wallet "$RECEIVER_MERCURY_WALLET" >/dev/null

log "funding Mercury token fee if required"
"$ML" new-token | tee "$DEMO_DIR/token.json"
TOKEN_ID="$(jq -r '.token_id' "$DEMO_DIR/token.json")"
TOKEN_FEE="$(jq -r '.fee // 0' "$DEMO_DIR/token.json")"
TOKEN_ADDRESS="$(jq -r '.deposit_address // empty' "$DEMO_DIR/token.json")"

if [ "$TOKEN_FEE" -gt 0 ]; then
  curl -fsS -H 'content-type: application/json' \
    --data "$(jq -n --arg address "$TOKEN_ADDRESS" --argjson sats "$TOKEN_FEE" '{address: $address, sats: $sats}')" \
    "$MERCURY_FAUCET_URL/api/onchain" | tee "$DEMO_DIR/fund-token-fee.json"

  curl -fsS -H 'content-type: application/json' \
    --data '{"blocks":2}' \
    "$MERCURY_ADMIN_URL/api/mine" | tee "$DEMO_DIR/mine-token-fee.json"

  sleep 10
fi

log "funding seller Mercury statecoin"
"$ML" new-deposit-address "$OWNER_MERCURY_WALLET" "$TOKEN_ID" "$MERCURY_AMOUNT_SAT" \
  | tee "$DEMO_DIR/deposit-address.json"
DEPOSIT_ADDRESS="$(jq -r '.address' "$DEMO_DIR/deposit-address.json")"

curl -fsS -H 'content-type: application/json' \
  --data "$(jq -n --arg address "$DEPOSIT_ADDRESS" --argjson sats "$MERCURY_AMOUNT_SAT" '{address: $address, sats: $sats}')" \
  "$MERCURY_FAUCET_URL/api/onchain" | tee "$DEMO_DIR/fund-statecoin-deposit.json"

curl -fsS -H 'content-type: application/json' \
  --data '{"blocks":2}' \
  "$MERCURY_ADMIN_URL/api/mine" | tee "$DEMO_DIR/mine-statecoin-deposit.json"

log "waiting for confirmed seller statecoin"
while true; do
  "$ML" list-statecoins "$OWNER_MERCURY_WALLET" | tee "$DEMO_DIR/seller-statecoins.json"
  STATECHAIN_ID="$(
    jq -r --arg address "$DEPOSIT_ADDRESS" '
      .[]
      | select(."coin.aggregated_address" == $address)
      | select(."coin.status" == "CONFIRMED")
      | ."coin.statechain_id"
    ' "$DEMO_DIR/seller-statecoins.json" | tail -n 1
  )"

  if [ -n "$STATECHAIN_ID" ] && [ "$STATECHAIN_ID" != "null" ]; then
    export STATECHAIN_ID
    echo "$STATECHAIN_ID" | tee "$DEMO_DIR/statechain-id.txt"
    break
  fi

  sleep 5
done

section "Prepare Zerosats"
log "preparing Zerosats wallets"
cd "$ZEROSATS_WALLET_DIR"
ensure_wallet "$FUNDER_ZEROSATS_WALLET"
ensure_wallet "$CLAIMER_ZEROSATS_WALLET"
sync_wallet "$FUNDER_ZEROSATS_WALLET"

NEEDED_WEI=$((AMOUNT_SAT * 10000000000))
BALANCE_WEI="$(wallet_balance_wei "$FUNDER_ZEROSATS_WALLET")"
if [ "$BALANCE_WEI" -lt "$NEEDED_WEI" ]; then
  require_private_key
  CIPHERA_ROLLUP="$(curl -fsS "$CIPHERA_HOST/v0/network" | jq -r '.rollup_contract')"
  export CIPHERA_ROLLUP

  log "approving Zerosats mint allowance"
  "$CIPHERA_CLI" \
    --host "$CIPHERA_HOST" \
    --chain "$CITREA_CHAIN" \
    approve-mint \
    --secret "$CITREA_PRIVATE_KEY" \
    --geth-rpc "$CITREA_RPC"

  log "minting buyer Zerosats wallet"
  "$CIPHERA_CLI" \
    --name "$FUNDER_ZEROSATS_WALLET" \
    --host "$CIPHERA_HOST" \
    --chain "$CITREA_CHAIN" \
    --rollup "$CIPHERA_ROLLUP" \
    mint \
    --amount-sat "$AMOUNT_SAT" \
    --secret "$CITREA_PRIVATE_KEY" \
    --geth-rpc "$CITREA_RPC"

  sync_wallet "$FUNDER_ZEROSATS_WALLET"
else
  log "buyer already has enough Zerosats balance"
fi

section "Swap"
log "starting timed swap batch"
json_step seller-init \
  "Seller initializes local state" \
  "$SWAP" seller init \
    --state "$SELLER_STATE" \
    --mercury-settings-file "$SETTINGS" \
    --mercury-wallet "$OWNER_MERCURY_WALLET" \
    --zerosats-wallet "$CLAIMER_ZEROSATS_WALLET" \
    --zerosats-wallet-dir "$WALLETS" \
    --output-prefix "$SELLER_OUT" \
    --statechain-id "$STATECHAIN_ID" \
    --amount-sat "$AMOUNT_SAT" \
    --mercury-amount-sat "$MERCURY_AMOUNT_SAT" \
    --zerosats-host "$CIPHERA_HOST" \
    --citrea-chain "$CITREA_CHAIN" \
    --refund-blocks "$REFUND_BLOCKS" \
    --btc-explorer "$BTC_EXPLORER"

json_step seller-offer \
  "Seller creates offer section in swap transcript" \
  "$SWAP" seller offer \
    --state "$SELLER_STATE" \
    --swap "$SWAP_JSON"

inspect_swap_transcript

json_step buyer-accept \
  "Buyer imports offer and adds addresses to swap transcript" \
  "$SWAP" buyer accept \
    --state "$BUYER_STATE" \
    --swap "$SWAP_JSON" \
    --mercury-settings-file "$SETTINGS" \
    --mercury-wallet "$RECEIVER_MERCURY_WALLET" \
    --zerosats-wallet "$FUNDER_ZEROSATS_WALLET" \
    --zerosats-wallet-dir "$WALLETS" \
    --output-prefix "$BUYER_OUT"

inspect_swap_transcript

json_step seller-template \
  "Seller imports addresses and adds template to swap transcript" \
  "$SWAP" seller template \
    --state "$SELLER_STATE" \
    --swap "$SWAP_JSON"

inspect_swap_transcript

json_step buyer-funding \
  "Buyer imports template and adds funding to swap transcript" \
  "$SWAP" buyer fund \
    --state "$BUYER_STATE" \
    --swap "$SWAP_JSON"

inspect_swap_transcript

json_step seller-release \
  "Seller verifies funding, releases Mercury, and adds unlock notice" \
  "$SWAP" seller release \
    --state "$SELLER_STATE" \
    --swap "$SWAP_JSON" \
    --release-mercury

inspect_swap_transcript

json_step buyer-receipt \
  "Buyer imports unlock and adds receive confirmation" \
  "$SWAP" buyer receive \
    --state "$BUYER_STATE" \
    --swap "$SWAP_JSON"

inspect_swap_transcript

json_step seller-claim \
  "Seller imports receive confirmation, retrieves preimage, and claims latch" \
  "$SWAP" seller claim \
    --state "$SELLER_STATE" \
    --swap "$SWAP_JSON"

section "Final Checks"
log "checking final balances and artifacts"
"$ML" list-statecoins "$RECEIVER_MERCURY_WALLET" | tee "$DEMO_DIR/buyer-statecoins-final.json"
sync_wallet "$CLAIMER_ZEROSATS_WALLET"
jq '.name, .chain_id, .balance' "$ZEROSATS_WALLET_DIR/${CLAIMER_ZEROSATS_WALLET}.json"
ls -l "$BUYER_OUT-refund.json"
jq .claim "$SELLER_STATE"

log "demo complete"
echo "Run directory: $DEMO_DIR"
echo "Seller state: $SELLER_STATE"
echo "Buyer state: $BUYER_STATE"
echo "Swap transcript: $SWAP_JSON"
EOF

chmod +x "$RUNNER"

cat > "$DEMO_DIR/env.sh" <<EOF
export CIPHERA_DIR=$(printf '%q' "$CIPHERA_DIR")
export CIPHERA_WORKTREE=$(printf '%q' "$CIPHERA_WORKTREE")
export MERCURY_LAB_DIR=$(printf '%q' "$MERCURY_LAB_DIR")
export DEMO_DATA_DIR=$(printf '%q' "$DEMO_DATA_DIR")
export ZEROSATS_WALLET_DIR=$(printf '%q' "$ZEROSATS_WALLET_DIR")
export DEMO_DIR=$(printf '%q' "$DEMO_DIR")
export MERCURY_URL=$(printf '%q' "${MERCURY_URL:-http://204.168.231.217:28000}")
export MERCURY_ELECTRUM_SERVER=$(printf '%q' "${MERCURY_ELECTRUM_SERVER:-tcp://62.238.20.16:50001}")
export MERCURY_FAUCET_URL=$(printf '%q' "${MERCURY_FAUCET_URL:-http://62.238.20.16:3000}")
export MERCURY_ADMIN_URL=$(printf '%q' "${MERCURY_ADMIN_URL:-http://62.238.20.16:3030}")
export CIPHERA_HOST=$(printf '%q' "${CIPHERA_HOST:-https://ciphera.satsbridge.com}")
export CITREA_CHAIN=$(printf '%q' "${CITREA_CHAIN:-5115}")
export CITREA_RPC=$(printf '%q' "${CITREA_RPC:-https://rpc.testnet.citrea.xyz}")
export BTC_EXPLORER=$(printf '%q' "${BTC_EXPLORER:-https://mempool.space}")
export MERCURY_AMOUNT_SAT=$(printf '%q' "${MERCURY_AMOUNT_SAT:-1000}")
export AMOUNT_SAT=$(printf '%q' "${AMOUNT_SAT:-1000}")
export REFUND_BLOCKS=$(printf '%q' "${REFUND_BLOCKS:-2}")
export SWAP=$(printf '%q' "$CIPHERA_DIR/pkg/mercury-latch-swap/target/debug/mercury-latch-swap")
export RUNNER=$(printf '%q' "$RUNNER")
export SETTINGS=$(printf '%q' "$DEMO_DIR/mercury-wallet/Settings.toml")
export WALLETS=$(printf '%q' "$ZEROSATS_WALLET_DIR")
export MERCURY_SETTINGS_FILE=$(printf '%q' "$DEMO_DIR/mercury-wallet/Settings.toml")
export SELLER_OUT=$(printf '%q' "$DEMO_DIR/seller/mercury-ciphera-swap")
export BUYER_OUT=$(printf '%q' "$DEMO_DIR/buyer/mercury-ciphera-swap")
export SWAP_JSON=$(printf '%q' "$DEMO_DIR/swap.json")
export SELLER_STATE=$(printf '%q' "$DEMO_DIR/seller/mercury-ciphera-swap-state.json")
export BUYER_STATE=$(printf '%q' "$DEMO_DIR/buyer/mercury-ciphera-swap-state.json")
export SELLER_WALLET=$(printf '%q' "${SELLER_WALLET:-seller}")
export BUYER_WALLET=$(printf '%q' "${BUYER_WALLET:-buyer}")
export OWNER_MERCURY_WALLET=$(printf '%q' "${OWNER_MERCURY_WALLET:-${SELLER_WALLET:-seller}}")
export RECEIVER_MERCURY_WALLET=$(printf '%q' "${RECEIVER_MERCURY_WALLET:-${BUYER_WALLET:-buyer}}")
export FUNDER_ZEROSATS_WALLET=$(printf '%q' "${FUNDER_ZEROSATS_WALLET:-${BUYER_WALLET:-buyer}}")
export CLAIMER_ZEROSATS_WALLET=$(printf '%q' "${CLAIMER_ZEROSATS_WALLET:-${SELLER_WALLET:-seller}}")
EOF

if [ "${RUNNER_ONLY:-0}" = "1" ]; then
  echo "Wrote demo runner: $RUNNER"
  echo "Wrote environment file: $DEMO_DIR/env.sh"
  echo "Run it manually with: source '$DEMO_DIR/env.sh'; '$RUNNER'"
  echo "Readable progress is logged in: $DEMO_DIR/demo.log"
  exit 0
fi

if ! command -v tmux >/dev/null 2>&1; then
  echo "tmux is required for automatic attach." >&2
  echo "Manual runner is ready. Run: source '$DEMO_DIR/env.sh'; '$RUNNER'" >&2
  exit 1
fi

if tmux has-session -t "$SESSION" 2>/dev/null; then
  echo "tmux session '$SESSION' already exists. Attach with: tmux attach -t $SESSION" >&2
  exit 1
fi

tmux new-session -d -s "$SESSION" "source '$DEMO_DIR/env.sh'; '$RUNNER'; echo; echo 'Press enter to close this pane.'; read"

echo "Started tmux session '$SESSION'"
echo "Run directory: $DEMO_DIR"

if [ "${ATTACH:-1}" = "1" ]; then
  if [ -n "${TMUX:-}" ]; then
    tmux switch-client -t "$SESSION"
  else
    tmux attach -t "$SESSION"
  fi
fi
