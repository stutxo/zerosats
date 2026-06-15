use std::path::{Path, PathBuf};

use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use sha2::{Digest, Sha256};

use crate::{
    cli::*,
    latch,
    mercury::{WrapDisplay, load_mercury},
    state::*,
};

pub(crate) async fn execute(command: Command) -> Result<SwapState> {
    match command {
        Command::InitState(args) => init(args),
        Command::MercuryCreateHash(args) => create_hash(args).await,
        Command::MercuryAddress(args) => mercury_address(args).await,
        Command::ZerosatsAddress(args) => zerosats_address(args).await,
        Command::ZerosatsTemplate(args) => template(args).await,
        Command::ZerosatsFund(args) => fund(args).await,
        Command::ZerosatsVerify(args) => verify(args).await,
        Command::MercuryTransfer(args) => transfer(args).await,
        Command::MercuryUnlock(args) => unlock(args).await,
        Command::MercuryReceive(args) => receive(args).await,
        Command::MercuryPreimage(args) => preimage(args).await,
        Command::ZerosatsClaim(args) => claim(args).await,
    }
}

fn init(args: InitArgs) -> Result<SwapState> {
    ensure_non_empty("--statechain-id", &args.statechain_id)?;
    ensure_non_empty("--owner-mercury-wallet", &args.owner_mercury_wallet)?;
    ensure_non_empty("--receiver-mercury-wallet", &args.receiver_mercury_wallet)?;
    ensure_non_empty("--funder-zerosats-wallet", &args.funder_zerosats_wallet)?;
    ensure_non_empty("--claimer-zerosats-wallet", &args.claimer_zerosats_wallet)?;
    ensure_amount(args.amount_sat)?;
    ensure_amount(args.mercury_amount_sat)?;
    ensure_refund_blocks(args.latch.refund_blocks)?;
    ensure_output_prefix(&args.output_prefix)?;

    let mercury_settings_file = args
        .mercury_settings_file
        .canonicalize()
        .wrap_err_with(|| format!("failed to resolve {}", args.mercury_settings_file.display()))?;

    let state = SwapState::new(SwapConfig {
        mercury_settings_file,
        statechain_id: args.statechain_id,
        amount_sat: args.amount_sat,
        mercury_amount_sat: args.mercury_amount_sat,
        owner_mercury_wallet: args.owner_mercury_wallet,
        receiver_mercury_wallet: args.receiver_mercury_wallet,
        funder_zerosats_wallet: args.funder_zerosats_wallet,
        claimer_zerosats_wallet: args.claimer_zerosats_wallet,
        zerosats_wallet_dir: args.zerosats_wallet_dir,
        zerosats_host: args.node.zerosats_host,
        citrea_chain: args.latch.citrea_chain,
        refund_blocks: args.latch.refund_blocks,
        ticker: args.latch.ticker.to_uppercase(),
        btc_explorer: args.latch.btc_explorer,
        output_prefix: args.output_prefix,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn create_hash(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    let mercury = load_mercury(&config.mercury_settings_file).await?;

    let payment = mercuryrustlib::lightning_latch::create_pre_image(
        &mercury,
        &config.owner_mercury_wallet,
        &config.statechain_id,
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

async fn mercury_address(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    let mercury = load_mercury(&config.mercury_settings_file).await?;

    let mercury_transfer_address = mercuryrustlib::transfer_receiver::new_transfer_address(
        &mercury,
        &config.receiver_mercury_wallet,
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

async fn zerosats_address(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    let refund_address = latch::latch_address(
        config.zerosats_wallet_dir.as_deref(),
        &config.funder_zerosats_wallet,
    )
    .wrap_err("failed to load Zerosats wallet")?;

    state.zerosats_address = Some(ZerosatsAddressState {
        refund_address: refund_address.to_string(),
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn template(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    let create_hash = state.create_hash()?.clone();
    let zerosats_address = state.zerosats_address()?.clone();
    let amount_wei = latch::sats_to_wei(config.amount_sat);
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    let claim_address = latch::latch_address(
        config.zerosats_wallet_dir.as_deref(),
        &config.claimer_zerosats_wallet,
    )
    .wrap_err("failed to load Zerosats wallet")?;
    let refund_address =
        latch::parse_element_arg(&zerosats_address.refund_address, "refund address")?;
    let template_path = template_note_path(&config.output_prefix);

    let commitment = latch::create_latch_template(latch::LatchTemplateRequest {
        chain: config.citrea_chain,
        amount_wei,
        ticker: &config.ticker,
        payment_hash_hex: &payment_hash,
        claim_address,
        refund_address,
        output_path: &template_path,
        refund_blocks: config.refund_blocks,
        btc_explorer: &config.btc_explorer,
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

async fn fund(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    let create_hash = state.create_hash()?.clone();
    let template = state.template()?.clone();
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    verify_mercury_payment_hash(
        &config.mercury_settings_file,
        &create_hash.batch_id,
        &payment_hash,
    )
    .await?;
    let claim_address = latch::parse_element_arg(&template.claim_address, "claim address")?;

    let fund = latch::fund_latch(latch::LatchFundRequest {
        chain: config.citrea_chain,
        wallet_dir: config.zerosats_wallet_dir.as_deref(),
        wallet_name: &config.funder_zerosats_wallet,
        host: &config.zerosats_host,
        btc_explorer: &config.btc_explorer,
        template_path: &template.template_note,
        amount_wei: latch::sats_to_wei(config.amount_sat),
        ticker: &config.ticker,
        payment_hash_hex: &payment_hash,
        claim_address,
        refund_blocks: config.refund_blocks,
        output_prefix: &config.output_prefix,
    })
    .await
    .wrap_err("failed to fund Zerosats latch")?;

    state.fund = Some(FundState {
        refund_note: fund.refund_path,
        lock_transaction: summarize_tx(&fund.transaction),
        balance_sat: latch::wei_to_sats(fund.balance_wei),
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn verify(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let latch_verify = verify_latch_from_state(&state).await?;

    state.verify = Some(VerifyState { latch_verify });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn transfer(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    let create_hash = state.create_hash()?.clone();
    let mercury_address = state.mercury_address()?.clone();
    state.verify()?;
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    let mercury = load_mercury(&config.mercury_settings_file).await?;

    mercuryrustlib::transfer_sender::execute(
        &mercury,
        &mercury_address.mercury_transfer_address,
        &config.owner_mercury_wallet,
        &config.statechain_id,
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

async fn unlock(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    state.transfer()?;
    let latch_verify = verify_latch_from_state(&state)
        .await
        .wrap_err("fresh latch verification failed before Mercury unlock")?;

    let mercury = load_mercury(&config.mercury_settings_file).await?;

    mercuryrustlib::lightning_latch::confirm_pending_invoice(
        &mercury,
        &config.owner_mercury_wallet,
        &config.statechain_id,
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

async fn receive(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    state.unlock()?;
    let mercury = load_mercury(&config.mercury_settings_file).await?;

    let final_receive =
        mercuryrustlib::transfer_receiver::execute(&mercury, &config.receiver_mercury_wallet)
            .await
            .wrap_display("failed to complete Mercury receive")?;
    let final_receive = summarize_receive(final_receive);
    ensure_received_statechain(&final_receive, &config.statechain_id)?;
    let received_mercury_amount_sat = received_mercury_amount_sat(
        &mercury,
        &config.receiver_mercury_wallet,
        &config.statechain_id,
    )
    .await?;
    ensure_mercury_amount(config.mercury_amount_sat, received_mercury_amount_sat)?;

    mercury.pool.close().await;

    state.receive = Some(ReceiveState {
        received_mercury_amount_sat,
        received_expected_statechain_id: true,
        final_receive,
    });
    save_state(&args.state, &state)?;
    Ok(state)
}

async fn preimage(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    let create_hash = state.create_hash()?.clone();
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    ensure_state_allows_preimage(&state)?;
    let mercury = load_mercury(&config.mercury_settings_file).await?;

    let preimage = mercuryrustlib::lightning_latch::retrieve_pre_image(
        &mercury,
        &config.owner_mercury_wallet,
        &config.statechain_id,
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

async fn claim(args: StateCommandArgs) -> Result<SwapState> {
    let mut state = load_state(&args.state)?;
    let config = state.config.clone();
    let template = state.template()?.clone();
    let preimage = state.preimage()?.clone();

    let claim = latch::claim_latch(latch::LatchClaimRequest {
        chain: config.citrea_chain,
        wallet_dir: config.zerosats_wallet_dir.as_deref(),
        wallet_name: &config.claimer_zerosats_wallet,
        host: &config.zerosats_host,
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
    let config = state.config.clone();
    let create_hash = state.create_hash()?.clone();
    let zerosats_address = state.zerosats_address()?.clone();
    let template = state.template()?.clone();
    let payment_hash = validate_payment_hash(&create_hash.payment_hash)?;
    let refund_address =
        latch::parse_element_arg(&zerosats_address.refund_address, "refund address")?;

    let verified = latch::verify_latch(latch::LatchVerifyRequest {
        chain: config.citrea_chain,
        wallet_dir: config.zerosats_wallet_dir.as_deref(),
        wallet_name: &config.claimer_zerosats_wallet,
        host: &config.zerosats_host,
        btc_explorer: &config.btc_explorer,
        note_path: &template.template_note,
        amount_wei: latch::sats_to_wei(config.amount_sat),
        ticker: &config.ticker,
        payment_hash_hex: &payment_hash,
        refund_address,
        refund_blocks: config.refund_blocks,
    })
    .await
    .wrap_err("failed to verify funded Zerosats latch")?;

    Ok(summarize_latch_verify(verified))
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
    use super::*;

    fn swap_state(
        statechain_id: &str,
        expected_mercury_amount_sat: u64,
        received_mercury_amount_sat: u64,
        is_there_batch_locked: bool,
    ) -> SwapState {
        let mut state = SwapState::new(SwapConfig {
            mercury_settings_file: PathBuf::from("/tmp/mercury-settings.toml"),
            statechain_id: statechain_id.to_string(),
            amount_sat: 1_000,
            mercury_amount_sat: expected_mercury_amount_sat,
            owner_mercury_wallet: "seller".to_string(),
            receiver_mercury_wallet: "buyer".to_string(),
            funder_zerosats_wallet: "buyer".to_string(),
            claimer_zerosats_wallet: "seller".to_string(),
            zerosats_wallet_dir: Some(PathBuf::from("/tmp/wallets")),
            zerosats_host: "http://localhost:3000".to_string(),
            citrea_chain: 5115,
            refund_blocks: 2,
            ticker: "WCBTC".to_string(),
            btc_explorer: "http://localhost:8080".to_string(),
            output_prefix: PathBuf::from("/tmp/swap"),
        });
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

    #[test]
    fn state_allows_preimage_after_successful_receive() {
        let state = swap_state("statechain-demo", 10_000, 10_000, false);

        ensure_state_allows_preimage(&state).unwrap();
    }

    #[test]
    fn state_rejects_batch_locked_receive() {
        let state = swap_state("statechain-demo", 10_000, 0, true);
        let err = ensure_state_allows_preimage(&state)
            .unwrap_err()
            .to_string();

        assert!(err.contains("did not confirm the expected statechain_id"));
    }

    #[test]
    fn state_rejects_wrong_statechain_for_preimage_release() {
        let mut state = swap_state("statechain-demo", 10_000, 10_000, false);
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
        let state = swap_state("statechain-demo", 10_000, 9_999, false);

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
}
