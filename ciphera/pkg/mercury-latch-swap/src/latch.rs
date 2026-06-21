use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use barretenberg::{Prove, Verify};
use cli::{
    NodeClient, Wallet,
    address::{citrea_ticker_from_contract, citrea_token_data, network_for_chain},
    units,
};
use color_eyre::{
    Result,
    eyre::{WrapErr, eyre},
};
use element::Element;
use hash::hash_merge;
use node_interface::{ElementsResponseSingle, TransactionResponse};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zk_primitives::{EscrowInputNote, Note, TimeLock, TimeProof, get_address_for_private_key};

const REFUND_PROOF_HEADERS: u64 = 2;

#[derive(Debug, Clone)]
pub struct LatchTemplateRequest<'a> {
    pub chain: u64,
    pub amount_wei: u64,
    pub ticker: &'a str,
    pub payment_hash_hex: &'a str,
    pub claim_address: Element,
    pub refund_address: Element,
    pub output_path: &'a Path,
    pub refund_blocks: u64,
    pub btc_explorer: &'a str,
}

#[derive(Debug, Clone)]
pub struct LatchFundRequest<'a> {
    pub chain: u64,
    pub wallet_dir: Option<&'a Path>,
    pub wallet_name: &'a str,
    pub host: &'a str,
    pub btc_explorer: &'a str,
    pub template_path: &'a Path,
    pub amount_wei: u64,
    pub ticker: &'a str,
    pub payment_hash_hex: &'a str,
    pub claim_address: Element,
    pub refund_blocks: u64,
    pub output_prefix: &'a Path,
}

#[derive(Debug, Clone)]
pub struct LatchVerifyRequest<'a> {
    pub chain: u64,
    pub wallet_dir: Option<&'a Path>,
    pub wallet_name: &'a str,
    pub host: &'a str,
    pub btc_explorer: &'a str,
    pub note_path: &'a Path,
    pub amount_wei: u64,
    pub ticker: &'a str,
    pub payment_hash_hex: &'a str,
    pub refund_address: Element,
    pub refund_blocks: u64,
}

#[derive(Debug, Clone)]
pub struct LatchFundResult {
    pub commitment: Element,
    pub refund_address: Element,
    pub refund_path: PathBuf,
    pub transaction: TransactionResponse,
    pub balance_wei: u64,
}

#[derive(Debug, Clone)]
pub struct LatchClaimRequest<'a> {
    pub chain: u64,
    pub wallet_dir: Option<&'a Path>,
    pub wallet_name: &'a str,
    pub host: &'a str,
    pub note_path: &'a Path,
    pub preimage_hex: &'a str,
}

#[derive(Debug, Clone)]
pub struct LatchClaimResult {
    pub transaction: TransactionResponse,
    pub balance_wei: u64,
    pub ticker: String,
}

#[derive(Debug, Deserialize)]
struct MempoolBlockInfo {
    height: u64,
}

pub fn parse_bytes32_hex(s: &str, field: &str) -> Result<[u8; 32]> {
    let trimmed = s.trim().trim_start_matches("0x");
    let bytes = hex::decode(trimmed).map_err(|e| eyre!("{field} must be hex: {e}"))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| eyre!("{field} must be 32 bytes"))
}

pub fn parse_preimage_hex(s: &str) -> Result<[u8; 32]> {
    parse_bytes32_hex(s, "preimage")
}

pub fn parse_payment_hash_hex(s: &str) -> Result<[u8; 32]> {
    parse_bytes32_hex(s, "payment_hash")
}

pub fn parse_element_arg(s: &str, field: &str) -> Result<Element> {
    Element::from_str(s).map_err(|e| eyre!("invalid {field}: {e}"))
}

pub fn latch_address(wallet_dir: Option<&Path>, wallet_name: &str) -> Result<Element> {
    let wallet = load_wallet(wallet_dir, wallet_name)?;
    Ok(get_address_for_private_key(wallet.pk))
}

fn htlc_claim_address_from_payment_hash(
    redeemer_secret_key: Element,
    payment_hash: [u8; 32],
) -> Element {
    let key_hash = get_address_for_private_key(redeemer_secret_key);
    htlc_claim_address_for_key_hash(key_hash, payment_hash)
}

