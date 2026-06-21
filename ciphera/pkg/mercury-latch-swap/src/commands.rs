use std::{
    fs,
    path::{Path, PathBuf},
};

use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    cli::*,
    latch,
    mercury::{WrapDisplay, load_mercury},
    state::*,
};

const STDOUT_REDACTED: &str = "<redacted: stored in local state file>";
const SWAP_TRANSCRIPT_FILE: &str = "swap.json";

pub(crate) async fn execute(command: Command) -> Result<serde_json::Value> {
    match command {
        Command::Status(args) => output(status(args)),
        Command::InspectSwap(args) => output(inspect_swap(args)),
        Command::Seller(command) => execute_seller(command).await,
        Command::Buyer(command) => execute_buyer(command).await,
    }
}

async fn execute_seller(command: SellerCommand) -> Result<serde_json::Value> {
    match command {
        SellerCommand::Init(args) => output(seller_init(args)),
        SellerCommand::Offer(args) => output(seller_offer(args).await),
        SellerCommand::Template(args) => output(seller_template(args).await),
        SellerCommand::Release(args) => output(seller_release(args).await),
        SellerCommand::Claim(args) => output(seller_claim_phase(args).await),
    }
}

async fn execute_buyer(command: BuyerCommand) -> Result<serde_json::Value> {
    match command {
        BuyerCommand::Accept(args) => output(buyer_accept(args).await),
        BuyerCommand::Fund(args) => output(buyer_funding(args).await),
        BuyerCommand::Receive(args) => output(buyer_receive_phase(args).await),
    }
}

#[derive(Debug, Serialize)]
struct SwapStatus {
    role: SwapRole,
    statechain_id: String,
    completed_steps: Vec<&'static str>,
    next_local_command: Option<&'static str>,
    waiting_for: Option<ProtocolAction>,
    available_exports: Vec<ProtocolAction>,
}

#[derive(Debug, Serialize)]
struct ProtocolAction {
    file: &'static str,
    command: &'static str,
}

#[derive(Debug, Serialize)]
struct SwapInspection {
    version: u32,
    statechain_id: Option<String>,
    completed_sections: Vec<&'static str>,
    next_command: Option<&'static str>,
    safe_contents: Vec<&'static str>,
}

fn output<T: Serialize>(result: Result<T>) -> Result<serde_json::Value> {
    let mut value = serde_json::to_value(result?).wrap_err("failed to serialize command output")?;
    redact_stdout_secrets(&mut value);
    Ok(value)
}

fn redact_stdout_secrets(value: &mut serde_json::Value) {
    let Some(root) = value.as_object_mut() else {
        return;
    };

    if let Some(local) = root
        .get_mut("local")
        .and_then(serde_json::Value::as_object_mut)
    {
        for field in [
            "mercury_settings_file",
            "mercury_wallet",
            "zerosats_wallet",
            "zerosats_wallet_dir",
            "output_prefix",
        ] {
            redact_field(local, field);
        }
    }

    if let Some(template) = root
        .get_mut("template")
        .and_then(serde_json::Value::as_object_mut)
    {
        redact_field(template, "template_note");
    }

    if let Some(fund) = root
        .get_mut("fund")
        .and_then(serde_json::Value::as_object_mut)
    {
        redact_field(fund, "refund_note");
    }

    if let Some(preimage) = root
        .get_mut("preimage")
        .and_then(serde_json::Value::as_object_mut)
    {
        redact_field(preimage, "preimage");
    }
}

fn redact_field(object: &mut serde_json::Map<String, serde_json::Value>, field: &str) {
    if let Some(value) = object.get_mut(field) {
        *value = serde_json::Value::String(STDOUT_REDACTED.to_string());
    }
}

fn status(args: StateCommandArgs) -> Result<SwapStatus> {
    let state = load_state(&args.state)?;
    Ok(match state.role {
        SwapRole::Seller => seller_status(&state),
        SwapRole::Buyer => buyer_status(&state),
    })
}

fn inspect_swap(args: SwapCommandArgs) -> Result<SwapInspection> {
    let transcript = load_transcript(&args.swap)?;
    validate_transcript(&transcript, &args.swap)?;

    let mut completed_sections = Vec::new();
    let mut safe_contents = Vec::new();
    let mut statechain_id = None;

    if let Some(offer) = &transcript.seller_offer {
        statechain_id = Some(offer.terms.statechain_id.clone());
        completed_sections.push("seller_offer");
        safe_contents.push("swap terms");
        safe_contents.push("payment hash");
        safe_contents.push("batch id");
    }
    if transcript.buyer_addresses.is_some() {
        completed_sections.push("buyer_addresses");
        safe_contents.push("Mercury receive address");
        safe_contents.push("Zerosats refund address");
    }
    if transcript.seller_template.is_some() {
        completed_sections.push("seller_template");
        safe_contents.push("latch template summary");
        safe_contents.push("embedded latch template JSON");
    }
    if transcript.buyer_funding.is_some() {
        completed_sections.push("buyer_funding");
        safe_contents.push("funded latch transaction summary");
    }
    if transcript.seller_unlock.is_some() {
        completed_sections.push("seller_unlock");
        safe_contents.push("Mercury transfer hash check");
        safe_contents.push("unlock verification summary");
    }
    if transcript.buyer_receive.is_some() {
        completed_sections.push("buyer_receive");
        safe_contents.push("Mercury receive confirmation");
    }

    let next_command = if transcript.seller_offer.is_none() {
        Some("seller offer --swap <file>")
    } else if transcript.buyer_addresses.is_none() {
        Some("buyer accept --swap <file>")
    } else if transcript.seller_template.is_none() {
        Some("seller template --swap <file>")
    } else if transcript.buyer_funding.is_none() {
        Some("buyer fund --swap <file>")
    } else if transcript.seller_unlock.is_none() {
        Some("seller release --swap <file> --release-mercury")
    } else if transcript.buyer_receive.is_none() {
        Some("buyer receive --swap <file>")
    } else {
        Some("seller claim --swap <file>")
    };

    Ok(SwapInspection {
        version: transcript.version,
        statechain_id,
        completed_sections,
        next_command,
        safe_contents,
    })
}

fn seller_status(state: &SwapState) -> SwapStatus {
    let mut completed_steps = Vec::new();
    let mut available_exports = Vec::new();

    if state.create_hash.is_some() {
        completed_steps.push("offer");
        available_exports.push(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "seller offer --swap <file>",
        ));
    }
    if state.mercury_address.is_some() && state.zerosats_address.is_some() {
        completed_steps.push("buyer-addresses");
    }
    if state.template.is_some() {
        completed_steps.push("template");
        available_exports.push(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "seller template --swap <file>",
        ));
    }
    if state.fund.is_some() {
        completed_steps.push("funding");
    }
    if state.verify.is_some() {
        completed_steps.push("funding-verified");
    }
    if state.transfer.is_some() {
        completed_steps.push("mercury-transfer");
    }
    if state.unlock.is_some() {
        completed_steps.push("mercury-unlock");
        available_exports.push(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "seller release --swap <file> --release-mercury",
        ));
    }
    if state.unlock.is_some() && state.receive.is_some() {
        completed_steps.push("receive-confirmed");
    }
    if state.receive.is_some() && state.preimage.is_some() {
        completed_steps.push("preimage");
    }
    if state.preimage.is_some() && state.claim.is_some() {
        completed_steps.push("claim");
    }

    let next_local_command = if state.create_hash.is_none() {
        Some("seller offer")
    } else if state.mercury_address.is_none() || state.zerosats_address.is_none() {
        None
    } else if state.template.is_none() {
        Some("seller template")
    } else if state.fund.is_none() {
        None
    } else if state.verify.is_none() {
        Some("seller release --release-mercury")
    } else if state.transfer.is_none() {
        Some("seller release --release-mercury")
    } else if state.unlock.is_none() {
        Some("seller release --release-mercury")
    } else if state.receive.is_none() {
        None
    } else if state.preimage.is_none() {
        Some("seller claim")
    } else if state.claim.is_none() {
        Some("seller claim")
    } else {
        None
    };

    let waiting_for = if state.create_hash.is_some()
        && (state.mercury_address.is_none() || state.zerosats_address.is_none())
    {
        Some(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "seller template --swap <file>",
        ))
    } else if state.template.is_some() && state.fund.is_none() {
        Some(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "seller release --swap <file> --release-mercury",
        ))
    } else if state.unlock.is_some() && state.receive.is_none() {
        Some(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "buyer receive --swap <file>",
        ))
    } else {
        None
    };

    status_output(
        state,
        completed_steps,
        next_local_command,
        waiting_for,
        available_exports,
    )
}

fn buyer_status(state: &SwapState) -> SwapStatus {
    let mut completed_steps = Vec::new();
    let mut available_exports = Vec::new();

    if state.create_hash.is_some() {
        completed_steps.push("offer");
    }
    if state.mercury_address.is_some() {
        completed_steps.push("mercury-address");
    }
    if state.zerosats_address.is_some() {
        completed_steps.push("refund-address");
    }
    if state.mercury_address.is_some() && state.zerosats_address.is_some() {
        available_exports.push(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "buyer accept --swap <file>",
        ));
    }
    if state.template.is_some() {
        completed_steps.push("template");
    }
    if state.fund.is_some() {
        completed_steps.push("funding");
        available_exports.push(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "buyer fund --swap <file>",
        ));
    }
    if state.unlock.is_some() {
        completed_steps.push("unlock");
    }
    if state.unlock.is_some() && state.receive.is_some() {
        completed_steps.push("receive");
        available_exports.push(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "buyer receive --swap <file>",
        ));
    }

    let next_local_command = if state.mercury_address.is_none() {
        Some("buyer accept")
    } else if state.zerosats_address.is_none() {
        Some("buyer accept")
    } else if state.template.is_none() {
        None
    } else if state.fund.is_none() {
        Some("buyer fund")
    } else if state.unlock.is_none() {
        None
    } else if state.receive.is_none() {
        Some("buyer receive")
    } else {
        None
    };

    let waiting_for = if state.mercury_address.is_some()
        && state.zerosats_address.is_some()
        && state.template.is_none()
    {
        Some(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "buyer fund --swap <file>",
        ))
    } else if state.fund.is_some() && state.unlock.is_none() {
        Some(protocol_file(
            SWAP_TRANSCRIPT_FILE,
            "buyer receive --swap <file>",
        ))
    } else {
        None
    };

    status_output(
        state,
        completed_steps,
        next_local_command,
        waiting_for,
        available_exports,
    )
}

fn status_output(
    state: &SwapState,
    completed_steps: Vec<&'static str>,
    next_local_command: Option<&'static str>,
    waiting_for: Option<ProtocolAction>,
    available_exports: Vec<ProtocolAction>,
) -> SwapStatus {
    SwapStatus {
        role: state.role,
        statechain_id: state.terms.statechain_id.clone(),
        completed_steps,
        next_local_command,
        waiting_for,
        available_exports,
    }
}

fn protocol_file(file: &'static str, command: &'static str) -> ProtocolAction {
    ProtocolAction { file, command }
}

