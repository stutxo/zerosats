use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use node_interface::{ElementsResponseSingle, TransactionResponse};
use serde::{Deserialize, Serialize};

pub(crate) const STATE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct SwapState {
    pub(crate) version: u32,
    pub(crate) config: SwapConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) create_hash: Option<CreateHashState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) mercury_address: Option<MercuryAddressState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) zerosats_address: Option<ZerosatsAddressState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) template: Option<TemplateState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) fund: Option<FundState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verify: Option<VerifyState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transfer: Option<TransferState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unlock: Option<UnlockState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) receive: Option<ReceiveState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preimage: Option<PreimageState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) claim: Option<ClaimState>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct SwapConfig {
    pub(crate) mercury_settings_file: PathBuf,
    pub(crate) statechain_id: String,
    pub(crate) amount_sat: u64,
    pub(crate) mercury_amount_sat: u64,
    pub(crate) owner_mercury_wallet: String,
    pub(crate) receiver_mercury_wallet: String,
    pub(crate) funder_zerosats_wallet: String,
    pub(crate) claimer_zerosats_wallet: String,
    pub(crate) zerosats_wallet_dir: Option<PathBuf>,
    pub(crate) zerosats_host: String,
    pub(crate) citrea_chain: u64,
    pub(crate) refund_blocks: u64,
    pub(crate) ticker: String,
    pub(crate) btc_explorer: String,
    pub(crate) output_prefix: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MercuryReceiveState {
    pub(crate) is_there_batch_locked: bool,
    pub(crate) received_statechain_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TransactionState {
    pub(crate) txn_hash: String,
    pub(crate) height: String,
    pub(crate) root_hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct LatchVerifyState {
    pub(crate) commitment: String,
    pub(crate) height: u64,
    pub(crate) root_hash: String,
    pub(crate) txn_hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct CreateHashState {
    pub(crate) payment_hash: String,
    pub(crate) batch_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MercuryAddressState {
    pub(crate) mercury_transfer_address: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ZerosatsAddressState {
    pub(crate) refund_address: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TemplateState {
    pub(crate) amount_wei: u64,
    pub(crate) claim_address: String,
    pub(crate) latch_commitment: String,
    pub(crate) template_note: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct FundState {
    pub(crate) refund_note: PathBuf,
    pub(crate) lock_transaction: TransactionState,
    pub(crate) balance_sat: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct VerifyState {
    pub(crate) latch_verify: LatchVerifyState,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct TransferState {
    pub(crate) retrieved_hash: String,
    pub(crate) hash_matches: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct UnlockState {
    pub(crate) latch_verify: LatchVerifyState,
    pub(crate) unlocked: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ReceiveState {
    pub(crate) received_mercury_amount_sat: u64,
    pub(crate) received_expected_statechain_id: bool,
    pub(crate) final_receive: MercuryReceiveState,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct PreimageState {
    pub(crate) preimage: String,
    pub(crate) preimage_hash: String,
    pub(crate) hash_matches: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ClaimState {
    pub(crate) claim_transaction: TransactionState,
    pub(crate) balance_sat: u64,
    pub(crate) ticker: String,
}

impl SwapState {
    pub(crate) fn new(config: SwapConfig) -> Self {
        Self {
            version: STATE_VERSION,
            config,
            create_hash: None,
            mercury_address: None,
            zerosats_address: None,
            template: None,
            fund: None,
            verify: None,
            transfer: None,
            unlock: None,
            receive: None,
            preimage: None,
            claim: None,
        }
    }

    pub(crate) fn create_hash(&self) -> Result<&CreateHashState> {
        required_step(&self.create_hash, "mercury-create-hash")
    }

    pub(crate) fn mercury_address(&self) -> Result<&MercuryAddressState> {
        required_step(&self.mercury_address, "mercury-address")
    }

    pub(crate) fn zerosats_address(&self) -> Result<&ZerosatsAddressState> {
        required_step(&self.zerosats_address, "zerosats-address")
    }

    pub(crate) fn template(&self) -> Result<&TemplateState> {
        required_step(&self.template, "zerosats-template")
    }

    pub(crate) fn verify(&self) -> Result<&VerifyState> {
        required_step(&self.verify, "zerosats-verify")
    }

    pub(crate) fn transfer(&self) -> Result<&TransferState> {
        required_step(&self.transfer, "mercury-transfer")
    }

    pub(crate) fn unlock(&self) -> Result<&UnlockState> {
        required_step(&self.unlock, "mercury-unlock")
    }

    pub(crate) fn receive(&self) -> Result<&ReceiveState> {
        required_step(&self.receive, "mercury-receive")
    }

    pub(crate) fn preimage(&self) -> Result<&PreimageState> {
        required_step(&self.preimage, "mercury-preimage")
    }
}

pub(crate) fn load_state(path: &Path) -> Result<SwapState> {
    let json = fs::read_to_string(path).map_err(|e| eyre!("{}: {e}", path.display()))?;
    let state: SwapState = serde_json::from_str(&json)
        .wrap_err_with(|| format!("failed to parse swap state {}", path.display()))?;
    if state.version != STATE_VERSION {
        return Err(eyre!(
            "unsupported swap state version {} in {}; expected {}",
            state.version,
            path.display(),
            STATE_VERSION
        ));
    }
    Ok(state)
}

pub(crate) fn save_state(path: &Path, state: &SwapState) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
        }
    }

    let json = serde_json::to_string_pretty(state).wrap_err("failed to serialize swap state")?;
    let tmp = temporary_state_path(path);
    fs::write(&tmp, format!("{json}\n"))
        .wrap_err_with(|| format!("failed to write {}", tmp.display()))?;
    set_private_permissions(&tmp)?;
    fs::rename(&tmp, path).wrap_err_with(|| {
        format!(
            "failed to replace swap state {} with {}",
            path.display(),
            tmp.display()
        )
    })?;
    Ok(())
}

pub(crate) fn summarize_receive(
    receive: mercuryrustlib::transfer_receiver::TransferReceiveResult,
) -> MercuryReceiveState {
    MercuryReceiveState {
        is_there_batch_locked: receive.is_there_batch_locked,
        received_statechain_ids: receive.received_statechain_ids,
    }
}

pub(crate) fn ensure_received_statechain(
    receive: &MercuryReceiveState,
    expected_statechain_id: &str,
) -> Result<()> {
    if receive.is_there_batch_locked {
        return Err(eyre!(
            "Mercury receive is still batch-locked; do not retrieve the preimage or claim \
             Zerosats yet"
        ));
    }

    if !receive
        .received_statechain_ids
        .iter()
        .any(|statechain_id| statechain_id == expected_statechain_id)
    {
        return Err(eyre!(
            "Mercury receive did not return expected statechain_id {}; received {:?}. Do not \
             retrieve the preimage or claim Zerosats.",
            expected_statechain_id,
            receive.received_statechain_ids
        ));
    }

    Ok(())
}

pub(crate) fn ensure_state_allows_preimage(state: &SwapState) -> Result<()> {
    let receive = state.receive()?;

    if !receive.received_expected_statechain_id {
        return Err(eyre!(
            "receive step did not confirm the expected statechain_id"
        ));
    }

    if receive.received_mercury_amount_sat != state.config.mercury_amount_sat {
        return Err(eyre!(
            "received Mercury amount mismatch: expected {} sats, got {} sats",
            state.config.mercury_amount_sat,
            receive.received_mercury_amount_sat
        ));
    }

    ensure_received_statechain(&receive.final_receive, &state.config.statechain_id)
}

pub(crate) fn summarize_latch_verify(verified: ElementsResponseSingle) -> LatchVerifyState {
    LatchVerifyState {
        commitment: verified.element.to_string(),
        height: verified.height,
        root_hash: verified.root_hash.to_string(),
        txn_hash: verified.txn_hash.to_string(),
    }
}

pub(crate) fn summarize_tx(tx: &TransactionResponse) -> TransactionState {
    TransactionState {
        txn_hash: tx.txn_hash.to_string(),
        height: tx.height.to_string(),
        root_hash: tx.root_hash.to_string(),
    }
}

fn required_step<'a, T>(value: &'a Option<T>, step: &str) -> Result<&'a T> {
    value
        .as_ref()
        .ok_or_else(|| eyre!("{step} step has not completed"))
}

fn temporary_state_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("swap-state.json"))
        .to_string_lossy();
    path.with_file_name(format!("{file_name}.{}.tmp", std::process::id()))
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .wrap_err_with(|| format!("failed to restrict permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}