fn htlc_claim_address_for_key_hash(redeemer_key_hash: Element, payment_hash: [u8; 32]) -> Element {
    let elem = Element::from_be_bytes(payment_hash);
    let (high, low) = elem.decompose_be();
    hash_merge([redeemer_key_hash, high, low])
}

fn htlc_refund_psi_for_key_hash(locker_key_hash: Element, lock: &TimeLock) -> Element {
    hash_merge([locker_key_hash, lock.commitment()])
}

pub fn load_latch_template(path: &Path) -> Result<EscrowInputNote> {
    let json_str = fs::read_to_string(path).map_err(|e| eyre!("{}: {e}", path.display()))?;
    serde_json::from_str(&json_str).wrap_err_with(|| format!("{}", path.display()))
}

pub fn validate_latch_template(
    template: &EscrowInputNote,
    chain: u64,
    amount_wei: u64,
    ticker: &str,
    payment_hash: [u8; 32],
    claim_address: Element,
    refund_address: Element,
    refund_blocks: u64,
) -> Result<Element> {
    let (expected_utxo_kind, expected_note_kind) =
        citrea_token_data(network_for_chain(chain), ticker);

    verify_latch_note_shape(
        template,
        claim_address,
        payment_hash,
        amount_wei,
        expected_utxo_kind,
        expected_note_kind,
        refund_address,
        refund_blocks,
    )
    .map_err(|e| eyre!("latch template verification failed: {e}"))
}

pub fn validate_latch_template_json(
    template_json: &serde_json::Value,
    chain: u64,
    amount_wei: u64,
    ticker: &str,
    payment_hash: [u8; 32],
    claim_address: Element,
    refund_address: Element,
    refund_blocks: u64,
) -> Result<Element> {
    let template: EscrowInputNote = serde_json::from_value(template_json.clone())
        .wrap_err("failed to parse latch template section JSON")?;
    validate_latch_template(
        &template,
        chain,
        amount_wei,
        ticker,
        payment_hash,
        claim_address,
        refund_address,
        refund_blocks,
    )
}

fn verify_latch_note_shape(
    input_note: &EscrowInputNote,
    claim_key_hash: Element,
    payment_hash: [u8; 32],
    expected_value_wei: u64,
    expected_utxo_kind: Element,
    expected_note_kind: Element,
    refund_key_hash: Element,
    expected_refund_blocks: u64,
) -> std::result::Result<Element, String> {
    if expected_refund_blocks == 0 || expected_refund_blocks > 2 {
        return Err("expected refund blocks must be between 1 and 2".to_owned());
    }

    if input_note.spend_type != 3 {
        return Err(format!(
            "expected HTLC spend_type 3, got {}",
            input_note.spend_type
        ));
    }

    if input_note.preimage != [0u8; 32] {
        return Err("latch note template must not contain a preimage".to_owned());
    }

    if input_note.note.utxo_kind != expected_utxo_kind {
        return Err(format!(
            "unexpected utxo kind: got {}, expected {}",
            input_note.note.utxo_kind, expected_utxo_kind
        ));
    }

    if input_note.note.note_kind != expected_note_kind {
        return Err(format!(
            "unexpected note kind: got {}, expected {}",
            input_note.note.note_kind, expected_note_kind
        ));
    }

    let expected_value = Element::from(expected_value_wei);
    if input_note.note.value != expected_value {
        return Err(format!(
            "unexpected note value: got {}, expected {}",
            input_note.note.value, expected_value
        ));
    }

    let expected_address = htlc_claim_address_for_key_hash(claim_key_hash, payment_hash);
    if input_note.note.address != expected_address {
        return Err(format!(
            "claim path mismatch: note address {} does not match expected {}",
            input_note.note.address, expected_address
        ));
    }

    if input_note.note.psi == Element::ZERO {
        return Err("refund path is not committed: note psi is zero".to_owned());
    }

    if input_note.time_proof.lock.zero_block == [0u8; 32] {
        return Err("refund timelock has no Bitcoin anchor".to_owned());
    }

    if input_note.time_proof.lock.n_blocks == Element::ZERO {
        return Err("refund timelock requires at least one block".to_owned());
    }

    let expected_refund_blocks = Element::from(expected_refund_blocks);
    if input_note.time_proof.lock.n_blocks != expected_refund_blocks {
        return Err(format!(
            "unexpected refund timelock: got {} blocks, expected {}",
            input_note.time_proof.lock.n_blocks, expected_refund_blocks
        ));
    }

    let expected_psi = htlc_refund_psi_for_key_hash(refund_key_hash, &input_note.time_proof.lock);
    if input_note.note.psi != expected_psi {
        return Err(format!(
            "refund path mismatch: note psi {} does not match expected {}",
            input_note.note.psi, expected_psi
        ));
    }

    if input_note.time_proof.headers != [[0u8; 80]; 2] {
        return Err("latch note template must not contain refund proof headers".to_owned());
    }

    Ok(input_note.note.commitment())
}