async fn seller_offer(args: SwapStateCommandArgs) -> Result<SellerOfferSection> {
    let state = args.state;
    let swap = args.transcript.swap;
    let current = load_state(&state)?;
    current.ensure_role(SwapRole::Seller)?;
    if current.create_hash.is_none() {
        seller_create_offer(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }

    let section = seller_offer_section(&state)?;
    save_seller_offer_section(&swap, section.clone())?;
    Ok(section)
}

async fn buyer_accept(args: BuyerAcceptArgs) -> Result<BuyerAddressesSection> {
    let BuyerAcceptArgs {
        state,
        transcript,
        local,
    } = args;
    let swap = transcript.swap;
    let offer = load_seller_offer_section(&swap)?;

    if state_file_exists(&state)? {
        ensure_existing_buyer_matches_offer_section(&state, &offer)?;
    } else {
        buyer_init_from_offer(state.clone(), local, offer)?;
    }

    let current = load_state(&state)?;
    current.ensure_role(SwapRole::Buyer)?;
    if current.mercury_address.is_none() {
        buyer_mercury_address(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }
    let current = load_state(&state)?;
    if current.zerosats_address.is_none() {
        buyer_zerosats_address(StateCommandArgs {
            state: state.clone(),
        })?;
    }

    let section = buyer_addresses_section(&state)?;
    save_buyer_addresses_section(&swap, section.clone())?;
    Ok(section)
}

async fn seller_template(args: SwapStateCommandArgs) -> Result<SellerTemplateSection> {
    let state = args.state;
    let swap = args.transcript.swap;
    let buyer_addresses = load_buyer_addresses_section(&swap)?;
    let current = load_state(&state)?;
    current.ensure_role(SwapRole::Seller)?;
    if current.mercury_address.is_none() || current.zerosats_address.is_none() {
        seller_import_addresses_section(&state, buyer_addresses)?;
    } else {
        ensure_existing_seller_matches_addresses_section(&state, &buyer_addresses)?;
    }

    let current = load_state(&state)?;
    if current.template.is_none() {
        seller_create_template(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }

    let section = seller_template_section(&state)?;
    save_seller_template_section(&swap, section.clone())?;
    Ok(section)
}

async fn buyer_funding(args: SwapStateCommandArgs) -> Result<BuyerFundingSection> {
    let state = args.state;
    let swap = args.transcript.swap;
    let seller_template = load_seller_template_section(&swap)?;
    let current = load_state(&state)?;
    current.ensure_role(SwapRole::Buyer)?;
    if current.template.is_none() {
        buyer_import_template_section(&state, seller_template)?;
    } else {
        ensure_existing_buyer_matches_template_section(&state, &seller_template)?;
    }

    let current = load_state(&state)?;
    if current.fund.is_none() {
        buyer_fund(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }

    let section = buyer_funding_section(&state)?;
    save_buyer_funding_section(&swap, section.clone())?;
    Ok(section)
}

async fn seller_release(args: SellerReleaseArgs) -> Result<SellerUnlockSection> {
    if !args.release_mercury {
        return Err(eyre!(
            "seller release verifies buyer funding and unlocks the Mercury transfer; rerun with \
             --release-mercury to continue"
        ));
    }

    let state = args.swap.state;
    let swap = args.swap.transcript.swap;
    let buyer_funding = load_buyer_funding_section(&swap)?;
    let current = load_state(&state)?;
    current.ensure_role(SwapRole::Seller)?;
    if current.fund.is_none() {
        seller_import_funding_section(&state, buyer_funding)?;
    } else {
        ensure_existing_seller_matches_funding_section(&state, &buyer_funding)?;
    }

    let current = load_state(&state)?;
    if current.verify.is_none() {
        seller_verify(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }
    let current = load_state(&state)?;
    if current.transfer.is_none() {
        seller_transfer(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }
    let current = load_state(&state)?;
    if current.unlock.is_none() {
        seller_unlock(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }

    let section = seller_unlock_section(&state)?;
    save_seller_unlock_section(&swap, section.clone())?;
    Ok(section)
}

async fn buyer_receive_phase(args: SwapStateCommandArgs) -> Result<BuyerReceiveSection> {
    let state = args.state;
    let swap = args.transcript.swap;
    let seller_unlock = load_seller_unlock_section(&swap)?;
    let current = load_state(&state)?;
    current.ensure_role(SwapRole::Buyer)?;
    if current.unlock.is_none() {
        buyer_import_unlock_section(&state, seller_unlock)?;
    } else {
        ensure_existing_buyer_matches_unlock_section(&state, &seller_unlock)?;
    }

    let current = load_state(&state)?;
    if current.receive.is_none() {
        buyer_receive(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }

    let section = buyer_receive_section(&state)?;
    save_buyer_receive_section(&swap, section.clone())?;
    Ok(section)
}

async fn seller_claim_phase(args: SwapStateCommandArgs) -> Result<SwapState> {
    let state = args.state;
    let swap = args.transcript.swap;
    let buyer_receive = load_buyer_receive_section(&swap)?;
    let current = load_state(&state)?;
    current.ensure_role(SwapRole::Seller)?;
    if current.receive.is_none() {
        seller_import_receive_section(&state, buyer_receive)?;
    } else {
        ensure_existing_seller_matches_receive_section(&state, &buyer_receive)?;
    }

    let current = load_state(&state)?;
    if current.preimage.is_none() {
        seller_preimage(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }
    let current = load_state(&state)?;
    if current.claim.is_none() {
        seller_claim(StateCommandArgs {
            state: state.clone(),
        })
        .await?;
    }

    load_state(&state)
}

fn load_seller_offer_section(path: &Path) -> Result<SellerOfferSection> {
    let transcript = load_valid_transcript(path)?;
    transcript.seller_offer.ok_or_else(|| {
        eyre!(
            "swap transcript {} has no seller_offer section",
            path.display()
        )
    })
}

fn load_buyer_addresses_section(path: &Path) -> Result<BuyerAddressesSection> {
    let transcript = load_valid_transcript(path)?;
    transcript.buyer_addresses.ok_or_else(|| {
        eyre!(
            "swap transcript {} has no buyer_addresses section",
            path.display()
        )
    })
}

fn load_seller_template_section(path: &Path) -> Result<SellerTemplateSection> {
    let transcript = load_valid_transcript(path)?;
    transcript.seller_template.ok_or_else(|| {
        eyre!(
            "swap transcript {} has no seller_template section",
            path.display()
        )
    })
}

fn load_buyer_funding_section(path: &Path) -> Result<BuyerFundingSection> {
    let transcript = load_valid_transcript(path)?;
    transcript.buyer_funding.ok_or_else(|| {
        eyre!(
            "swap transcript {} has no buyer_funding section",
            path.display()
        )
    })
}

fn load_seller_unlock_section(path: &Path) -> Result<SellerUnlockSection> {
    let transcript = load_valid_transcript(path)?;
    transcript.seller_unlock.ok_or_else(|| {
        eyre!(
            "swap transcript {} has no seller_unlock section",
            path.display()
        )
    })
}

fn load_buyer_receive_section(path: &Path) -> Result<BuyerReceiveSection> {
    let transcript = load_valid_transcript(path)?;
    transcript.buyer_receive.ok_or_else(|| {
        eyre!(
            "swap transcript {} has no buyer_receive section",
            path.display()
        )
    })
}

fn load_valid_transcript(path: &Path) -> Result<SwapTranscript> {
    let transcript = load_transcript(path)?;
    validate_transcript(&transcript, path)?;
    Ok(transcript)
}

fn save_seller_offer_section(path: &Path, section: SellerOfferSection) -> Result<()> {
    validate_seller_offer_section(&section)?;
    let mut transcript = load_transcript_or_default(path)?;
    validate_transcript(&transcript, path)?;
    if let Some(existing) = &transcript.seller_offer {
        ensure_same_section("seller_offer", existing, &section)?;
        return Ok(());
    }
    transcript.seller_offer = Some(section);
    validate_transcript(&transcript, path)?;
    save_transcript(path, &transcript)
}

fn save_buyer_addresses_section(path: &Path, section: BuyerAddressesSection) -> Result<()> {
    validate_buyer_addresses_section(&section)?;
    let mut transcript = load_transcript(path)?;
    validate_transcript(&transcript, path)?;
    require_seller_offer(&transcript, "buyer_addresses")?;
    if let Some(existing) = &transcript.buyer_addresses {
        ensure_same_section("buyer_addresses", existing, &section)?;
        return Ok(());
    }
    transcript.buyer_addresses = Some(section);
    validate_transcript(&transcript, path)?;
    save_transcript(path, &transcript)
}

fn save_seller_template_section(path: &Path, section: SellerTemplateSection) -> Result<()> {
    let mut transcript = load_transcript(path)?;
    validate_transcript(&transcript, path)?;
    let offer = require_seller_offer(&transcript, "seller_template")?;
    let addresses = transcript
        .buyer_addresses
        .as_ref()
        .ok_or_else(|| eyre!("buyer_addresses section must exist before seller_template"))?;
    validate_seller_template_section(offer, &section)?;
    if addresses.zerosats_address != section.zerosats_address {
        return Err(eyre!(
            "seller_template refund address does not match buyer_addresses"
        ));
    }
    if let Some(existing) = &transcript.seller_template {
        ensure_same_section("seller_template", existing, &section)?;
        return Ok(());
    }
    transcript.seller_template = Some(section);
    validate_transcript(&transcript, path)?;
    save_transcript(path, &transcript)
}

fn save_buyer_funding_section(path: &Path, section: BuyerFundingSection) -> Result<()> {
    validate_buyer_funding_section(&section)?;
    let mut transcript = load_transcript(path)?;
    validate_transcript(&transcript, path)?;
    require_seller_offer(&transcript, "buyer_funding")?;
    let template = transcript
        .seller_template
        .as_ref()
        .ok_or_else(|| eyre!("seller_template section must exist before buyer_funding"))?;
    if template.zerosats_address != section.zerosats_address {
        return Err(eyre!(
            "buyer_funding refund address does not match seller_template"
        ));
    }
    ensure_template_public_match(&template.template, &section.template)?;
    if let Some(existing) = &transcript.buyer_funding {
        ensure_same_section("buyer_funding", existing, &section)?;
        return Ok(());
    }
    transcript.buyer_funding = Some(section);
    validate_transcript(&transcript, path)?;
    save_transcript(path, &transcript)
}

fn save_seller_unlock_section(path: &Path, section: SellerUnlockSection) -> Result<()> {
    let mut transcript = load_transcript(path)?;
    validate_transcript(&transcript, path)?;
    let offer = require_seller_offer(&transcript, "seller_unlock")?;
    let template = transcript
        .seller_template
        .as_ref()
        .ok_or_else(|| eyre!("seller_template section must exist before seller_unlock"))?;
    transcript
        .buyer_funding
        .as_ref()
        .ok_or_else(|| eyre!("buyer_funding section must exist before seller_unlock"))?;
    validate_seller_unlock_section(offer, &section)?;
    ensure_hash_match(
        "seller_unlock latch commitment",
        &template.template.latch_commitment,
        &section.unlock.latch_verify.commitment,
    )?;
    if let Some(existing) = &transcript.seller_unlock {
        ensure_same_section("seller_unlock", existing, &section)?;
        return Ok(());
    }
    transcript.seller_unlock = Some(section);
    validate_transcript(&transcript, path)?;
    save_transcript(path, &transcript)
}

fn save_buyer_receive_section(path: &Path, section: BuyerReceiveSection) -> Result<()> {
    let mut transcript = load_transcript(path)?;
    validate_transcript(&transcript, path)?;
    let offer = require_seller_offer(&transcript, "buyer_receive")?;
    transcript
        .seller_unlock
        .as_ref()
        .ok_or_else(|| eyre!("seller_unlock section must exist before buyer_receive"))?;
    validate_buyer_receive_section(offer, &section)?;
    if let Some(existing) = &transcript.buyer_receive {
        ensure_same_section("buyer_receive", existing, &section)?;
        return Ok(());
    }
    transcript.buyer_receive = Some(section);
    validate_transcript(&transcript, path)?;
    save_transcript(path, &transcript)
}

fn validate_transcript(transcript: &SwapTranscript, path: &Path) -> Result<()> {
    if transcript.version != TRANSCRIPT_VERSION {
        return Err(eyre!(
            "unsupported swap transcript version {} in {}; expected {}",
            transcript.version,
            path.display(),
            TRANSCRIPT_VERSION
        ));
    }

    let offer = match &transcript.seller_offer {
        Some(offer) => {
            validate_seller_offer_section(offer)?;
            offer
        }
        None => {
            if transcript.buyer_addresses.is_some() {
                return Err(eyre!("buyer_addresses exists before seller_offer"));
            }
            if transcript.seller_template.is_some() {
                return Err(eyre!("seller_template exists before seller_offer"));
            }
            if transcript.buyer_funding.is_some() {
                return Err(eyre!("buyer_funding exists before seller_offer"));
            }
            if transcript.seller_unlock.is_some() {
                return Err(eyre!("seller_unlock exists before seller_offer"));
            }
            if transcript.buyer_receive.is_some() {
                return Err(eyre!("buyer_receive exists before seller_offer"));
            }
            return Ok(());
        }
    };

    if let Some(addresses) = &transcript.buyer_addresses {
        validate_buyer_addresses_section(addresses)?;
    }

    if let Some(template) = &transcript.seller_template {
        let addresses = transcript
            .buyer_addresses
            .as_ref()
            .ok_or_else(|| eyre!("seller_template exists before buyer_addresses"))?;
        validate_seller_template_section(offer, template)?;
        if addresses.zerosats_address != template.zerosats_address {
            return Err(eyre!(
                "seller_template refund address does not match buyer_addresses"
            ));
        }
    }

    if let Some(funding) = &transcript.buyer_funding {
        let template = transcript
            .seller_template
            .as_ref()
            .ok_or_else(|| eyre!("buyer_funding exists before seller_template"))?;
        validate_buyer_funding_section(funding)?;
        if template.zerosats_address != funding.zerosats_address {
            return Err(eyre!(
                "buyer_funding refund address does not match seller_template"
            ));
        }
        ensure_template_public_match(&template.template, &funding.template)?;
    }

    if let Some(unlock) = &transcript.seller_unlock {
        let template = transcript
            .seller_template
            .as_ref()
            .ok_or_else(|| eyre!("seller_unlock exists before seller_template"))?;
        transcript
            .buyer_funding
            .as_ref()
            .ok_or_else(|| eyre!("seller_unlock exists before buyer_funding"))?;
        validate_seller_unlock_section(offer, unlock)?;
        ensure_hash_match(
            "seller_unlock latch commitment",
            &template.template.latch_commitment,
            &unlock.unlock.latch_verify.commitment,
        )?;
    }

    if let Some(receive) = &transcript.buyer_receive {
        transcript
            .seller_unlock
            .as_ref()
            .ok_or_else(|| eyre!("buyer_receive exists before seller_unlock"))?;
        validate_buyer_receive_section(offer, receive)?;
    }

    Ok(())
}

fn require_seller_offer<'a>(
    transcript: &'a SwapTranscript,
    section: &str,
) -> Result<&'a SellerOfferSection> {
    transcript
        .seller_offer
        .as_ref()
        .ok_or_else(|| eyre!("seller_offer section must exist before {section}"))
}

fn validate_seller_offer_section(section: &SellerOfferSection) -> Result<()> {
    validate_terms(&section.terms)?;
    validate_create_hash(&section.create_hash)
}

fn validate_buyer_addresses_section(section: &BuyerAddressesSection) -> Result<()> {
    validate_mercury_address(&section.mercury_address)?;
    validate_zerosats_address(&section.zerosats_address)
}

fn validate_seller_template_section(
    offer: &SellerOfferSection,
    section: &SellerTemplateSection,
) -> Result<()> {
    validate_template_public_parts(
        &offer.terms,
        &offer.create_hash,
        &section.zerosats_address,
        &section.template,
        &section.template_note,
    )
}

fn validate_buyer_funding_section(section: &BuyerFundingSection) -> Result<()> {
    validate_zerosats_address(&section.zerosats_address)?;
    validate_template_public_state(&section.template)?;
    validate_fund_public_state(&section.fund)
}

fn validate_seller_unlock_section(
    offer: &SellerOfferSection,
    section: &SellerUnlockSection,
) -> Result<()> {
    validate_transfer_for_create_hash(&offer.create_hash, &section.transfer)?;
    validate_unlock_public_state(&section.unlock)
}

fn validate_buyer_receive_section(
    offer: &SellerOfferSection,
    section: &BuyerReceiveSection,
) -> Result<()> {
    validate_receive_for_terms(&offer.terms, &section.receive)
}

fn ensure_same_section<T: Serialize>(section: &str, existing: &T, next: &T) -> Result<()> {
    let existing = serde_json::to_value(existing)
        .wrap_err_with(|| format!("failed to serialize existing {section} section"))?;
    let next = serde_json::to_value(next)
        .wrap_err_with(|| format!("failed to serialize new {section} section"))?;
    if existing == next {
        Ok(())
    } else {
        Err(eyre!(
            "{section} section already exists with different contents"
        ))
    }
}

fn ensure_template_public_match(
    expected: &TemplatePublicState,
    actual: &TemplatePublicState,
) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(eyre!(
            "buyer_funding template does not match seller_template"
        ))
    }
}

fn state_file_exists(path: &Path) -> Result<bool> {
    path.try_exists()
        .wrap_err_with(|| format!("failed to check {}", path.display()))
}

fn ensure_existing_buyer_matches_offer_section(
    state_path: &Path,
    offer: &SellerOfferSection,
) -> Result<()> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Buyer)?;
    ensure_terms_match(&state, &offer.terms)?;
    ensure_create_hash_match(&state, &offer.create_hash)
}

fn ensure_existing_seller_matches_addresses_section(
    state_path: &Path,
    section: &BuyerAddressesSection,
) -> Result<()> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    validate_mercury_address(&section.mercury_address)?;
    validate_zerosats_address(&section.zerosats_address)?;
    if state.mercury_address()? != &section.mercury_address {
        return Err(eyre!(
            "buyer_addresses Mercury address does not match local state"
        ));
    }
    ensure_zerosats_address_match(&state, &section.zerosats_address)
}

fn ensure_existing_buyer_matches_template_section(
    state_path: &Path,
    section: &SellerTemplateSection,
) -> Result<()> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Buyer)?;
    ensure_zerosats_address_match(&state, &section.zerosats_address)?;
    ensure_template_match(state.template()?, &section.template)?;
    validate_template_public_from_state(&state, &section.template, &section.template_note)
}

