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
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(crate) const STATE_VERSION: u32 = 2;
pub(crate) const TRANSCRIPT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SwapRole {
    Seller,
    Buyer,
}

impl std::fmt::Display for SwapRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seller => write!(f, "seller"),
            Self::Buyer => write!(f, "buyer"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct SwapState {
    pub(crate) version: u32,
    pub(crate) role: SwapRole,
    pub(crate) terms: SwapTerms,
    pub(crate) local: LocalConfig,
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SwapTerms {
    pub(crate) statechain_id: String,
    pub(crate) amount_sat: u64,
    pub(crate) mercury_amount_sat: u64,
    pub(crate) zerosats_host: String,
    pub(crate) citrea_chain: u64,
    pub(crate) refund_blocks: u64,
    pub(crate) ticker: String,
    pub(crate) btc_explorer: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct LocalConfig {
    pub(crate) mercury_settings_file: PathBuf,
    pub(crate) mercury_wallet: String,
    pub(crate) zerosats_wallet: String,
    pub(crate) zerosats_wallet_dir: Option<PathBuf>,
    pub(crate) output_prefix: PathBuf,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MercuryReceiveState {
    pub(crate) is_there_batch_locked: bool,
    pub(crate) received_statechain_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransactionState {
    pub(crate) txn_hash: String,
    pub(crate) height: String,
    pub(crate) root_hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LatchVerifyState {
    pub(crate) commitment: String,
    pub(crate) height: u64,
    pub(crate) root_hash: String,
    pub(crate) txn_hash: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateHashState {
    pub(crate) payment_hash: String,
    pub(crate) batch_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MercuryAddressState {
    pub(crate) mercury_transfer_address: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TemplatePublicState {
    pub(crate) amount_wei: u64,
    pub(crate) claim_address: String,
    pub(crate) latch_commitment: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct FundState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) refund_note: Option<PathBuf>,
    pub(crate) lock_transaction: TransactionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) balance_sat: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct FundPublicState {
    pub(crate) lock_transaction: TransactionState,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct VerifyState {
    pub(crate) latch_verify: LatchVerifyState,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TransferState {
    pub(crate) retrieved_hash: String,
    pub(crate) hash_matches: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnlockState {
    pub(crate) latch_verify: LatchVerifyState,
    pub(crate) unlocked: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct SellerOfferSection {
    pub(crate) terms: SwapTerms,
    pub(crate) create_hash: CreateHashState,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuyerAddressesSection {
    pub(crate) mercury_address: MercuryAddressState,
    pub(crate) zerosats_address: ZerosatsAddressState,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct SellerTemplateSection {
    pub(crate) zerosats_address: ZerosatsAddressState,
    pub(crate) template: TemplatePublicState,
    pub(crate) template_note: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuyerFundingSection {
    pub(crate) zerosats_address: ZerosatsAddressState,
    pub(crate) template: TemplatePublicState,
    pub(crate) fund: FundPublicState,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct SellerUnlockSection {
    pub(crate) transfer: TransferState,
    pub(crate) unlock: UnlockState,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuyerReceiveSection {
    pub(crate) receive: ReceiveState,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct SwapTranscript {
    pub(crate) version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seller_offer: Option<SellerOfferSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) buyer_addresses: Option<BuyerAddressesSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seller_template: Option<SellerTemplateSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) buyer_funding: Option<BuyerFundingSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) seller_unlock: Option<SellerUnlockSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) buyer_receive: Option<BuyerReceiveSection>,
}

impl Default for SwapTranscript {
    fn default() -> Self {
        Self {
            version: TRANSCRIPT_VERSION,
            seller_offer: None,
            buyer_addresses: None,
            seller_template: None,
            buyer_funding: None,
            seller_unlock: None,
            buyer_receive: None,
        }
    }
}

impl SwapState {
    pub(crate) fn new(role: SwapRole, terms: SwapTerms, local: LocalConfig) -> Self {
        Self {
            version: STATE_VERSION,
            role,
            terms,
            local,
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

    pub(crate) fn ensure_role(&self, expected: SwapRole) -> Result<()> {
        if self.role == expected {
            Ok(())
        } else {
            Err(eyre!(
                "this is a {} state; run this command with a {} state",
                self.role,
                expected
            ))
        }
    }

    pub(crate) fn create_hash(&self) -> Result<&CreateHashState> {
        required_step(&self.create_hash, "seller offer")
    }

    pub(crate) fn mercury_address(&self) -> Result<&MercuryAddressState> {
        required_step(&self.mercury_address, "buyer accept")
    }

    pub(crate) fn zerosats_address(&self) -> Result<&ZerosatsAddressState> {
        required_step(&self.zerosats_address, "buyer accept")
    }

    pub(crate) fn template(&self) -> Result<&TemplateState> {
        required_step(&self.template, "seller template or buyer fund")
    }

    pub(crate) fn fund(&self) -> Result<&FundState> {
        required_step(&self.fund, "buyer fund or seller release")
    }

    pub(crate) fn verify(&self) -> Result<&VerifyState> {
        required_step(&self.verify, "seller release")
    }

    pub(crate) fn transfer(&self) -> Result<&TransferState> {
        required_step(&self.transfer, "seller release")
    }

    pub(crate) fn unlock(&self) -> Result<&UnlockState> {
        required_step(&self.unlock, "seller release or buyer receive")
    }

    pub(crate) fn receive(&self) -> Result<&ReceiveState> {
        required_step(&self.receive, "buyer receive or seller claim")
    }

    pub(crate) fn preimage(&self) -> Result<&PreimageState> {
        required_step(&self.preimage, "seller claim")
    }
}

impl From<&TemplateState> for TemplatePublicState {
    fn from(template: &TemplateState) -> Self {
        Self {
            amount_wei: template.amount_wei,
            claim_address: template.claim_address.clone(),
            latch_commitment: template.latch_commitment.clone(),
        }
    }
}

impl From<&FundState> for FundPublicState {
    fn from(fund: &FundState) -> Self {
        Self {
            lock_transaction: fund.lock_transaction.clone(),
        }
    }
}

pub(crate) fn load_state(path: &Path) -> Result<SwapState> {
    let state: SwapState = load_json(path, "swap state")?;
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
    save_json(path, state, true)
}

pub(crate) fn load_transcript(path: &Path) -> Result<SwapTranscript> {
    let transcript: SwapTranscript = load_json(path, "swap transcript")?;
    if transcript.version != TRANSCRIPT_VERSION {
        return Err(eyre!(
            "unsupported swap transcript version {} in {}; expected {}",
            transcript.version,
            path.display(),
            TRANSCRIPT_VERSION
        ));
    }
    Ok(transcript)
}

pub(crate) fn load_transcript_or_default(path: &Path) -> Result<SwapTranscript> {
    if path
        .try_exists()
        .wrap_err_with(|| format!("failed to check {}", path.display()))?
    {
        load_transcript(path)
    } else {
        Ok(SwapTranscript::default())
    }
}

pub(crate) fn save_transcript(path: &Path, transcript: &SwapTranscript) -> Result<()> {
    save_json(path, transcript, false)
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

    if receive.received_mercury_amount_sat != state.terms.mercury_amount_sat {
        return Err(eyre!(
            "received Mercury amount mismatch: expected {} sats, got {} sats",
            state.terms.mercury_amount_sat,
            receive.received_mercury_amount_sat
        ));
    }

    ensure_received_statechain(&receive.final_receive, &state.terms.statechain_id)
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

fn load_json<T: DeserializeOwned>(path: &Path, label: &str) -> Result<T> {
    let json = fs::read_to_string(path).map_err(|e| eyre!("{}: {e}", path.display()))?;
    serde_json::from_str(&json)
        .wrap_err_with(|| format!("failed to parse {label} {}", path.display()))
}

fn save_json<T: Serialize>(path: &Path, value: &T, private: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
        }
    }

    let json = serde_json::to_string_pretty(value).wrap_err("failed to serialize JSON")?;
    let tmp = temporary_file_path(path);
    fs::write(&tmp, format!("{json}\n"))
        .wrap_err_with(|| format!("failed to write {}", tmp.display()))?;
    if private {
        set_private_permissions(&tmp)?;
    }
    fs::rename(&tmp, path).wrap_err_with(|| {
        format!(
            "failed to replace {} with {}",
            path.display(),
            tmp.display()
        )
    })?;
    Ok(())
}

fn required_step<'a, T>(value: &'a Option<T>, step: &str) -> Result<&'a T> {
    value
        .as_ref()
        .ok_or_else(|| eyre!("{step} step has not completed"))
}

fn temporary_file_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new("swap.json"))
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