pub async fn create_latch_template(req: LatchTemplateRequest<'_>) -> Result<Element> {
    ensure_positive_refund_blocks(req.refund_blocks)?;

    let payment_hash = parse_payment_hash_hex(req.payment_hash_hex)?;
    let lock = bitcoin_clock::BitcoinClock::new(req.btc_explorer)
        .tip_lock(req.refund_blocks)
        .await?;
    let note = latch_note_from_parts(
        req.chain,
        req.amount_wei,
        req.ticker,
        payment_hash,
        req.claim_address,
        req.refund_address,
        &lock,
    );
    let input_note = latch_input_note(note.clone(), Element::ZERO, &lock);

    write_json_file(req.output_path, &input_note)?;

    Ok(note.commitment())
}

pub async fn fund_latch(req: LatchFundRequest<'_>) -> Result<LatchFundResult> {
    let mut client = node_client(req.chain, req.wallet_dir, req.wallet_name, req.host)?;

    let template = load_latch_template(req.template_path)?;
    let payment_hash = parse_payment_hash_hex(req.payment_hash_hex)?;
    let refund_secret_key = client.get_wallet().pk;
    let refund_address = get_address_for_private_key(refund_secret_key);
    let commitment = validate_latch_template(
        &template,
        req.chain,
        req.amount_wei,
        req.ticker,
        payment_hash,
        req.claim_address,
        refund_address,
        req.refund_blocks,
    )?;

    ensure_refund_path_not_mature(req.btc_explorer, &template.time_proof.lock)
        .await
        .wrap_err("failed to verify latch refund timelock freshness")?;

    let (prepared_wallet, utxo) = client
        .get_wallet()
        .prepare_escrow_lock_to_note(template.note.clone())?;

    let refund_path = write_latch_refund(req.output_prefix, &template, refund_secret_key)?;

    let snark = utxo
        .prove()
        .map_err(|e| eyre!("utxo.prove() failed: {e}"))?;

    let transaction = client.transaction(&snark).await?;

    prepared_wallet.save()?;
    client.replace_wallet(prepared_wallet);

    Ok(LatchFundResult {
        commitment,
        refund_address,
        refund_path,
        transaction,
        balance_wei: client.get_wallet().balance,
    })
}

pub async fn verify_latch(req: LatchVerifyRequest<'_>) -> Result<ElementsResponseSingle> {
    let template = load_latch_template(req.note_path)?;
    let commitment = template.note.commitment();
    let payment_hash = parse_payment_hash_hex(req.payment_hash_hex)?;
    let claim_address = latch_address(req.wallet_dir, req.wallet_name)?;

    validate_latch_template(
        &template,
        req.chain,
        req.amount_wei,
        req.ticker,
        payment_hash,
        claim_address,
        req.refund_address,
        req.refund_blocks,
    )?;
    ensure_refund_path_not_mature(req.btc_explorer, &template.time_proof.lock)
        .await
        .wrap_err("failed to verify latch refund timelock freshness")?;

    let client = node_client(req.chain, req.wallet_dir, req.wallet_name, req.host)?;
    let element = get_node_element(&client, commitment)
        .await?
        .ok_or_else(|| {
            eyre!(
                "latch note commitment {} is not present as an unspent note on the node",
                commitment
            )
        })?;

    Ok(element)
}