fn ensure_existing_seller_matches_funding_section(
    state_path: &Path,
    section: &BuyerFundingSection,
) -> Result<()> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    ensure_zerosats_address_match(&state, &section.zerosats_address)?;
    ensure_template_match(state.template()?, &section.template)?;
    validate_fund_public_state(&section.fund)?;
    if state.fund()?.lock_transaction != section.fund.lock_transaction {
        return Err(eyre!(
            "buyer_funding transaction does not match local state"
        ));
    }
    Ok(())
}

fn ensure_existing_buyer_matches_unlock_section(
    state_path: &Path,
    section: &SellerUnlockSection,
) -> Result<()> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Buyer)?;
    validate_latch_verify_state(&section.unlock.latch_verify)?;
    ensure_transfer_hash_matches_state(&state, &section.transfer)?;
    ensure_unlock_matches_template(&state, &section.unlock)?;
    if !section.unlock.unlocked {
        return Err(eyre!("seller_unlock section says Mercury is not unlocked"));
    }
    ensure_transfer_complete(&state, state.transfer()?)?;
    ensure_unlock_complete(&state, state.unlock()?)?;
    if state.transfer()?.retrieved_hash != section.transfer.retrieved_hash {
        return Err(eyre!(
            "seller_unlock transfer hash does not match local state"
        ));
    }
    if state.unlock()?.latch_verify != section.unlock.latch_verify {
        return Err(eyre!("seller_unlock does not match local state"));
    }
    Ok(())
}

fn ensure_existing_seller_matches_receive_section(
    state_path: &Path,
    section: &BuyerReceiveSection,
) -> Result<()> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    ensure_receive_complete(&state, &section.receive)?;
    ensure_receive_complete(&state, state.receive()?)?;
    if state.receive()?.received_mercury_amount_sat != section.receive.received_mercury_amount_sat
        || state.receive()?.final_receive.received_statechain_ids
            != section.receive.final_receive.received_statechain_ids
        || state.receive()?.final_receive.is_there_batch_locked
            != section.receive.final_receive.is_there_batch_locked
    {
        return Err(eyre!("buyer_receive does not match local state"));
    }
    Ok(())
}

