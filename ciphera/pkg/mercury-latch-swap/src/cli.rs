use std::path::PathBuf;

use clap::{Args as ClapArgs, Parser, Subcommand};

pub(crate) const DEFAULT_BTC_EXPLORER: &str = "https://mempool.space";
pub(crate) const DEFAULT_CITREA_CHAIN: u64 = 5115;
pub(crate) const DEFAULT_REFUND_BLOCKS: u64 = 2;
pub(crate) const DEFAULT_TICKER: &str = "WCBTC";
pub(crate) const DEFAULT_ZEROSATS_HOST: &str = "https://ciphera.satsbridge.com";

#[derive(Parser, Debug, Clone)]
#[command(name = "mercury-latch-swap")]
#[command(about = "Step-by-step Mercury/Ciphera latch swap demo")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum Command {
    /// Create the local swap state file.
    InitState(InitArgs),
    /// Create a Mercury latch payment hash and batch id.
    MercuryCreateHash(StateCommandArgs),
    /// Create a Mercury receive address.
    MercuryAddress(StateCommandArgs),
    /// Create a Zerosats refund address.
    ZerosatsAddress(StateCommandArgs),
    /// Create an unfunded Ciphera latch template.
    ZerosatsTemplate(StateCommandArgs),
    /// Fund the exact Ciphera latch template and save refund material.
    ZerosatsFund(StateCommandArgs),
    /// Verify the funded latch commitment before the Mercury transfer.
    ZerosatsVerify(StateCommandArgs),
    /// Start the Mercury transfer only after funded latch verification
    /// succeeds.
    MercuryTransfer(StateCommandArgs),
    /// Unlock the Mercury transfer after verifying the funded latch.
    MercuryUnlock(StateCommandArgs),
    /// Complete the Mercury receive after unlock.
    MercuryReceive(StateCommandArgs),
    /// Retrieve the Mercury preimage after the receiver can receive.
    MercuryPreimage(StateCommandArgs),
    /// Claim the Ciphera latch with the Mercury preimage.
    ZerosatsClaim(StateCommandArgs),
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct StateCommandArgs {
    #[arg(long, env = "SWAP_STATE")]
    pub(crate) state: PathBuf,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct ZerosatsHostArgs {
    #[arg(long, env = "CIPHERA_HOST", default_value = DEFAULT_ZEROSATS_HOST)]
    pub(crate) zerosats_host: String,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct LatchSettingsArgs {
    #[arg(long, env = "CITREA_CHAIN", default_value_t = DEFAULT_CITREA_CHAIN)]
    pub(crate) citrea_chain: u64,

    #[arg(long, env = "REFUND_BLOCKS", default_value_t = DEFAULT_REFUND_BLOCKS)]
    pub(crate) refund_blocks: u64,

    #[arg(long, env = "CIPHERA_TICKER", default_value = DEFAULT_TICKER)]
    pub(crate) ticker: String,

    #[arg(long, env = "BTC_EXPLORER", default_value = DEFAULT_BTC_EXPLORER)]
    pub(crate) btc_explorer: String,
}

#[derive(ClapArgs, Debug, Clone)]
pub(crate) struct InitArgs {
    #[arg(long, env = "SWAP_STATE")]
    pub(crate) state: PathBuf,

    #[arg(long, env = "MERCURY_SETTINGS_FILE")]
    pub(crate) mercury_settings_file: PathBuf,

    #[arg(long, env = "STATECHAIN_ID")]
    pub(crate) statechain_id: String,

    #[arg(long, env = "AMOUNT_SAT")]
    pub(crate) amount_sat: u64,

    #[arg(long, env = "MERCURY_AMOUNT_SAT")]
    pub(crate) mercury_amount_sat: u64,

    #[arg(long, env = "OWNER_MERCURY_WALLET")]
    pub(crate) owner_mercury_wallet: String,

    #[arg(long, env = "RECEIVER_MERCURY_WALLET")]
    pub(crate) receiver_mercury_wallet: String,

    #[arg(long, env = "FUNDER_ZEROSATS_WALLET")]
    pub(crate) funder_zerosats_wallet: String,

    #[arg(long, env = "CLAIMER_ZEROSATS_WALLET")]
    pub(crate) claimer_zerosats_wallet: String,

    #[arg(long, env = "ZEROSATS_WALLET_DIR")]
    pub(crate) zerosats_wallet_dir: Option<PathBuf>,

    #[command(flatten)]
    pub(crate) node: ZerosatsHostArgs,

    #[command(flatten)]
    pub(crate) latch: LatchSettingsArgs,

    #[arg(long, env = "SWAP_OUTPUT_PREFIX")]
    pub(crate) output_prefix: PathBuf,
}