pub async fn claim_latch(req: LatchClaimRequest<'_>) -> Result<LatchClaimResult> {
    let mut client = node_client(req.chain, req.wallet_dir, req.wallet_name, req.host)?;

    let mut htlc_input_note = load_latch_template(req.note_path)?;
    htlc_input_note.preimage = parse_preimage_hex(req.preimage_hex)?;
    htlc_input_note.secret_key = client.get_wallet().pk;

    if htlc_input_note.preimage == [0u8; 32] {
        return Err(eyre!("latch-claim requires a non-zero preimage"));
    }

    let expected_address = htlc_claim_address_from_payment_hash(
        htlc_input_note.secret_key,
        Sha256::digest(htlc_input_note.preimage).into(),
    );
    if expected_address != htlc_input_note.note.address {
        return Err(eyre!("preimage/secret key do not unlock this latch note"));
    }

    let ticker = citrea_ticker_from_contract(htlc_input_note.note.note_kind);
    let (prepared_wallet, escrow, _received) = client
        .get_wallet()
        .prepare_escrow_redeem(&htlc_input_note)?;

    let snark = escrow
        .prove()
        .map_err(|e| eyre!("escrow.prove() failed: {e}"))?;
    snark
        .verify()
        .map_err(|e| eyre!("escrow.verify() failed: {e}"))?;

    let transaction = client.transaction_escrow(&snark).await?;

    prepared_wallet.save()?;
    client.replace_wallet(prepared_wallet);

    Ok(LatchClaimResult {
        transaction,
        balance_wei: client.get_wallet().balance,
        ticker,
    })
}

pub fn sats_to_wei(amount_sat: u64) -> u64 {
    units::sats_to_wei(amount_sat)
}

pub fn wei_to_sats(amount_wei: u64) -> u64 {
    units::wei_to_sats(amount_wei)
}

fn ensure_positive_refund_blocks(refund_blocks: u64) -> Result<()> {
    if refund_blocks == 0 {
        return Err(eyre!("--refund-blocks must be greater than zero"));
    }
    if refund_blocks > 2 {
        return Err(eyre!(
            "--refund-blocks must be 2 or less with the current escrow circuit"
        ));
    }
    Ok(())
}

fn latch_note_from_parts(
    chain: u64,
    amount_wei: u64,
    ticker: &str,
    payment_hash: [u8; 32],
    claim_key_hash: Element,
    refund_key_hash: Element,
    lock: &TimeLock,
) -> Note {
    let (utxo_kind, note_kind) = citrea_token_data(network_for_chain(chain), ticker);
    Note {
        utxo_kind,
        note_kind,
        address: htlc_claim_address_for_key_hash(claim_key_hash, payment_hash),
        psi: htlc_refund_psi_for_key_hash(refund_key_hash, lock),
        value: Element::from(amount_wei),
    }
}

fn latch_input_note(note: Note, secret_key: Element, lock: &TimeLock) -> EscrowInputNote {
    EscrowInputNote {
        note,
        spend_type: 3,
        secret_key,
        preimage: [0u8; 32],
        time_proof: TimeProof {
            lock: lock.clone(),
            ..Default::default()
        },
    }
}

async fn ensure_refund_path_not_mature(btc_explorer: &str, lock: &TimeLock) -> Result<()> {
    let required_blocks = required_refund_blocks(lock)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let base_url = btc_explorer.trim_end_matches('/');
    let anchor_height = mempool_anchor_height(&http, base_url, lock).await?;
    let tip_height = mempool_tip_height(&http, base_url).await?;

    if refund_is_mature_at_height(anchor_height, tip_height, required_blocks) {
        return Err(eyre!(
            "latch refund path is already mature at the current Bitcoin tip; refusing to fund a \
             note that can be refunded immediately"
        ));
    }
    Ok(())
}

fn required_refund_blocks(lock: &TimeLock) -> Result<u64> {
    let required_blocks =
        u64::try_from(lock.n_blocks).map_err(|_| eyre!("timelock n_blocks exceeds u64"))?;
    if required_blocks == 0 {
        return Err(eyre!("timelock n_blocks must be greater than zero"));
    }
    if required_blocks > REFUND_PROOF_HEADERS {
        return Err(eyre!(
            "timelock n_blocks exceeds supported {REFUND_PROOF_HEADERS}-header refund proof"
        ));
    }
    Ok(required_blocks)
}