fn seller_init(args: SellerInitArgs) -> Result<SwapState> {
    let terms = SwapTerms {
        statechain_id: args.statechain_id,
        amount_sat: args.amount_sat,
        mercury_amount_sat: args.mercury_amount_sat,
        zerosats_host: args.node.zerosats_host,
        citrea_chain: args.latch.citrea_chain,
        refund_blocks: args.latch.refund_blocks,
        ticker: args.latch.ticker.to_uppercase(),
        btc_explorer: args.latch.btc_explorer,
    };
    validate_terms(&terms)?;
    ensure_new_state_file(&args.state)?;

    let state = SwapState::new(SwapRole::Seller, terms, local_config(args.local)?);
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn seller_create_offer(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Seller)?;
    let mercury = load_mercury(&state.local.mercury_settings_file).await?;

    let payment = mercuryrustlib::lightning_latch::create_pre_image(
        &mercury,
        &state.local.mercury_wallet,
        &state.terms.statechain_id,
    )
    .await
    .wrap_display("failed to create Mercury latch payment hash")?;

    mercury.pool.close().await;

    state.create_hash = Some(CreateHashState {
        payment_hash: normalize_hex(&payment.hash),
        batch_id: payment.batch_id,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

fn seller_offer_section(state_path: &Path) -> Result<SellerOfferSection> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    Ok(SellerOfferSection {
        terms: state.terms.clone(),
        create_hash: validated_create_hash(state.create_hash()?)?,
    })
}

fn buyer_init_from_offer(
    state_path: PathBuf,
    local: LocalWalletArgs,
    offer: SellerOfferSection,
) -> Result<SwapState> {
    validate_terms(&offer.terms)?;
    validate_create_hash(&offer.create_hash)?;

    let mut state = SwapState::new(SwapRole::Buyer, offer.terms, local_config(local)?);
    state.create_hash = Some(offer.create_hash);
    save_state(&state_path, &state)?;
    Ok(state)
}

async fn buyer_mercury_address(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Buyer)?;
    let mercury = load_mercury(&state.local.mercury_settings_file).await?;

    let mercury_transfer_address = mercuryrustlib::transfer_receiver::new_transfer_address(
        &mercury,
        &state.local.mercury_wallet,
    )
    .await
    .wrap_display("failed to create Mercury transfer address")?;

    mercury.pool.close().await;

    state.mercury_address = Some(MercuryAddressState {
        mercury_transfer_address,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

fn buyer_zerosats_address(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Buyer)?;
    let refund_address = latch::latch_address(
        state.local.zerosats_wallet_dir.as_deref(),
        &state.local.zerosats_wallet,
    )
    .wrap_err("failed to load Zerosats wallet")?;

    state.zerosats_address = Some(ZerosatsAddressState {
        refund_address: refund_address.to_string(),
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

fn buyer_addresses_section(state_path: &Path) -> Result<BuyerAddressesSection> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Buyer)?;
    validate_mercury_address(state.mercury_address()?)?;
    validate_zerosats_address(state.zerosats_address()?)?;
    Ok(BuyerAddressesSection {
        mercury_address: state.mercury_address()?.clone(),
        zerosats_address: state.zerosats_address()?.clone(),
    })
}

fn seller_import_addresses_section(
    state_path: &Path,
    section: BuyerAddressesSection,
) -> Result<SwapState> {
    let mut state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    validate_mercury_address(&section.mercury_address)?;
    validate_zerosats_address(&section.zerosats_address)?;

    state.mercury_address = Some(section.mercury_address);
    state.zerosats_address = Some(section.zerosats_address);
    save_state(state_path, &state)?;
    Ok(state)
}

async fn seller_create_template(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Seller)?;
    let create_hash = state.create_hash()?.clone();
    let zerosats_address = state.zerosats_address()?.clone();
    let amount_wei = latch::sats_to_wei(state.terms.amount_sat);
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    let claim_address = latch::latch_address(
        state.local.zerosats_wallet_dir.as_deref(),
        &state.local.zerosats_wallet,
    )
    .wrap_err("failed to load Zerosats wallet")?;
    let refund_address =
        latch::parse_element_arg(&zerosats_address.refund_address, "refund address")?;
    let template_path = template_note_path(&state.local.output_prefix);

    let commitment = latch::create_latch_template(latch::LatchTemplateRequest {
        chain: state.terms.citrea_chain,
        amount_wei,
        ticker: &state.terms.ticker,
        payment_hash_hex: &payment_hash,
        claim_address,
        refund_address,
        output_path: &template_path,
        refund_blocks: state.terms.refund_blocks,
        btc_explorer: &state.terms.btc_explorer,
    })
    .await
    .wrap_err("failed to create Zerosats latch template")?;

    state.template = Some(TemplateState {
        amount_wei,
        claim_address: claim_address.to_string(),
        latch_commitment: commitment.to_string(),
        template_note: template_path,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

fn seller_template_section(state_path: &Path) -> Result<SellerTemplateSection> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    let template = state.template()?;
    let template_note = load_template_note_json(&template.template_note)?;
    validate_template_public_from_state(
        &state,
        &TemplatePublicState::from(template),
        &template_note,
    )?;
    Ok(SellerTemplateSection {
        zerosats_address: state.zerosats_address()?.clone(),
        template: TemplatePublicState::from(template),
        template_note,
    })
}

fn buyer_import_template_section(
    state_path: &Path,
    section: SellerTemplateSection,
) -> Result<SwapState> {
    let mut state = load_state(state_path)?;
    state.ensure_role(SwapRole::Buyer)?;
    ensure_zerosats_address_match(&state, &section.zerosats_address)?;
    validate_template_public_from_state(&state, &section.template, &section.template_note)?;

    let template_path = template_note_path(&state.local.output_prefix);
    save_template_note_json(&template_path, &section.template_note)?;
    state.template = Some(template_from_public(section.template, template_path));
    save_state(state_path, &state)?;
    Ok(state)
}

async fn buyer_fund(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Buyer)?;
    let create_hash = state.create_hash()?.clone();
    let zerosats_address = state.zerosats_address()?.clone();
    let template = state.template()?.clone();
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    verify_mercury_payment_hash(
        &state.local.mercury_settings_file,
        &create_hash.batch_id,
        &payment_hash,
    )
    .await?;
    let claim_address = latch::parse_element_arg(&template.claim_address, "claim address")?;

    let fund = latch::fund_latch(latch::LatchFundRequest {
        chain: state.terms.citrea_chain,
        wallet_dir: state.local.zerosats_wallet_dir.as_deref(),
        wallet_name: &state.local.zerosats_wallet,
        host: &state.terms.zerosats_host,
        btc_explorer: &state.terms.btc_explorer,
        template_path: &template.template_note,
        amount_wei: latch::sats_to_wei(state.terms.amount_sat),
        ticker: &state.terms.ticker,
        payment_hash_hex: &payment_hash,
        claim_address,
        refund_blocks: state.terms.refund_blocks,
        output_prefix: &state.local.output_prefix,
    })
    .await
    .wrap_err("failed to fund Zerosats latch")?;
    ensure_hash_match(
        "funded latch commitment",
        &template.latch_commitment,
        &fund.commitment.to_string(),
    )?;
    ensure_hash_match(
        "fund refund address",
        &zerosats_address.refund_address,
        &fund.refund_address.to_string(),
    )?;

    state.fund = Some(FundState {
        refund_note: Some(fund.refund_path),
        lock_transaction: summarize_tx(&fund.transaction),
        balance_sat: Some(latch::wei_to_sats(fund.balance_wei)),
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

fn buyer_funding_section(state_path: &Path) -> Result<BuyerFundingSection> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Buyer)?;
    validate_zerosats_address(state.zerosats_address()?)?;
    validate_template_state(state.template()?)?;
    validate_fund_state(state.fund()?)?;
    Ok(BuyerFundingSection {
        zerosats_address: state.zerosats_address()?.clone(),
        template: TemplatePublicState::from(state.template()?),
        fund: FundPublicState::from(state.fund()?),
    })
}

fn seller_import_funding_section(
    state_path: &Path,
    section: BuyerFundingSection,
) -> Result<SwapState> {
    let mut state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    ensure_zerosats_address_match(&state, &section.zerosats_address)?;
    ensure_template_match(state.template()?, &section.template)?;
    validate_zerosats_address(&section.zerosats_address)?;
    validate_template_public_state(&section.template)?;
    validate_fund_public_state(&section.fund)?;

    state.fund = Some(FundState {
        refund_note: None,
        lock_transaction: section.fund.lock_transaction,
        balance_sat: None,
    });
    save_state(state_path, &state)?;
    Ok(state)
}

async fn seller_verify(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Seller)?;
    validate_fund_state(state.fund()?)?;
    let latch_verify = verify_latch_from_state(&state).await?;

    state.verify = Some(VerifyState { latch_verify });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn seller_transfer(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Seller)?;
    let create_hash = state.create_hash()?.clone();
    let mercury_address = state.mercury_address()?.clone();
    ensure_verify_matches_template(&state, state.verify()?)?;
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    let mercury = load_mercury(&state.local.mercury_settings_file).await?;

    mercuryrustlib::transfer_sender::execute(
        &mercury,
        &mercury_address.mercury_transfer_address,
        &state.local.mercury_wallet,
        &state.terms.statechain_id,
        None,
        false,
        Some(create_hash.batch_id.clone()),
    )
    .await
    .wrap_display("failed to start Mercury latch transfer")?;

    let retrieved_hash =
        mercuryrustlib::lightning_latch::get_payment_hash(&mercury, &create_hash.batch_id)
            .await
            .wrap_display("failed to retrieve Mercury payment hash by batch id")?
            .ok_or_else(|| {
                eyre!(
                    "Mercury returned no payment hash for batch {}",
                    create_hash.batch_id
                )
            })?;
    let retrieved_hash = normalize_hex(&retrieved_hash);
    ensure_hash_match("Mercury payment hash", &payment_hash, &retrieved_hash)?;

    mercury.pool.close().await;

    state.transfer = Some(TransferState {
        retrieved_hash,
        hash_matches: true,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn seller_unlock(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Seller)?;
    ensure_transfer_complete(&state, state.transfer()?)?;
    let latch_verify = verify_latch_from_state(&state)
        .await
        .wrap_err("fresh latch verification failed before Mercury unlock")?;

    let mercury = load_mercury(&state.local.mercury_settings_file).await?;

    mercuryrustlib::lightning_latch::confirm_pending_invoice(
        &mercury,
        &state.local.mercury_wallet,
        &state.terms.statechain_id,
    )
    .await
    .wrap_display("failed to unlock Mercury latch")?;

    mercury.pool.close().await;

    state.unlock = Some(UnlockState {
        latch_verify,
        unlocked: true,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

fn seller_unlock_section(state_path: &Path) -> Result<SellerUnlockSection> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    ensure_transfer_complete(&state, state.transfer()?)?;
    ensure_unlock_complete(&state, state.unlock()?)?;
    validate_latch_verify_state(&state.unlock()?.latch_verify)?;
    Ok(SellerUnlockSection {
        transfer: state.transfer()?.clone(),
        unlock: state.unlock()?.clone(),
    })
}

fn buyer_import_unlock_section(
    state_path: &Path,
    section: SellerUnlockSection,
) -> Result<SwapState> {
    let mut state = load_state(state_path)?;
    state.ensure_role(SwapRole::Buyer)?;
    state.mercury_address()?;
    state.zerosats_address()?;
    validate_latch_verify_state(&section.unlock.latch_verify)?;
    ensure_transfer_hash_matches_state(&state, &section.transfer)?;
    ensure_unlock_matches_template(&state, &section.unlock)?;
    if !section.unlock.unlocked {
        return Err(eyre!("seller_unlock section says Mercury is not unlocked"));
    }

    let mut transfer = section.transfer;
    transfer.hash_matches = true;
    let mut unlock = section.unlock;
    unlock.unlocked = true;
    state.transfer = Some(transfer);
    state.unlock = Some(unlock);
    save_state(state_path, &state)?;
    Ok(state)
}

async fn buyer_receive(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Buyer)?;
    ensure_transfer_complete(&state, state.transfer()?)?;
    ensure_unlock_complete(&state, state.unlock()?)?;
    let mercury = load_mercury(&state.local.mercury_settings_file).await?;

    let final_receive =
        mercuryrustlib::transfer_receiver::execute(&mercury, &state.local.mercury_wallet)
            .await
            .wrap_display("failed to complete Mercury receive")?;
    let final_receive = summarize_receive(final_receive);
    ensure_received_statechain(&final_receive, &state.terms.statechain_id)?;
    let received_mercury_amount_sat = received_mercury_amount_sat(
        &mercury,
        &state.local.mercury_wallet,
        &state.terms.statechain_id,
    )
    .await?;
    ensure_mercury_amount(state.terms.mercury_amount_sat, received_mercury_amount_sat)?;

    mercury.pool.close().await;

    state.receive = Some(ReceiveState {
        received_mercury_amount_sat,
        received_expected_statechain_id: true,
        final_receive,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

fn buyer_receive_section(state_path: &Path) -> Result<BuyerReceiveSection> {
    let state = load_state(state_path)?;
    state.ensure_role(SwapRole::Buyer)?;
    ensure_receive_complete(&state, state.receive()?)?;
    Ok(BuyerReceiveSection {
        receive: state.receive()?.clone(),
    })
}

fn seller_import_receive_section(
    state_path: &Path,
    section: BuyerReceiveSection,
) -> Result<SwapState> {
    let mut state = load_state(state_path)?;
    state.ensure_role(SwapRole::Seller)?;
    ensure_transfer_complete(&state, state.transfer()?)?;
    ensure_unlock_complete(&state, state.unlock()?)?;
    ensure_received_statechain(&section.receive.final_receive, &state.terms.statechain_id)?;
    ensure_mercury_amount(
        state.terms.mercury_amount_sat,
        section.receive.received_mercury_amount_sat,
    )?;

    let mut receive = section.receive;
    receive.received_expected_statechain_id = true;
    state.receive = Some(receive);
    save_state(state_path, &state)?;
    Ok(state)
}

async fn seller_preimage(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Seller)?;
    let create_hash = state.create_hash()?.clone();
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    ensure_seller_can_request_preimage(&state)?;
    let mercury = load_mercury(&state.local.mercury_settings_file).await?;

    let preimage = mercuryrustlib::lightning_latch::retrieve_pre_image(
        &mercury,
        &state.local.mercury_wallet,
        &state.terms.statechain_id,
        &create_hash.batch_id,
    )
    .await
    .wrap_display("failed to retrieve Mercury latch preimage")?;

    mercury.pool.close().await;

    let preimage_bytes = latch::parse_preimage_hex(&preimage)?;
    let preimage_hash = hex::encode(Sha256::digest(preimage_bytes));
    ensure_hash_match("preimage hash", &payment_hash, &preimage_hash)?;

    state.preimage = Some(PreimageState {
        preimage,
        preimage_hash,
        hash_matches: true,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn seller_claim(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    state.ensure_role(SwapRole::Seller)?;
    let template = state.template()?.clone();
    let preimage = state.preimage()?.clone();
    ensure_seller_can_claim(&state, &preimage)?;

    let claim = latch::claim_latch(latch::LatchClaimRequest {
        chain: state.terms.citrea_chain,
        wallet_dir: state.local.zerosats_wallet_dir.as_deref(),
        wallet_name: &state.local.zerosats_wallet,
        host: &state.terms.zerosats_host,
        note_path: &template.template_note,
        preimage_hex: &preimage.preimage,
    })
    .await
    .wrap_err("failed to claim Zerosats latch")?;

    state.claim = Some(ClaimState {
        claim_transaction: summarize_tx(&claim.transaction),
        balance_sat: latch::wei_to_sats(claim.balance_wei),
        ticker: claim.ticker,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn verify_latch_from_state(state: &SwapState) -> Result<LatchVerifyState> {
    let create_hash = state.create_hash()?.clone();
    let zerosats_address = state.zerosats_address()?.clone();
    let template = state.template()?.clone();
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    let refund_address =
        latch::parse_element_arg(&zerosats_address.refund_address, "refund address")?;

    let verified = latch::verify_latch(latch::LatchVerifyRequest {
        chain: state.terms.citrea_chain,
        wallet_dir: state.local.zerosats_wallet_dir.as_deref(),
        wallet_name: &state.local.zerosats_wallet,
        host: &state.terms.zerosats_host,
        btc_explorer: &state.terms.btc_explorer,
        note_path: &template.template_note,
        amount_wei: latch::sats_to_wei(state.terms.amount_sat),
        ticker: &state.terms.ticker,
        payment_hash_hex: &payment_hash,
        refund_address,
        refund_blocks: state.terms.refund_blocks,
    })
    .await
    .wrap_err("failed to verify funded Zerosats latch")?;

    Ok(summarize_latch_verify(verified))
}

fn local_config(args: LocalWalletArgs) -> Result<LocalConfig> {
    ensure_non_empty("--mercury-wallet", &args.mercury_wallet)?;
    ensure_non_empty("--zerosats-wallet", &args.zerosats_wallet)?;
    ensure_output_prefix(&args.output_prefix)?;

    let mercury_settings_file = args
        .mercury_settings_file
        .canonicalize()
        .wrap_err_with(|| format!("failed to resolve {}", args.mercury_settings_file.display()))?;

    Ok(LocalConfig {
        mercury_settings_file,
        mercury_wallet: args.mercury_wallet,
        zerosats_wallet: args.zerosats_wallet,
        zerosats_wallet_dir: args.zerosats_wallet_dir,
        output_prefix: args.output_prefix,
    })
}

fn ensure_new_state_file(path: &Path) -> Result<()> {
    if path
        .try_exists()
        .wrap_err_with(|| format!("failed to check {}", path.display()))?
    {
        return Err(eyre!(
            "swap state {} already exists; use a new --state path for each swap",
            path.display()
        ));
    }
    Ok(())
}

fn validate_terms(terms: &SwapTerms) -> Result<()> {
    ensure_non_empty("statechain_id", &terms.statechain_id)?;
    ensure_amount(terms.amount_sat)?;
    ensure_amount(terms.mercury_amount_sat)?;
    ensure_non_empty("zerosats_host", &terms.zerosats_host)?;
    ensure_refund_blocks(terms.refund_blocks)?;
    ensure_non_empty("ticker", &terms.ticker)?;
    ensure_non_empty("btc_explorer", &terms.btc_explorer)?;
    Ok(())
}

fn validate_create_hash(create_hash: &CreateHashState) -> Result<()> {
    validate_payment_hash(&create_hash.payment_hash)?;
    ensure_non_empty("batch_id", &create_hash.batch_id)
}

fn validated_create_hash(create_hash: &CreateHashState) -> Result<CreateHashState> {
    validate_create_hash(create_hash)?;
    Ok(create_hash.clone())
}

fn validate_mercury_address(address: &MercuryAddressState) -> Result<()> {
    ensure_non_empty(
        "mercury_transfer_address",
        &address.mercury_transfer_address,
    )
}

fn validate_zerosats_address(address: &ZerosatsAddressState) -> Result<()> {
    latch::parse_element_arg(&address.refund_address, "refund address")?;
    Ok(())
}

fn validate_template_state(template: &TemplateState) -> Result<()> {
    ensure_amount(template.amount_wei)?;
    latch::parse_element_arg(&template.claim_address, "claim address")?;
    latch::parse_element_arg(&template.latch_commitment, "latch commitment")?;
    Ok(())
}

fn validate_template_public_state(template: &TemplatePublicState) -> Result<()> {
    ensure_amount(template.amount_wei)?;
    latch::parse_element_arg(&template.claim_address, "claim address")?;
    latch::parse_element_arg(&template.latch_commitment, "latch commitment")?;
    Ok(())
}

fn validate_fund_state(fund: &FundState) -> Result<()> {
    validate_transaction_state(&fund.lock_transaction)
}

fn validate_fund_public_state(fund: &FundPublicState) -> Result<()> {
    validate_transaction_state(&fund.lock_transaction)
}

fn validate_latch_verify_state(verify: &LatchVerifyState) -> Result<()> {
    latch::parse_element_arg(&verify.commitment, "latch commitment")?;
    if verify.height == 0 {
        return Err(eyre!("latch verification height must be greater than zero"));
    }
    ensure_non_empty("latch verification root_hash", &verify.root_hash)?;
    ensure_non_empty("latch verification txn_hash", &verify.txn_hash)
}

fn validate_transaction_state(tx: &TransactionState) -> Result<()> {
    ensure_non_empty("transaction txn_hash", &tx.txn_hash)?;
    ensure_non_empty("transaction height", &tx.height)?;
    ensure_non_empty("transaction root_hash", &tx.root_hash)
}

fn ensure_terms_match(state: &SwapState, terms: &SwapTerms) -> Result<()> {
    validate_terms(terms)?;
    if &state.terms == terms {
        Ok(())
    } else {
        Err(eyre!(
            "section terms do not match local state: local={:?}, section={:?}",
            state.terms,
            terms
        ))
    }
}

fn ensure_create_hash_match(state: &SwapState, create_hash: &CreateHashState) -> Result<()> {
    validate_create_hash(create_hash)?;
    let local = state.create_hash()?;
    validate_create_hash(local)?;
    if local == create_hash {
        Ok(())
    } else {
        Err(eyre!(
            "section payment hash or batch id does not match local state"
        ))
    }
}

fn ensure_zerosats_address_match(
    state: &SwapState,
    zerosats_address: &ZerosatsAddressState,
) -> Result<()> {
    let local = state.zerosats_address()?;
    if local == zerosats_address {
        Ok(())
    } else {
        Err(eyre!("section refund address does not match local state"))
    }
}

fn ensure_template_match(local: &TemplateState, section: &TemplatePublicState) -> Result<()> {
    if local.amount_wei != section.amount_wei {
        return Err(eyre!(
            "section template amount mismatch: local {}, section {}",
            local.amount_wei,
            section.amount_wei
        ));
    }
    if local.claim_address != section.claim_address {
        return Err(eyre!(
            "section template claim address does not match local state"
        ));
    }
    if local.latch_commitment != section.latch_commitment {
        return Err(eyre!(
            "section template commitment does not match local state"
        ));
    }
    Ok(())
}

fn ensure_verify_matches_template(state: &SwapState, verify: &VerifyState) -> Result<()> {
    ensure_hash_match(
        "verify latch commitment",
        &state.template()?.latch_commitment,
        &verify.latch_verify.commitment,
    )
}

fn validate_template_public_from_state(
    state: &SwapState,
    template: &TemplatePublicState,
    template_note: &serde_json::Value,
) -> Result<()> {
    validate_template_public_parts(
        &state.terms,
        state.create_hash()?,
        state.zerosats_address()?,
        template,
        template_note,
    )
}

fn validate_template_public_parts(
    terms: &SwapTerms,
    create_hash: &CreateHashState,
    zerosats_address: &ZerosatsAddressState,
    template: &TemplatePublicState,
    template_note: &serde_json::Value,
) -> Result<()> {
    validate_terms(terms)?;
    validate_create_hash(create_hash)?;
    validate_zerosats_address(zerosats_address)?;
    validate_template_public_state(template)?;

    let expected_amount_wei = latch::sats_to_wei(terms.amount_sat);
    if template.amount_wei != expected_amount_wei {
        return Err(eyre!(
            "section template amount mismatch: expected {}, got {}",
            expected_amount_wei,
            template.amount_wei
        ));
    }

    let payment_hash = latch::parse_payment_hash_hex(&create_hash.payment_hash)?;
    let claim_address = latch::parse_element_arg(&template.claim_address, "claim address")?;
    let refund_address =
        latch::parse_element_arg(&zerosats_address.refund_address, "refund address")?;
    let commitment = latch::validate_latch_template_json(
        template_note,
        terms.citrea_chain,
        expected_amount_wei,
        &terms.ticker,
        payment_hash,
        claim_address,
        refund_address,
        terms.refund_blocks,
    )
    .wrap_err("failed to validate latch template section")?;
    ensure_hash_match(
        "latch template commitment",
        &template.latch_commitment,
        &commitment.to_string(),
    )
}

fn validate_transfer_for_create_hash(
    create_hash: &CreateHashState,
    transfer: &TransferState,
) -> Result<()> {
    validate_create_hash(create_hash)?;
    ensure_hash_match(
        "Mercury payment hash",
        &create_hash.payment_hash,
        &transfer.retrieved_hash,
    )?;
    if !transfer.hash_matches {
        return Err(eyre!("Mercury transfer payment hash has not been verified"));
    }
    Ok(())
}

fn validate_unlock_public_state(unlock: &UnlockState) -> Result<()> {
    validate_latch_verify_state(&unlock.latch_verify)?;
    if !unlock.unlocked {
        return Err(eyre!("Mercury transfer is not unlocked"));
    }
    Ok(())
}

fn validate_receive_for_terms(terms: &SwapTerms, receive: &ReceiveState) -> Result<()> {
    validate_terms(terms)?;
    ensure_receive_values(&terms.statechain_id, terms.mercury_amount_sat, receive)
}

fn ensure_transfer_hash_matches_state(state: &SwapState, transfer: &TransferState) -> Result<()> {
    ensure_hash_match(
        "Mercury payment hash",
        &state.create_hash()?.payment_hash,
        &transfer.retrieved_hash,
    )
}

fn ensure_transfer_complete(state: &SwapState, transfer: &TransferState) -> Result<()> {
    ensure_transfer_hash_matches_state(state, transfer)?;
    if !transfer.hash_matches {
        return Err(eyre!("Mercury transfer payment hash has not been verified"));
    }
    Ok(())
}

fn ensure_unlock_matches_template(state: &SwapState, unlock: &UnlockState) -> Result<()> {
    ensure_hash_match(
        "unlock latch commitment",
        &state.template()?.latch_commitment,
        &unlock.latch_verify.commitment,
    )
}

fn ensure_unlock_complete(state: &SwapState, unlock: &UnlockState) -> Result<()> {
    ensure_unlock_matches_template(state, unlock)?;
    if !unlock.unlocked {
        return Err(eyre!("Mercury transfer is not unlocked"));
    }
    Ok(())
}

fn ensure_receive_complete(state: &SwapState, receive: &ReceiveState) -> Result<()> {
    ensure_receive_values(
        &state.terms.statechain_id,
        state.terms.mercury_amount_sat,
        receive,
    )
}

fn ensure_receive_values(
    statechain_id: &str,
    mercury_amount_sat: u64,
    receive: &ReceiveState,
) -> Result<()> {
    ensure_received_statechain(&receive.final_receive, statechain_id)?;
    ensure_mercury_amount(mercury_amount_sat, receive.received_mercury_amount_sat)?;
    if !receive.received_expected_statechain_id {
        return Err(eyre!(
            "receive step did not confirm the expected statechain_id"
        ));
    }
    Ok(())
}

fn ensure_seller_can_request_preimage(state: &SwapState) -> Result<()> {
    ensure_transfer_complete(state, state.transfer()?)?;
    ensure_unlock_complete(state, state.unlock()?)?;
    ensure_state_allows_preimage(state)
}

fn ensure_seller_can_claim(state: &SwapState, preimage: &PreimageState) -> Result<()> {
    ensure_seller_can_request_preimage(state)?;
    ensure_preimage_matches_state(state, preimage)
}

fn ensure_preimage_matches_state(state: &SwapState, preimage: &PreimageState) -> Result<()> {
    let payment_hash = validate_payment_hash(&state.create_hash()?.payment_hash)?;
    let preimage_bytes = latch::parse_preimage_hex(&preimage.preimage)?;
    let preimage_hash = hex::encode(Sha256::digest(preimage_bytes));
    ensure_hash_match("preimage hash", &payment_hash, &preimage_hash)?;
    ensure_hash_match(
        "stored preimage hash",
        &preimage_hash,
        &preimage.preimage_hash,
    )?;
    if !preimage.hash_matches {
        return Err(eyre!("preimage hash has not been verified"));
    }
    Ok(())
}

fn template_from_public(template: TemplatePublicState, template_note: PathBuf) -> TemplateState {
    TemplateState {
        amount_wei: template.amount_wei,
        claim_address: template.claim_address,
        latch_commitment: template.latch_commitment,
        template_note,
    }
}

fn load_template_note_json(path: &Path) -> Result<serde_json::Value> {
    let json =
        fs::read_to_string(path).wrap_err_with(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&json).wrap_err_with(|| format!("failed to parse {}", path.display()))
}

fn save_template_note_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
        }
    }
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))
        .wrap_err_with(|| format!("failed to write {}", path.display()))
}

fn ensure_amount(amount_sat: u64) -> Result<()> {
    if amount_sat == 0 {
        return Err(eyre!("amount must be greater than zero"));
    }
    Ok(())
}

fn ensure_refund_blocks(refund_blocks: u64) -> Result<()> {
    if refund_blocks == 0 || refund_blocks > 2 {
        return Err(eyre!(
            "--refund-blocks must be between 1 and 2 with the current escrow circuit"
        ));
    }
    Ok(())
}

fn ensure_non_empty(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(eyre!("{name} is required"));
    }
    Ok(())
}

fn ensure_output_prefix(output_prefix: &Path) -> Result<()> {
    if output_prefix.as_os_str().is_empty() {
        return Err(eyre!("--output-prefix is required"));
    }
    Ok(())
}

fn validate_payment_hash(value: &str) -> Result<String> {
    Ok(hex::encode(latch::parse_payment_hash_hex(value)?))
}

fn normalize_hex(value: &str) -> String {
    value.trim().trim_start_matches("0x").to_ascii_lowercase()
}

fn ensure_hash_match(label: &str, expected: &str, actual: &str) -> Result<()> {
    if expected.eq_ignore_ascii_case(actual) {
        Ok(())
    } else {
        Err(eyre!(
            "{label} mismatch: expected {}, got {}",
            expected,
            actual
        ))
    }
}

fn ensure_mercury_amount(expected_amount_sat: u64, actual_amount_sat: u64) -> Result<()> {
    if expected_amount_sat == actual_amount_sat {
        Ok(())
    } else {
        Err(eyre!(
            "Mercury amount mismatch: expected {} sats, got {} sats",
            expected_amount_sat,
            actual_amount_sat
        ))
    }
}

async fn received_mercury_amount_sat(
    mercury: &mercuryrustlib::client_config::ClientConfig,
    wallet_name: &str,
    statechain_id: &str,
) -> Result<u64> {
    let wallet = mercuryrustlib::sqlite_manager::get_wallet(&mercury.pool, wallet_name)
        .await
        .wrap_display("failed to load Mercury receiver wallet after receive")?;
    let coin = wallet
        .coins
        .iter()
        .find(|coin| coin.statechain_id.as_deref() == Some(statechain_id))
        .ok_or_else(|| {
            eyre!(
                "received statechain_id {} was not found in Mercury wallet {}",
                statechain_id,
                wallet_name
            )
        })?;
    coin.amount
        .map(u64::from)
        .ok_or_else(|| eyre!("received statechain_id {statechain_id} has no Mercury amount"))
}

async fn verify_mercury_payment_hash(
    mercury_settings_file: &Path,
    batch_id: &str,
    expected_payment_hash: &str,
) -> Result<String> {
    let mercury = load_mercury(mercury_settings_file).await?;
    let retrieved_hash = mercuryrustlib::lightning_latch::get_payment_hash(&mercury, batch_id)
        .await
        .wrap_display("failed to retrieve Mercury payment hash by batch id")?
        .ok_or_else(|| eyre!("Mercury returned no payment hash for batch {batch_id}"))?;
    let retrieved_hash = normalize_hex(&retrieved_hash);
    mercury.pool.close().await;

    ensure_hash_match(
        "Mercury payment hash",
        expected_payment_hash,
        &retrieved_hash,
    )?;
    Ok(retrieved_hash)
}

fn template_note_path(prefix: &Path) -> PathBuf {
    latch::path_with_suffix(prefix, "-template.json")
}

#[cfg(test)]
mod tests {
    use ::cli::address::{citrea_token_data, network_for_chain};
    use element::Element;
    use hash::hash_merge;
    use zk_primitives::{EscrowInputNote, Note, TimeLock, TimeProof};

    use super::*;

    fn swap_state(
        role: SwapRole,
        statechain_id: &str,
        expected_mercury_amount_sat: u64,
        received_mercury_amount_sat: u64,
        is_there_batch_locked: bool,
    ) -> SwapState {
        let mut state = SwapState::new(
            role,
            SwapTerms {
                statechain_id: statechain_id.to_string(),
                amount_sat: 1_000,
                mercury_amount_sat: expected_mercury_amount_sat,
                zerosats_host: "http://localhost:3000".to_string(),
                citrea_chain: 5115,
                refund_blocks: 2,
                ticker: "WCBTC".to_string(),
                btc_explorer: "http://localhost:8080".to_string(),
            },
            LocalConfig {
                mercury_settings_file: PathBuf::from("/tmp/mercury-settings.toml"),
                mercury_wallet: "seller".to_string(),
                zerosats_wallet: "seller".to_string(),
                zerosats_wallet_dir: Some(PathBuf::from("/tmp/wallets")),
                output_prefix: PathBuf::from("/tmp/swap"),
            },
        );
        state.receive = Some(ReceiveState {
            received_mercury_amount_sat,
            received_expected_statechain_id: !is_there_batch_locked,
            final_receive: MercuryReceiveState {
                is_there_batch_locked,
                received_statechain_ids: if is_there_batch_locked {
                    vec![]
                } else {
                    vec![statechain_id.to_string()]
                },
            },
        });
        state
    }

    fn completed_hash() -> CreateHashState {
        CreateHashState {
            payment_hash: hex::encode([42u8; 32]),
            batch_id: "batch-demo".to_string(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mercury-latch-swap-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn local_wallet_args(dir: &Path, output_prefix: PathBuf) -> LocalWalletArgs {
        let mercury_settings_file = dir.join("Settings.toml");
        fs::write(&mercury_settings_file, "").unwrap();
        LocalWalletArgs {
            mercury_settings_file,
            mercury_wallet: "buyer-mercury".to_string(),
            zerosats_wallet: "buyer-zerosats".to_string(),
            zerosats_wallet_dir: Some(dir.join("wallets")),
            output_prefix,
        }
    }

    fn valid_template_public(
        state: &SwapState,
        claim_key_hash: Element,
        refund_key_hash: Element,
    ) -> (TemplatePublicState, serde_json::Value) {
        let payment_hash = [42u8; 32];
        let amount_wei = latch::sats_to_wei(state.terms.amount_sat);
        let lock = TimeLock {
            zero_block: [9u8; 32],
            n_blocks: Element::from(state.terms.refund_blocks),
        };
        let (utxo_kind, note_kind) = citrea_token_data(
            network_for_chain(state.terms.citrea_chain),
            &state.terms.ticker,
        );
        let payment_hash_element = Element::from_be_bytes(payment_hash);
        let (high, low) = payment_hash_element.decompose_be();
        let note = Note {
            utxo_kind,
            note_kind,
            address: hash_merge([claim_key_hash, high, low]),
            psi: hash_merge([refund_key_hash, lock.commitment()]),
            value: Element::from(amount_wei),
        };
        let input_note = EscrowInputNote {
            note: note.clone(),
            spend_type: 3,
            secret_key: Element::ZERO,
            preimage: [0u8; 32],
            time_proof: TimeProof {
                lock,
                ..Default::default()
            },
        };

        (
            TemplatePublicState {
                amount_wei,
                claim_address: claim_key_hash.to_string(),
                latch_commitment: note.commitment().to_string(),
            },
            serde_json::to_value(input_note).unwrap(),
        )
    }

    #[test]
    fn state_allows_preimage_after_successful_receive() {
        let state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);

        ensure_state_allows_preimage(&state).unwrap();
    }

    #[test]
    fn state_rejects_batch_locked_receive() {
        let state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 0, true);
        let err = ensure_state_allows_preimage(&state)
            .unwrap_err()
            .to_string();

        assert!(err.contains("did not confirm the expected statechain_id"));
    }

    #[test]
    fn state_rejects_wrong_statechain_for_preimage_release() {
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state
            .receive
            .as_mut()
            .unwrap()
            .final_receive
            .received_statechain_ids = vec!["other-statechain".to_string()];

        let err = ensure_state_allows_preimage(&state)
            .unwrap_err()
            .to_string();

        assert!(err.contains("Mercury receive did not return expected statechain_id"));
    }

    #[test]
    fn state_rejects_wrong_mercury_amount_for_preimage_release() {
        let state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 9_999, false);

        let err = ensure_state_allows_preimage(&state)
            .unwrap_err()
            .to_string();

        assert!(err.contains("received Mercury amount mismatch"));
    }

    #[test]
    fn ensure_hash_match_rejects_mismatched_hashes() {
        let err = ensure_hash_match(
            "Mercury payment hash",
            &hex::encode([42u8; 32]),
            &hex::encode([7u8; 32]),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("Mercury payment hash mismatch"));
    }

    #[test]
    fn seller_status_shows_offer_export_and_waits_for_buyer_addresses() {
        let dir = temp_dir("seller-status-waits-addresses");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        save_state(&state_path, &state).unwrap();

        let status = status(StateCommandArgs { state: state_path }).unwrap();

        assert_eq!(status.role, SwapRole::Seller);
        assert_eq!(status.completed_steps, vec!["offer"]);
        assert_eq!(status.next_local_command, None);
        let waiting_for = status.waiting_for.unwrap();
        assert_eq!(waiting_for.file, SWAP_TRANSCRIPT_FILE);
        assert_eq!(waiting_for.command, "seller template --swap <file>");
        assert_eq!(status.available_exports.len(), 1);
        assert_eq!(status.available_exports[0].file, SWAP_TRANSCRIPT_FILE);
        assert_eq!(
            status.available_exports[0].command,
            "seller offer --swap <file>"
        );
    }

    #[test]
    fn seller_status_after_unlock_waits_for_buyer_receive() {
        let dir = temp_dir("seller-status-waits-receive");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        state.mercury_address = Some(MercuryAddressState {
            mercury_transfer_address: "buyer-mercury-address".to_string(),
        });
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("seller-template.json"),
        ));
        state.fund = Some(FundState {
            refund_note: None,
            lock_transaction: TransactionState {
                txn_hash: "tx".to_string(),
                height: "100".to_string(),
                root_hash: "root".to_string(),
            },
            balance_sat: None,
        });
        state.verify = Some(VerifyState {
            latch_verify: LatchVerifyState {
                commitment: template.latch_commitment.clone(),
                height: 101,
                root_hash: "root".to_string(),
                txn_hash: "tx".to_string(),
            },
        });
        add_completed_seller_unlock_state(&mut state, template.latch_commitment);
        state.receive = None;
        save_state(&state_path, &state).unwrap();

        let status = status(StateCommandArgs { state: state_path }).unwrap();

        assert_eq!(status.role, SwapRole::Seller);
        assert_eq!(status.next_local_command, None);
        let waiting_for = status.waiting_for.unwrap();
        assert_eq!(waiting_for.file, SWAP_TRANSCRIPT_FILE);
        assert_eq!(waiting_for.command, "buyer receive --swap <file>");
        assert!(status.completed_steps.contains(&"mercury-unlock"));
    }

    #[test]
    fn buyer_status_shows_funding_export_and_waits_for_unlock() {
        let dir = temp_dir("buyer-status-waits-unlock");
        let state_path = dir.join("buyer-state.json");
        let mut state = swap_state(SwapRole::Buyer, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        state.mercury_address = Some(MercuryAddressState {
            mercury_transfer_address: "buyer-mercury-address".to_string(),
        });
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template,
            dir.join("buyer-template.json"),
        ));
        state.fund = Some(FundState {
            refund_note: Some(dir.join("buyer-refund.json")),
            lock_transaction: TransactionState {
                txn_hash: "tx".to_string(),
                height: "100".to_string(),
                root_hash: "root".to_string(),
            },
            balance_sat: Some(42),
        });
        save_state(&state_path, &state).unwrap();

        let status = status(StateCommandArgs { state: state_path }).unwrap();

        assert_eq!(status.role, SwapRole::Buyer);
        assert_eq!(status.next_local_command, None);
        let waiting_for = status.waiting_for.unwrap();
        assert_eq!(waiting_for.file, SWAP_TRANSCRIPT_FILE);
        assert_eq!(waiting_for.command, "buyer receive --swap <file>");
        assert!(status.completed_steps.contains(&"funding"));
        assert!(
            status
                .available_exports
                .iter()
                .any(|export| export.command == "buyer fund --swap <file>")
        );
    }

    #[test]
    fn status_output_omits_local_wallet_configuration() {
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.local = LocalConfig {
            mercury_settings_file: PathBuf::from("/private/local/Settings.toml"),
            mercury_wallet: "private-mercury-wallet".to_string(),
            zerosats_wallet: "private-zerosats-wallet".to_string(),
            zerosats_wallet_dir: Some(PathBuf::from("/private/local/zerosats-wallets")),
            output_prefix: PathBuf::from("/private/local/swap-output"),
        };

        let json = output(Ok(seller_status(&state))).unwrap().to_string();

        assert!(!json.contains("/private/local"));
        assert!(!json.contains("private-mercury-wallet"));
        assert!(!json.contains("private-zerosats-wallet"));
    }

    #[test]
    fn inspect_swap_identifies_next_command_after_offer() {
        let dir = temp_dir("inspect-swap-offer");
        let swap_path = dir.join("swap.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        save_transcript(
            &swap_path,
            &SwapTranscript {
                seller_offer: Some(SellerOfferSection {
                    terms: state.terms.clone(),
                    create_hash: state.create_hash().unwrap().clone(),
                }),
                ..SwapTranscript::default()
            },
        )
        .unwrap();

        let inspection = inspect_swap(SwapCommandArgs { swap: swap_path }).unwrap();

        assert_eq!(inspection.statechain_id.as_deref(), Some("statechain-demo"));
        assert_eq!(inspection.completed_sections, vec!["seller_offer"]);
        assert_eq!(inspection.next_command, Some("buyer accept --swap <file>"));
        assert!(inspection.safe_contents.contains(&"payment hash"));
    }

    #[test]
    fn inspect_swap_omits_embedded_template_json() {
        let dir = temp_dir("inspect-swap-template-omits-json");
        let swap_path = dir.join("swap.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, mut template_note) =
            valid_template_public(&state, Element::new(11), refund_key_hash);
        template_note["operator_note"] = serde_json::json!("do-not-print-this");
        save_transcript(
            &swap_path,
            &SwapTranscript {
                seller_offer: Some(SellerOfferSection {
                    terms: state.terms.clone(),
                    create_hash: state.create_hash().unwrap().clone(),
                }),
                buyer_addresses: Some(BuyerAddressesSection {
                    mercury_address: MercuryAddressState {
                        mercury_transfer_address: "buyer-mercury-address".to_string(),
                    },
                    zerosats_address: state.zerosats_address().unwrap().clone(),
                }),
                seller_template: Some(SellerTemplateSection {
                    zerosats_address: state.zerosats_address().unwrap().clone(),
                    template,
                    template_note,
                }),
                ..SwapTranscript::default()
            },
        )
        .unwrap();

        let json = output(inspect_swap(SwapCommandArgs { swap: swap_path }))
            .unwrap()
            .to_string();

        assert!(json.contains("seller_template"));
        assert!(!json.contains("do-not-print-this"));
        assert!(!json.contains("template_note"));
    }

    #[test]
    fn inspect_swap_rejects_unknown_private_fields() {
        let dir = temp_dir("inspect-swap-rejects-private");
        let swap_path = dir.join("swap.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        let mut transcript = serde_json::to_value(SwapTranscript {
            seller_offer: Some(SellerOfferSection {
                terms: state.terms.clone(),
                create_hash: state.create_hash().unwrap().clone(),
            }),
            buyer_addresses: Some(BuyerAddressesSection {
                mercury_address: MercuryAddressState {
                    mercury_transfer_address: "buyer-mercury-address".to_string(),
                },
                zerosats_address: state.zerosats_address().unwrap().clone(),
            }),
            seller_template: Some(SellerTemplateSection {
                zerosats_address: state.zerosats_address().unwrap().clone(),
                template: template.clone(),
                template_note: serde_json::json!({}),
            }),
            buyer_funding: Some(BuyerFundingSection {
                zerosats_address: state.zerosats_address().unwrap().clone(),
                template,
                fund: FundPublicState {
                    lock_transaction: TransactionState {
                        txn_hash: "tx".to_string(),
                        height: "100".to_string(),
                        root_hash: "root".to_string(),
                    },
                },
            }),
            ..SwapTranscript::default()
        })
        .unwrap();
        transcript["buyer_funding"]["fund"]["refund_note"] =
            serde_json::json!("/private/refund.json");
        fs::write(
            &swap_path,
            serde_json::to_string_pretty(&transcript).unwrap(),
        )
        .unwrap();

        let err = inspect_swap(SwapCommandArgs { swap: swap_path })
            .unwrap_err()
            .to_string();

        assert!(err.contains("failed to parse swap transcript"));
    }

    #[test]
    fn seller_init_refuses_existing_state_file() {
        let dir = temp_dir("seller-init-existing-state");
        let state_path = dir.join("seller-state.json");
        fs::write(&state_path, "{}").unwrap();

        let err = seller_init(SellerInitArgs {
            state: state_path.clone(),
            statechain_id: "statechain-demo".to_string(),
            amount_sat: 1_000,
            mercury_amount_sat: 10_000,
            local: local_wallet_args(&dir, dir.join("seller/swap")),
            node: ZerosatsHostArgs {
                zerosats_host: "http://localhost:3000".to_string(),
            },
            latch: LatchSettingsArgs {
                citrea_chain: 5115,
                refund_blocks: 2,
                ticker: "WCBTC".to_string(),
                btc_explorer: "http://localhost:8080".to_string(),
            },
        })
        .unwrap_err()
        .to_string();

        assert!(err.contains("already exists"));
        assert!(err.contains("use a new --state path"));
    }

    #[tokio::test]
    async fn buyer_accept_reexports_existing_addresses_to_transcript() {
        let dir = temp_dir("buyer-accept-existing");
        let swap_path = dir.join("swap.json");
        let state_path = dir.join("buyer-state.json");
        let mut state = swap_state(SwapRole::Buyer, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        state.mercury_address = Some(MercuryAddressState {
            mercury_transfer_address: "buyer-mercury-address".to_string(),
        });
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: Element::new(12).to_string(),
        });
        save_state(&state_path, &state).unwrap();
        save_transcript(
            &swap_path,
            &SwapTranscript {
                seller_offer: Some(SellerOfferSection {
                    terms: state.terms.clone(),
                    create_hash: state.create_hash().unwrap().clone(),
                }),
                ..SwapTranscript::default()
            },
        )
        .unwrap();

        let section = buyer_accept(BuyerAcceptArgs {
            state: state_path,
            transcript: SwapCommandArgs {
                swap: swap_path.clone(),
            },
            local: local_wallet_args(&dir, dir.join("buyer/swap")),
        })
        .await
        .unwrap();

        let transcript = load_transcript(&swap_path).unwrap();
        let saved = transcript.buyer_addresses.unwrap();
        assert_eq!(
            section.mercury_address,
            state.mercury_address.clone().unwrap()
        );
        assert_eq!(saved.mercury_address, state.mercury_address.unwrap());
        assert_eq!(saved.zerosats_address, state.zerosats_address.unwrap());
    }

    #[tokio::test]
    async fn seller_template_rejects_mismatched_addresses_without_updating_transcript() {
        let dir = temp_dir("seller-template-mismatched-addresses");
        let state_path = dir.join("seller-state.json");
        let swap_path = dir.join("swap.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        state.mercury_address = Some(MercuryAddressState {
            mercury_transfer_address: "buyer-mercury-address".to_string(),
        });
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template,
            dir.join("seller-template-note.json"),
        ));
        save_state(&state_path, &state).unwrap();
        save_transcript(
            &swap_path,
            &SwapTranscript {
                seller_offer: Some(SellerOfferSection {
                    terms: state.terms.clone(),
                    create_hash: state.create_hash().unwrap().clone(),
                }),
                buyer_addresses: Some(BuyerAddressesSection {
                    mercury_address: MercuryAddressState {
                        mercury_transfer_address: "different-address".to_string(),
                    },
                    zerosats_address: state.zerosats_address().unwrap().clone(),
                }),
                ..SwapTranscript::default()
            },
        )
        .unwrap();

        let err = seller_template(SwapStateCommandArgs {
            state: state_path.clone(),
            transcript: SwapCommandArgs {
                swap: swap_path.clone(),
            },
        })
        .await
        .unwrap_err()
        .to_string();
        let unchanged = load_state(&state_path).unwrap();
        let transcript = load_transcript(&swap_path).unwrap();

        assert!(err.contains("buyer_addresses Mercury address does not match local state"));
        assert!(transcript.seller_template.is_none());
        assert_eq!(
            unchanged
                .mercury_address()
                .unwrap()
                .mercury_transfer_address,
            "buyer-mercury-address"
        );
    }

    #[tokio::test]
    async fn seller_release_requires_explicit_mercury_acknowledgement() {
        let dir = temp_dir("seller-release-requires-ack");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        save_state(&state_path, &state).unwrap();

        let err = seller_release(SellerReleaseArgs {
            swap: SwapStateCommandArgs {
                state: state_path,
                transcript: SwapCommandArgs {
                    swap: dir.join("swap.json"),
                },
            },
            release_mercury: false,
        })
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("--release-mercury"));
        assert!(err.contains("unlocks the Mercury transfer"));
    }

    #[test]
    fn seller_preimage_gate_requires_transfer_and_unlock_steps() {
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());

        let err = ensure_seller_can_request_preimage(&state)
            .unwrap_err()
            .to_string();

        assert!(err.contains("seller release step has not completed"));
    }

    #[test]
    fn seller_transfer_gate_rejects_verify_commitment_mismatch() {
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template,
            PathBuf::from("/tmp/template.json"),
        ));
        state.verify = Some(VerifyState {
            latch_verify: LatchVerifyState {
                commitment: "wrong-commitment".to_string(),
                height: 100,
                root_hash: "root".to_string(),
                txn_hash: "tx".to_string(),
            },
        });

        let err = ensure_verify_matches_template(&state, state.verify().unwrap())
            .unwrap_err()
            .to_string();

        assert!(err.contains("verify latch commitment mismatch"));
    }

    #[test]
    fn seller_claim_gate_rejects_preimage_that_does_not_match_offer_hash() {
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            PathBuf::from("/tmp/template.json"),
        ));
        add_completed_seller_unlock_state(&mut state, template.latch_commitment);
        let preimage = PreimageState {
            preimage: hex::encode([9u8; 32]),
            preimage_hash: state.create_hash().unwrap().payment_hash.clone(),
            hash_matches: true,
        };

        let err = ensure_seller_can_claim(&state, &preimage)
            .unwrap_err()
            .to_string();

        assert!(err.contains("preimage hash mismatch"));
    }

    #[test]
    fn command_output_redacts_local_paths_and_secrets() {
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.local = LocalConfig {
            mercury_settings_file: PathBuf::from("/private/local/Settings.toml"),
            mercury_wallet: "private-mercury-wallet".to_string(),
            zerosats_wallet: "private-zerosats-wallet".to_string(),
            zerosats_wallet_dir: Some(PathBuf::from("/private/local/zerosats-wallets")),
            output_prefix: PathBuf::from("/private/local/swap-output"),
        };
        state.template = Some(TemplateState {
            amount_wei: 1_000,
            claim_address: Element::new(1).to_string(),
            latch_commitment: hex::encode([2u8; 32]),
            template_note: PathBuf::from("/private/local/template.json"),
        });
        state.fund = Some(FundState {
            refund_note: Some(PathBuf::from("/private/local/refund.json")),
            lock_transaction: TransactionState {
                txn_hash: "lock-tx".to_string(),
                height: "100".to_string(),
                root_hash: "lock-root".to_string(),
            },
            balance_sat: Some(42),
        });
        state.preimage = Some(PreimageState {
            preimage: hex::encode([9u8; 32]),
            preimage_hash: hex::encode([42u8; 32]),
            hash_matches: true,
        });

        let json = output(Ok(state)).unwrap();

        assert_eq!(json["local"]["mercury_settings_file"], STDOUT_REDACTED);
        assert_eq!(json["local"]["mercury_wallet"], STDOUT_REDACTED);
        assert_eq!(json["local"]["zerosats_wallet"], STDOUT_REDACTED);
        assert_eq!(json["local"]["zerosats_wallet_dir"], STDOUT_REDACTED);
        assert_eq!(json["local"]["output_prefix"], STDOUT_REDACTED);
        assert_eq!(json["template"]["template_note"], STDOUT_REDACTED);
        assert_eq!(json["fund"]["refund_note"], STDOUT_REDACTED);
        assert_eq!(json["preimage"]["preimage"], STDOUT_REDACTED);
        assert_eq!(json["preimage"]["preimage_hash"], hex::encode([42u8; 32]));
        assert_eq!(json["fund"]["balance_sat"], 42);
    }

    #[test]
    fn transcript_serialization_omits_local_config_and_private_material() {
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.local = LocalConfig {
            mercury_settings_file: PathBuf::from("/private/local/Settings.toml"),
            mercury_wallet: "private-mercury-wallet".to_string(),
            zerosats_wallet: "private-zerosats-wallet".to_string(),
            zerosats_wallet_dir: Some(PathBuf::from("/private/local/zerosats-wallets")),
            output_prefix: PathBuf::from("/private/local/swap-output"),
        };
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.mercury_address = Some(MercuryAddressState {
            mercury_transfer_address: "mercury-transfer-address".to_string(),
        });
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, template_note) =
            valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            PathBuf::from("/private/local/template.json"),
        ));
        state.fund = Some(FundState {
            refund_note: Some(PathBuf::from("/private/local/refund.json")),
            lock_transaction: TransactionState {
                txn_hash: "lock-tx".to_string(),
                height: "100".to_string(),
                root_hash: "lock-root".to_string(),
            },
            balance_sat: Some(42),
        });
        state.transfer = Some(TransferState {
            retrieved_hash: state.create_hash().unwrap().payment_hash.clone(),
            hash_matches: true,
        });
        state.unlock = Some(UnlockState {
            latch_verify: LatchVerifyState {
                commitment: template.latch_commitment.clone(),
                height: 101,
                root_hash: "verify-root".to_string(),
                txn_hash: "verify-tx".to_string(),
            },
            unlocked: true,
        });
        state.receive = Some(ReceiveState {
            received_mercury_amount_sat: state.terms.mercury_amount_sat,
            received_expected_statechain_id: true,
            final_receive: MercuryReceiveState {
                is_there_batch_locked: false,
                received_statechain_ids: vec![state.terms.statechain_id.clone()],
            },
        });
        state.preimage = Some(PreimageState {
            preimage: hex::encode([9u8; 32]),
            preimage_hash: state.create_hash().unwrap().payment_hash.clone(),
            hash_matches: true,
        });

        let transcript = serde_json::to_string(&SwapTranscript {
            seller_offer: Some(SellerOfferSection {
                terms: state.terms.clone(),
                create_hash: state.create_hash().unwrap().clone(),
            }),
            buyer_addresses: Some(BuyerAddressesSection {
                mercury_address: state.mercury_address().unwrap().clone(),
                zerosats_address: state.zerosats_address().unwrap().clone(),
            }),
            seller_template: Some(SellerTemplateSection {
                zerosats_address: state.zerosats_address().unwrap().clone(),
                template: template.clone(),
                template_note,
            }),
            buyer_funding: Some(BuyerFundingSection {
                zerosats_address: state.zerosats_address().unwrap().clone(),
                template,
                fund: FundPublicState::from(state.fund().unwrap()),
            }),
            seller_unlock: Some(SellerUnlockSection {
                transfer: state.transfer().unwrap().clone(),
                unlock: state.unlock().unwrap().clone(),
            }),
            buyer_receive: Some(BuyerReceiveSection {
                receive: state.receive().unwrap().clone(),
            }),
            ..SwapTranscript::default()
        })
        .unwrap();

        let forbidden = [
            "/private/local".to_string(),
            "private-mercury-wallet".to_string(),
            "private-zerosats-wallet".to_string(),
            "mercury_settings_file".to_string(),
            "zerosats_wallet_dir".to_string(),
            "output_prefix".to_string(),
            "refund_note".to_string(),
            "balance_sat".to_string(),
            hex::encode([9u8; 32]),
        ];

        for private_value in &forbidden {
            assert!(
                !transcript.contains(private_value),
                "transcript leaked {private_value}: {transcript}"
            );
        }
    }

    #[tokio::test]
    async fn buyer_accept_rejects_invalid_offer_payment_hash() {
        let dir = temp_dir("buyer-accept-invalid-hash");
        let swap_path = dir.join("swap.json");
        let mut offer_state =
            swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        offer_state.create_hash = Some(CreateHashState {
            payment_hash: "not-hex".to_string(),
            batch_id: "batch-demo".to_string(),
        });
        save_transcript(
            &swap_path,
            &SwapTranscript {
                seller_offer: Some(SellerOfferSection {
                    terms: offer_state.terms.clone(),
                    create_hash: offer_state.create_hash().unwrap().clone(),
                }),
                ..SwapTranscript::default()
            },
        )
        .unwrap();

        let err = buyer_accept(BuyerAcceptArgs {
            state: dir.join("buyer-state.json"),
            transcript: SwapCommandArgs { swap: swap_path },
            local: local_wallet_args(&dir, dir.join("buyer/swap")),
        })
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("payment_hash"));
    }

    #[tokio::test]
    async fn buyer_accept_rejects_invalid_offer_terms() {
        let dir = temp_dir("buyer-accept-invalid-terms");
        let swap_path = dir.join("swap.json");
        let mut offer_state =
            swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        offer_state.terms.amount_sat = 0;
        offer_state.create_hash = Some(completed_hash());
        save_transcript(
            &swap_path,
            &SwapTranscript {
                seller_offer: Some(SellerOfferSection {
                    terms: offer_state.terms.clone(),
                    create_hash: offer_state.create_hash().unwrap().clone(),
                }),
                ..SwapTranscript::default()
            },
        )
        .unwrap();

        let err = buyer_accept(BuyerAcceptArgs {
            state: dir.join("buyer-state.json"),
            transcript: SwapCommandArgs { swap: swap_path },
            local: local_wallet_args(&dir, dir.join("buyer/swap")),
        })
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("amount must be greater than zero"));
    }

    #[test]
    fn seller_import_addresses_rejects_invalid_refund_address() {
        let dir = temp_dir("seller-import-addresses-invalid-refund");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        save_state(&state_path, &state).unwrap();

        let section = BuyerAddressesSection {
            mercury_address: MercuryAddressState {
                mercury_transfer_address: "buyer-mercury-address".to_string(),
            },
            zerosats_address: ZerosatsAddressState {
                refund_address: "not-an-element".to_string(),
            },
        };

        let err = seller_import_addresses_section(&state_path, section)
            .unwrap_err()
            .to_string();

        assert!(err.contains("invalid refund address"));
    }

    #[test]
    fn buyer_addresses_section_rejects_unknown_private_fields() {
        let mut section = serde_json::to_value(BuyerAddressesSection {
            mercury_address: MercuryAddressState {
                mercury_transfer_address: "buyer-mercury-address".to_string(),
            },
            zerosats_address: ZerosatsAddressState {
                refund_address: Element::new(12).to_string(),
            },
        })
        .unwrap();
        section["local"] = serde_json::json!({"wallet": "should-not-import"});
        let serde_err = serde_json::from_value::<BuyerAddressesSection>(section.clone())
            .unwrap_err()
            .to_string();
        assert!(serde_err.contains("unknown field `local`"));
    }

    #[test]
    fn buyer_import_template_writes_local_template_file() {
        let dir = temp_dir("buyer-import-template");
        let state_path = dir.join("buyer-state.json");
        let output_prefix = dir.join("buyer/swap");
        let mut state = swap_state(SwapRole::Buyer, "statechain-demo", 10_000, 10_000, false);
        state.local.output_prefix = output_prefix.clone();
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        state.template = None;
        save_state(&state_path, &state).unwrap();

        let (template, template_note) =
            valid_template_public(&state, Element::new(11), refund_key_hash);
        let section = SellerTemplateSection {
            zerosats_address: state.zerosats_address().unwrap().clone(),
            template,
            template_note: template_note.clone(),
        };

        let imported = buyer_import_template_section(&state_path, section).unwrap();

        let expected_template_path = template_note_path(&output_prefix);
        assert_eq!(
            imported.template().unwrap().template_note,
            expected_template_path
        );
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(expected_template_path).unwrap()).unwrap();
        assert_eq!(saved, template_note);
    }

    #[test]
    fn buyer_import_template_rejects_invalid_template_json_before_writing_file() {
        let dir = temp_dir("buyer-import-template-invalid-json");
        let state_path = dir.join("buyer-state.json");
        let output_prefix = dir.join("buyer/swap");
        let mut state = swap_state(SwapRole::Buyer, "statechain-demo", 10_000, 10_000, false);
        state.local.output_prefix = output_prefix.clone();
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        state.template = None;
        save_state(&state_path, &state).unwrap();

        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        let section = SellerTemplateSection {
            zerosats_address: state.zerosats_address().unwrap().clone(),
            template,
            template_note: serde_json::json!({"not": "a latch template"}),
        };

        let err = buyer_import_template_section(&state_path, section)
            .unwrap_err()
            .to_string();

        assert!(err.contains("failed to validate latch template section"));
        assert!(!template_note_path(&output_prefix).exists());
    }

    #[test]
    fn seller_export_template_rejects_corrupted_local_template_file() {
        let dir = temp_dir("seller-export-template-corrupted");
        let state_path = dir.join("seller-state.json");
        let template_path = dir.join("seller-template-note.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(template, template_path.clone()));
        fs::write(
            &template_path,
            serde_json::json!({"not": "a latch template"}).to_string(),
        )
        .unwrap();
        save_state(&state_path, &state).unwrap();

        let err = seller_template_section(&state_path)
            .unwrap_err()
            .to_string();

        assert!(err.contains("failed to validate latch template section"));
    }

    #[test]
    fn seller_import_funding_rejects_empty_transaction_summary() {
        let dir = temp_dir("seller-import-funding-empty-tx");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("seller-template.json"),
        ));
        save_state(&state_path, &state).unwrap();

        let section = BuyerFundingSection {
            zerosats_address: state.zerosats_address().unwrap().clone(),
            template,
            fund: FundPublicState {
                lock_transaction: TransactionState {
                    txn_hash: String::new(),
                    height: "100".to_string(),
                    root_hash: "root".to_string(),
                },
            },
        };

        let err = seller_import_funding_section(&state_path, section)
            .unwrap_err()
            .to_string();

        assert!(err.contains("transaction txn_hash is required"));
    }

    #[test]
    fn seller_import_funding_rejects_unknown_nested_private_fields() {
        let dir = temp_dir("seller-import-funding-unknown-private");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("seller-template.json"),
        ));

        let mut section = serde_json::to_value(BuyerFundingSection {
            zerosats_address: state.zerosats_address().unwrap().clone(),
            template,
            fund: FundPublicState {
                lock_transaction: TransactionState {
                    txn_hash: "tx".to_string(),
                    height: "100".to_string(),
                    root_hash: "root".to_string(),
                },
            },
        })
        .unwrap();
        section["fund"]["refund_note"] = serde_json::json!("/private/buyer/refund.json");
        let serde_err = serde_json::from_value::<BuyerFundingSection>(section.clone())
            .unwrap_err()
            .to_string();
        assert!(serde_err.contains("unknown field `refund_note`"));
    }

    #[test]
    fn buyer_import_unlock_rejects_wrong_latch_commitment() {
        let dir = temp_dir("buyer-import-unlock-wrong-commitment");
        let state_path = dir.join("buyer-state.json");
        let mut state = swap_state(SwapRole::Buyer, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        state.mercury_address = Some(MercuryAddressState {
            mercury_transfer_address: "buyer-mercury-address".to_string(),
        });
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("buyer-template.json"),
        ));
        state.receive = None;
        save_state(&state_path, &state).unwrap();

        let section = SellerUnlockSection {
            transfer: TransferState {
                retrieved_hash: state.create_hash().unwrap().payment_hash.clone(),
                hash_matches: true,
            },
            unlock: UnlockState {
                latch_verify: LatchVerifyState {
                    commitment: Element::new(13).to_string(),
                    height: 101,
                    root_hash: "root".to_string(),
                    txn_hash: "tx".to_string(),
                },
                unlocked: true,
            },
        };

        let err = buyer_import_unlock_section(&state_path, section)
            .unwrap_err()
            .to_string();

        assert!(err.contains("unlock latch commitment mismatch"));
    }

    #[test]
    fn buyer_import_unlock_recomputes_transfer_hash_flag() {
        let dir = temp_dir("buyer-import-unlock-recompute-hash");
        let state_path = dir.join("buyer-state.json");
        let mut state = swap_state(SwapRole::Buyer, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        state.mercury_address = Some(MercuryAddressState {
            mercury_transfer_address: "buyer-mercury-address".to_string(),
        });
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("buyer-template.json"),
        ));
        state.receive = None;
        save_state(&state_path, &state).unwrap();

        let mut section = SellerUnlockSection {
            transfer: TransferState {
                retrieved_hash: state.create_hash().unwrap().payment_hash.clone(),
                hash_matches: false,
            },
            unlock: UnlockState {
                latch_verify: LatchVerifyState {
                    commitment: template.latch_commitment,
                    height: 101,
                    root_hash: "root".to_string(),
                    txn_hash: "tx".to_string(),
                },
                unlocked: false,
            },
        };

        let err = buyer_import_unlock_section(&state_path, section.clone())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not unlocked"));

        section.unlock.unlocked = true;
        let imported = buyer_import_unlock_section(&state_path, section).unwrap();

        assert!(imported.transfer().unwrap().hash_matches);
        assert!(imported.unlock().unwrap().unlocked);
    }

    #[test]
    fn buyer_import_unlock_requires_local_mercury_address_step() {
        let dir = temp_dir("buyer-import-unlock-missing-address");
        let state_path = dir.join("buyer-state.json");
        let mut state = swap_state(SwapRole::Buyer, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        state.mercury_address = None;
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("buyer-template.json"),
        ));
        state.receive = None;
        save_state(&state_path, &state).unwrap();

        let section = SellerUnlockSection {
            transfer: TransferState {
                retrieved_hash: state.create_hash().unwrap().payment_hash.clone(),
                hash_matches: true,
            },
            unlock: UnlockState {
                latch_verify: LatchVerifyState {
                    commitment: template.latch_commitment,
                    height: 101,
                    root_hash: "root".to_string(),
                    txn_hash: "tx".to_string(),
                },
                unlocked: true,
            },
        };

        let err = buyer_import_unlock_section(&state_path, section)
            .unwrap_err()
            .to_string();

        assert!(err.contains("buyer accept step has not completed"));
    }

    fn add_completed_seller_unlock_state(state: &mut SwapState, commitment: String) {
        state.transfer = Some(TransferState {
            retrieved_hash: state.create_hash().unwrap().payment_hash.clone(),
            hash_matches: true,
        });
        state.unlock = Some(UnlockState {
            latch_verify: LatchVerifyState {
                commitment,
                height: 101,
                root_hash: "root".to_string(),
                txn_hash: "tx".to_string(),
            },
            unlocked: true,
        });
    }

    #[test]
    fn seller_import_receive_recomputes_received_statechain_flag() {
        let dir = temp_dir("seller-import-receive");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("seller-template.json"),
        ));
        add_completed_seller_unlock_state(&mut state, template.latch_commitment);
        state.receive = None;
        save_state(&state_path, &state).unwrap();

        let section = BuyerReceiveSection {
            receive: ReceiveState {
                received_mercury_amount_sat: 10_000,
                received_expected_statechain_id: false,
                final_receive: MercuryReceiveState {
                    is_there_batch_locked: false,
                    received_statechain_ids: vec!["statechain-demo".to_string()],
                },
            },
        };

        let imported = seller_import_receive_section(&state_path, section).unwrap();

        assert!(imported.receive().unwrap().received_expected_statechain_id);
    }

    #[test]
    fn seller_import_receive_rejects_wrong_amount() {
        let dir = temp_dir("seller-import-receive-wrong-amount");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("seller-template.json"),
        ));
        add_completed_seller_unlock_state(&mut state, template.latch_commitment);
        state.receive = None;
        save_state(&state_path, &state).unwrap();

        let section = BuyerReceiveSection {
            receive: ReceiveState {
                received_mercury_amount_sat: 9_999,
                received_expected_statechain_id: true,
                final_receive: MercuryReceiveState {
                    is_there_batch_locked: false,
                    received_statechain_ids: vec!["statechain-demo".to_string()],
                },
            },
        };

        let err = seller_import_receive_section(&state_path, section)
            .unwrap_err()
            .to_string();

        assert!(err.contains("Mercury amount mismatch"));
    }

    #[test]
    fn seller_import_receive_rejects_incomplete_local_unlock() {
        let dir = temp_dir("seller-import-receive-incomplete-unlock");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        let refund_key_hash = Element::new(12);
        state.zerosats_address = Some(ZerosatsAddressState {
            refund_address: refund_key_hash.to_string(),
        });
        let (template, _) = valid_template_public(&state, Element::new(11), refund_key_hash);
        state.template = Some(template_from_public(
            template.clone(),
            dir.join("seller-template.json"),
        ));
        add_completed_seller_unlock_state(&mut state, template.latch_commitment);
        state.unlock.as_mut().unwrap().unlocked = false;
        state.receive = None;
        save_state(&state_path, &state).unwrap();

        let section = BuyerReceiveSection {
            receive: ReceiveState {
                received_mercury_amount_sat: 10_000,
                received_expected_statechain_id: true,
                final_receive: MercuryReceiveState {
                    is_there_batch_locked: false,
                    received_statechain_ids: vec!["statechain-demo".to_string()],
                },
            },
        };

        let err = seller_import_receive_section(&state_path, section)
            .unwrap_err()
            .to_string();

        assert!(err.contains("Mercury transfer is not unlocked"));
    }

    #[test]
    fn seller_import_receive_requires_local_unlock_step() {
        let dir = temp_dir("seller-import-receive-missing-unlock");
        let state_path = dir.join("seller-state.json");
        let mut state = swap_state(SwapRole::Seller, "statechain-demo", 10_000, 10_000, false);
        state.create_hash = Some(completed_hash());
        state.receive = None;
        save_state(&state_path, &state).unwrap();

        let section = BuyerReceiveSection {
            receive: ReceiveState {
                received_mercury_amount_sat: 10_000,
                received_expected_statechain_id: true,
                final_receive: MercuryReceiveState {
                    is_there_batch_locked: false,
                    received_statechain_ids: vec!["statechain-demo".to_string()],
                },
            },
        };

        let err = seller_import_receive_section(&state_path, section)
            .unwrap_err()
            .to_string();

        assert!(err.contains("seller release step has not completed"));
    }

    #[test]
    fn role_check_rejects_wrong_party_state() {
        let state = swap_state(SwapRole::Buyer, "statechain-demo", 10_000, 10_000, false);
        let err = state.ensure_role(SwapRole::Seller).unwrap_err().to_string();

        assert!(err.contains("this is a buyer state"));
    }

    #[test]
    fn template_section_does_not_include_local_path() {
        let template = TemplateState {
            amount_wei: 10,
            claim_address: "claim".to_string(),
            latch_commitment: "commitment".to_string(),
            template_note: PathBuf::from("/seller/local-template.json"),
        };

        let section = TemplatePublicState::from(&template);
        let json = serde_json::to_string(&section).unwrap();

        assert!(!json.contains("local-template"));
    }

    #[test]
    fn fund_section_does_not_include_refund_note() {
        let fund = FundState {
            refund_note: Some(PathBuf::from("/buyer/refund.json")),
            lock_transaction: TransactionState {
                txn_hash: "tx".to_string(),
                height: "1".to_string(),
                root_hash: "root".to_string(),
            },
            balance_sat: Some(42),
        };

        let section = FundPublicState::from(&fund);
        let json = serde_json::to_string(&section).unwrap();

        assert!(!json.contains("refund"));
        assert!(!json.contains("balance"));
    }
}