async fn mempool_anchor_height(
    http: &reqwest::Client,
    base_url: &str,
    lock: &TimeLock,
) -> Result<u64> {
    let anchor_display = mempool_display_hash(lock.zero_block);
    let info = http
        .get(format!("{base_url}/api/block/{anchor_display}"))
        .send()
        .await?
        .error_for_status()
        .map_err(|e| eyre!("anchor block {anchor_display} not found: {e}"))?
        .json::<MempoolBlockInfo>()
        .await?;
    Ok(info.height)
}

async fn mempool_tip_height(http: &reqwest::Client, base_url: &str) -> Result<u64> {
    let text = http
        .get(format!("{base_url}/api/blocks/tip/height"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    text.trim()
        .parse()
        .map_err(|e| eyre!("failed to parse Bitcoin tip height: {e}"))
}

fn mempool_display_hash(internal: [u8; 32]) -> String {
    let mut bytes = internal;
    bytes.reverse();
    hex::encode(bytes)
}

fn refund_is_mature_at_height(anchor_height: u64, tip_height: u64, required_blocks: u64) -> bool {
    tip_height.saturating_sub(anchor_height) >= required_blocks
}

fn write_latch_refund(
    prefix: &Path,
    template: &EscrowInputNote,
    refund_secret_key: Element,
) -> Result<PathBuf> {
    let refund_path = path_with_suffix(prefix, "-refund.json");

    let mut refund_input_note = template.clone();
    refund_input_note.secret_key = refund_secret_key;
    refund_input_note.preimage = [0u8; 32];
    write_json_file(&refund_path, &refund_input_note)?;
    set_private_permissions(&refund_path)?;

    Ok(refund_path)
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .wrap_err_with(|| format!("failed to create {}", parent.display()))?;
        }
    }

    fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))
        .wrap_err_with(|| format!("failed to write {}", path.display()))
}

pub fn path_with_suffix(prefix: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", prefix.display(), suffix))
}

fn load_wallet(wallet_dir: Option<&Path>, wallet_name: &str) -> Result<Wallet> {
    match wallet_dir {
        Some(dir) => Ok(Wallet::load_from(dir, wallet_name)?),
        None => Ok(Wallet::load(wallet_name)?),
    }
}

fn node_client(
    chain: u64,
    wallet_dir: Option<&Path>,
    wallet_name: &str,
    host: &str,
) -> Result<NodeClient> {
    let mut builder = NodeClient::builder().name(wallet_name).host(host);
    if let Some(wallet_dir) = wallet_dir {
        builder = builder.wallet_dir(wallet_dir);
    }
    builder.build(chain, false)
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

async fn get_node_element(
    client: &NodeClient,
    element: Element,
) -> Result<Option<ElementsResponseSingle>> {
    let url = format!("{}/elements/{}", client.base_url(), element.to_hex());
    let response = reqwest::Client::new()
        .get(&url)
        .timeout(client.timeout())
        .send()
        .await
        .map_err(|e| eyre!("Failed to connect to node: {e}"))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }

    if !response.status().is_success() {
        return Err(eyre!("Node returned error status: {}", response.status()));
    }

    let element = response
        .json::<ElementsResponseSingle>()
        .await
        .map_err(|e| eyre!("Failed to parse element response: {e}"))?;
    Ok(Some(element))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_with_suffix_keeps_parent_and_appends_suffix() {
        let path = path_with_suffix(Path::new("/tmp/swap"), "-refund.json");
        assert_eq!(path, PathBuf::from("/tmp/swap-refund.json"));
    }

    #[test]
    fn parse_payment_hash_requires_32_bytes() {
        let err = parse_payment_hash_hex("abcd").unwrap_err().to_string();
        assert!(err.contains("32 bytes"));
    }

    #[test]
    fn json_file_writer_creates_parent_directories() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mercury-latch-template-parent-{}-{nanos}/nested/template.json",
            std::process::id()
        ));

        write_json_file(&path, &valid_template_note()).unwrap();

        let saved = load_latch_template(&path).unwrap();
        assert_eq!(
            saved.note.commitment(),
            valid_template_note().note.commitment()
        );
    }

    #[cfg(unix)]
    #[test]
    fn refund_note_is_written_private() {
        use std::os::unix::fs::PermissionsExt;

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let prefix = std::env::temp_dir().join(format!(
            "mercury-latch-refund-perms-{}-{nanos}/nested/swap",
            std::process::id()
        ));

        let refund_path =
            write_latch_refund(&prefix, &valid_template_note(), Element::new(99)).unwrap();
        let mode = fs::metadata(refund_path).unwrap().permissions().mode() & 0o777;

        assert_eq!(mode, 0o600);
    }

    fn valid_template_note() -> EscrowInputNote {
        let payment_hash = [7u8; 32];
        let claim_key_hash = Element::new(11);
        let refund_key_hash = Element::new(12);
        let lock = TimeLock {
            zero_block: [9u8; 32],
            n_blocks: Element::new(2),
        };
        latch_input_note(
            Note {
                utxo_kind: Element::new(2),
                note_kind: Element::new(8),
                address: htlc_claim_address_for_key_hash(claim_key_hash, payment_hash),
                psi: htlc_refund_psi_for_key_hash(refund_key_hash, &lock),
                value: Element::new(1_000),
            },
            Element::ZERO,
            &lock,
        )
    }

    #[test]
    fn latch_template_rejects_prefilled_refund_headers() {
        let mut template = valid_template_note();
        template.time_proof.headers[0][0] = 1;

        let err = verify_latch_note_shape(
            &template,
            Element::new(11),
            [7u8; 32],
            1_000,
            Element::new(2),
            Element::new(8),
            Element::new(12),
            2,
        )
        .unwrap_err();

        assert!(err.contains("refund proof headers"));
    }

    #[test]
    fn latch_template_accepts_empty_refund_headers() {
        let template = valid_template_note();
        let commitment = verify_latch_note_shape(
            &template,
            Element::new(11),
            [7u8; 32],
            1_000,
            Element::new(2),
            Element::new(8),
            Element::new(12),
            2,
        )
        .unwrap();

        assert_eq!(commitment, template.note.commitment());
    }

    #[test]
    fn latch_template_rejects_wrong_refund_blocks() {
        let template = valid_template_note();
        let err = verify_latch_note_shape(
            &template,
            Element::new(11),
            [7u8; 32],
            1_000,
            Element::new(2),
            Element::new(8),
            Element::new(12),
            1,
        )
        .unwrap_err();

        assert!(err.contains("unexpected refund timelock"));
    }

    #[test]
    fn latch_template_rejects_wrong_payment_hash() {
        let template = valid_template_note();
        let err = verify_latch_note_shape(
            &template,
            Element::new(11),
            [8u8; 32],
            1_000,
            Element::new(2),
            Element::new(8),
            Element::new(12),
            2,
        )
        .unwrap_err();

        assert!(err.contains("claim path mismatch"));
    }

    #[test]
    fn latch_template_rejects_wrong_claim_key() {
        let template = valid_template_note();
        let err = verify_latch_note_shape(
            &template,
            Element::new(99),
            [7u8; 32],
            1_000,
            Element::new(2),
            Element::new(8),
            Element::new(12),
            2,
        )
        .unwrap_err();

        assert!(err.contains("claim path mismatch"));
    }

    #[test]
    fn latch_template_rejects_wrong_refund_key() {
        let template = valid_template_note();
        let err = verify_latch_note_shape(
            &template,
            Element::new(11),
            [7u8; 32],
            1_000,
            Element::new(2),
            Element::new(8),
            Element::new(99),
            2,
        )
        .unwrap_err();

        assert!(err.contains("refund path mismatch"));
    }

    #[test]
    fn refund_maturity_uses_required_blocks() {
        assert!(!refund_is_mature_at_height(100, 100, 2));
        assert!(!refund_is_mature_at_height(100, 101, 2));
        assert!(refund_is_mature_at_height(100, 102, 2));
        assert!(refund_is_mature_at_height(100, 103, 2));
    }

    #[test]
    fn refund_maturity_handles_one_block_locks() {
        assert!(!refund_is_mature_at_height(100, 100, 1));
        assert!(refund_is_mature_at_height(100, 101, 1));
    }

    #[test]
    fn refund_maturity_rejects_zero_or_unsupported_locks() {
        let zero = TimeLock {
            zero_block: [1u8; 32],
            n_blocks: Element::ZERO,
        };
        assert!(required_refund_blocks(&zero).is_err());

        let too_large = TimeLock {
            zero_block: [1u8; 32],
            n_blocks: Element::new(REFUND_PROOF_HEADERS + 1),
        };
        assert!(required_refund_blocks(&too_large).is_err());
    }
}
